//! Bounded HTTP callbacks from the watch. One binary cover in flight, no Base64 on the wire.
use crate::http_probe::{self, ProbeResponse};
use crate::ui::state::{ui_state, ComicInfo, SourceInfo, StatusState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::time::Instant;

pub const CHUNK_BYTES: usize = 16 * 1024;
pub const MAX_COVER_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 10_000;
const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_THUMBNAIL_CHARS: usize = 16 * 1024 * 1024;
pub const BINARY_PROBE: &[u8] = &[0, 1, 127, 128, 255, 0, 42, 13, 10];

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Comic {
    pub id: String,
    pub name: String,
    pub page_count: usize,
    pub chapters: usize,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Source {
    pub name: String,
    #[serde(rename = "apiUrl")]
    pub api_url: String,
}

pub enum Update {
    None,
    Header(usize, usize),
    Comics(Vec<Comic>),
    Sources(Vec<Source>),
    ListsDone,
    Cover(usize, String),
    Progress,
    Complete(usize),
}

struct CoverUpload { id: String, total: usize, bytes: Vec<u8> }
struct LastChunk { id: String, offset: usize, total: usize, bytes: Vec<u8> }
pub struct HttpDataSync {
    counts: Option<(usize, usize)>,
    comics: Vec<Comic>,
    sources: Vec<Source>,
    ids: HashMap<String, usize>,
    lists_done: bool,
    cover: Option<CoverUpload>,
    last_chunk: Option<LastChunk>,
    resolved: HashSet<usize>,
    skipped: usize,
    thumbnail_chars: usize,
    metadata_bytes: usize,
    pub started: bool,
    pub completed: bool,
}

impl Default for HttpDataSync {
    fn default() -> Self {
        Self { counts: None, comics: Vec::new(), sources: Vec::new(), ids: HashMap::new(),
            lists_done: false, cover: None, last_chunk: None, resolved: HashSet::new(),
            skipped: 0, thumbnail_chars: 0, metadata_bytes: 0, started: false, completed: false }
    }
}

type Failure = (u16, &'static str);

fn base64_decode(input: &str) -> Result<Vec<u8>, &'static str> {
    let input = input.trim();
    let data = if let Some(idx) = input.find(',') {
        &input[idx + 1..]
    } else {
        input
    };
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in bytes {
        if b == b'=' || b.is_ascii_whitespace() { continue; }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("Invalid base64 character"),
        } as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

fn number(value: &Value, key: &str) -> Result<usize, Failure> {
    value.get(key).and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok())
        .ok_or((400, "Invalid number"))
}
fn json_body(body: &[u8]) -> Result<Value, Failure> {
    if body.len() > MAX_JSON_BYTES { return Err((413, "Metadata body too large")); }
    serde_json::from_slice(body).map_err(|_| (400, "Invalid JSON"))
}

fn thumbnail(bytes: &[u8]) -> Result<String, Failure> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()
        .map_err(|_| (422, "Invalid cover"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| (422, "Cover cannot be decoded within limits"))?;
    let mut output = Cursor::new(Vec::new());
    image.thumbnail(100, 200).write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| (422, "Cover thumbnail failed"))?;
    Ok(format!("data:image/png;base64,{}", crate::ui::event_handler::base64_encode(output.get_ref())))
}

impl HttpDataSync {
    pub fn apply(&mut self, method: &str, resource: &str, query: &str, body: &[u8]) -> Result<Update, Failure> {
        if resource == "probe" && method == "POST" {
            if body == BINARY_PROBE {
                return Ok(Update::None);
            }
            if let Ok(value) = serde_json::from_slice::<Value>(body) {
                if value.get("probe").and_then(Value::as_str) == Some("test") {
                    return Ok(Update::None);
                }
            }
            return Err((422, "Probe mismatch"));
        }
        if self.completed {
            return if resource == "complete" && method == "POST" { Ok(Update::None) }
                else { Err((409, "Sync already completed")) };
        }
        if resource == "metadata" && method == "POST" {
            let value = json_body(body)?;
            match value.get("kind").and_then(Value::as_str) {
                Some("header") => {
                    let counts = (number(&value, "comicCount")?, number(&value, "sourceCount")?);
                    if counts.0 > MAX_ITEMS || counts.1 > MAX_ITEMS { return Err((413, "Too many items")); }
                    if let Some(previous) = self.counts {
                        return if previous == counts { Ok(Update::None) } else { Err((409, "Header changed")) };
                    }
                    self.counts = Some(counts);
                    self.started = true;
                    return Ok(Update::Header(counts.0, counts.1));
                }
                Some("comics") => {
                    let counts = self.counts.ok_or((409, "Header required"))?;
                    let offset = number(&value, "offset")?;
                    let items: Vec<Comic> = serde_json::from_value(value.get("items").cloned().unwrap_or(Value::Null))
                        .map_err(|_| (400, "Invalid comics"))?;
                    if items.is_empty() || items.len() > 16 || offset.saturating_add(items.len()) > counts.0 {
                        return Err((400, "Invalid comic batch"));
                    }
                    if offset < self.comics.len() {
                        return if self.comics.get(offset..offset + items.len()) == Some(items.as_slice()) {
                            Ok(Update::None)
                        } else { Err((409, "Comic batch changed")) };
                    }
                    if self.lists_done || offset != self.comics.len() { return Err((409, "Comic offset mismatch")); }
                    let mut batch_ids = HashSet::new();
                    if items.iter().any(|c| c.id.is_empty() || c.id.len() > 512 || c.name.len() > 4096 ||
                        self.ids.contains_key(&c.id) || !batch_ids.insert(c.id.clone())) {
                        return Err((400, "Invalid or duplicate comic ID"));
                    }
                    if self.metadata_bytes.saturating_add(body.len()) > MAX_METADATA_BYTES {
                        return Err((413, "Metadata budget exceeded"));
                    }
                    self.metadata_bytes += body.len();
                    for (i, item) in items.iter().enumerate() { self.ids.insert(item.id.clone(), offset + i); }
                    self.comics.extend(items.clone());
                    return Ok(Update::Comics(items));
                }
                Some("sources") => {
                    let counts = self.counts.ok_or((409, "Header required"))?;
                    let offset = number(&value, "offset")?;
                    let items: Vec<Source> = serde_json::from_value(value.get("items").cloned().unwrap_or(Value::Null))
                        .map_err(|_| (400, "Invalid sources"))?;
                    if items.is_empty() || items.len() > 16 || offset.saturating_add(items.len()) > counts.1 ||
                        items.iter().any(|s| s.name.len() > 4096 || s.api_url.len() > 8192) {
                        return Err((400, "Invalid source batch"));
                    }
                    if offset < self.sources.len() {
                        return if self.sources.get(offset..offset + items.len()) == Some(items.as_slice()) {
                            Ok(Update::None)
                        } else { Err((409, "Source batch changed")) };
                    }
                    if self.lists_done || offset != self.sources.len() { return Err((409, "Source offset mismatch")); }
                    if self.metadata_bytes.saturating_add(body.len()) > MAX_METADATA_BYTES {
                        return Err((413, "Metadata budget exceeded"));
                    }
                    self.metadata_bytes += body.len();
                    self.sources.extend(items.clone());
                    return Ok(Update::Sources(items));
                }
                Some("done") => {
                    if self.counts != Some((self.comics.len(), self.sources.len())) {
                        return Err((409, "Metadata incomplete"));
                    }
                    if self.lists_done { return Ok(Update::None); }
                    self.lists_done = true;
                    return Ok(Update::ListsDone);
                }
                _ => return Err((400, "Unknown metadata kind")),
            }
        }
        if resource == "skip" && method == "POST" {
            if !self.lists_done { return Err((409, "Metadata not complete")); }
            let value = json_body(body)?;
            let id = value.get("id").and_then(Value::as_str).ok_or((400, "Comic ID required"))?;
            let index = *self.ids.get(id).ok_or((404, "Comic not found"))?;
            if self.resolved.insert(index) {
                if self.cover.as_ref().is_some_and(|cover| cover.id == id) { self.cover = None; }
                if self.last_chunk.as_ref().is_some_and(|chunk| chunk.id == id) { self.last_chunk = None; }
                self.skipped += 1;
                return Ok(Update::Progress);
            }
            return Ok(Update::None);
        }
        if resource.starts_with("covers/") && (method == "PUT" || method == "POST") {
            if !self.lists_done { return Err((409, "Metadata not complete")); }
            let index = resource[7..].parse::<usize>().map_err(|_| (400, "Invalid cover index"))?;
            let id = self.comics.get(index).ok_or((404, "Comic not found"))?.id.clone();
            let params: HashMap<_, _> = query.split('&').filter_map(|pair| pair.split_once('=')).collect();
            let offset = params.get("offset").and_then(|v| v.parse::<usize>().ok()).ok_or((400, "Invalid offset"))?;
            let total = params.get("total").and_then(|v| v.parse::<usize>().ok()).ok_or((400, "Invalid total"))?;
            if total == 0 || total > MAX_COVER_BYTES || body.is_empty() { return Err((413, "Invalid cover size")); }
            let raw_bytes = if body.starts_with(b"{") {
                let val = json_body(body)?;
                let b64 = val.get("data").and_then(Value::as_str).ok_or((400, "Missing cover data"))?;
                base64_decode(b64).map_err(|_| (400, "Invalid base64 cover"))?
            } else {
                body.to_vec()
            };
            if raw_bytes.is_empty() || raw_bytes.len() > CHUNK_BYTES || offset.saturating_add(raw_bytes.len()) > total {
                return Err((413, "Invalid chunk size"));
            }
            if let Some(last) = &self.last_chunk {
                if last.id == id && last.offset == offset && last.total == total && last.bytes == raw_bytes {
                    return Ok(Update::None);
                }
            }
            if self.resolved.contains(&index) { return Err((409, "Cover already resolved")); }
            if self.cover.is_none() {
                if offset != 0 { return Err((409, "Cover must start at zero")); }
                self.cover = Some(CoverUpload { id: id.clone(), total, bytes: Vec::new() });
            }
            let cover = self.cover.as_mut().unwrap();
            if cover.id != id || cover.total != total { return Err((409, "Another cover is in flight")); }
            if offset < cover.bytes.len() {
                return if cover.bytes.get(offset..offset + raw_bytes.len()) == Some(&raw_bytes) { Ok(Update::None) }
                    else { Err((409, "Chunk changed")) };
            }
            if offset != cover.bytes.len() { return Err((409, "Cover offset mismatch")); }
            cover.bytes.extend_from_slice(&raw_bytes);
            self.last_chunk = Some(LastChunk { id, offset, total, bytes: raw_bytes });
            if cover.bytes.len() == total {
                let cover = self.cover.take().unwrap();
                let thumbnail = match thumbnail(&cover.bytes) {
                    Ok(thumbnail) => thumbnail,
                    Err(error) => { self.last_chunk = None; return Err(error); }
                };
                if self.thumbnail_chars.saturating_add(thumbnail.len()) > MAX_THUMBNAIL_CHARS {
                    self.last_chunk = None;
                    return Err((413, "Thumbnail budget exceeded"));
                }
                self.thumbnail_chars += thumbnail.len();
                self.resolved.insert(index);
                return Ok(Update::Cover(index, thumbnail));
            }
            return Ok(Update::Progress);
        }
        if resource == "complete" && method == "POST" {
            if !self.lists_done || self.cover.is_some() || self.resolved.len() != self.comics.len() {
                return Err((409, "Sync incomplete"));
            }
            self.completed = true;
            self.last_chunk = None;
            return Ok(Update::Complete(self.skipped));
        }
        Err((405, "Unsupported sync method or route"))
    }
}

pub fn fallback(session: &str) -> bool {
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    if !state.sync_receive.active() || !state.sync_receive.matches_session(Some(session)) ||
        state.http_data_sync.as_ref().is_none_or(|sync| sync.started) { return false; }
    state.http_data_sync = None;
    true
}

pub fn route(method: &str, path: &str, query: &str, body: &[u8]) -> Option<ProbeResponse> {
    let tail = path.strip_prefix("/control/sync/")?;
    tracing::info!("HTTP 数据同步请求: {} {}", method, path);
    let Some((session, resource)) = tail.split_once('/') else { return Some(http_probe::error(404, "Sync route not found")); };
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    if !state.sync_receive.matches_session(Some(session)) || state.http_data_sync.is_none() {
        return Some(http_probe::error(409, "Sync session expired"));
    }
    if !state.sync_receive.active() && !state.http_data_sync.as_ref().unwrap().completed {
        return Some(http_probe::error(409, "Sync stopped"));
    }
    if state.sync_receive.remaining(state.sync_receive.generation(), Instant::now()).is_some_and(|d| d.is_zero()) {
        state.http_data_sync = None;
        state.sync_receive.finish();
        state.app_data_status = StatusState::Error("HTTP 数据同步超时，请重试".into());
        let timer_id = state.app_data_recv_timer_id.take();
        drop(state);
        if let Some(id) = timer_id { crate::astrobox::psys_host_v4::timer::clear_timer(id); }
        crate::ui::build::rerender_main_ui();
        return Some(http_probe::error(409, "Sync timed out"));
    }
    let update = match state.http_data_sync.as_mut().unwrap().apply(method, resource, query, body) {
        Ok(update) => update,
        Err((code, message)) => return Some(http_probe::error(code, message)),
    };
    let changed = !matches!(update, Update::None);
    let refresh = !matches!(&update, Update::None | Update::Progress);
    let mut timer_id = None;
    match update {
        Update::Header(comics, sources) => {
            state.app_comic_count = Some(comics);
            state.app_source_count = Some(sources);
        }
        Update::Comics(items) => state.app_comics.extend(items.into_iter().map(|item| ComicInfo {
            id: item.id, name: item.name, page_count: item.page_count, chapters: item.chapters, cover_base64: String::new(),
        })),
        Update::Sources(items) => state.app_sources.extend(items.into_iter().map(|item| SourceInfo {
            name: item.name, api_url: item.api_url,
        })),
        Update::ListsDone => {
            state.sync_receive.http_covers(Instant::now());
            state.app_data_status = StatusState::Processing("HTTP 列表已接收，正在接收封面...".into());
        }
        Update::Cover(index, cover) => {
            state.app_comics[index].cover_base64 = cover;
            let received = state.http_data_sync.as_ref().unwrap().resolved.len();
            state.app_data_status = StatusState::Processing(format!("HTTP 封面 {}/{}", received, state.app_comics.len()));
        }
        Update::Complete(skipped) => {
            state.sync_receive.finish();
            timer_id = state.app_data_recv_timer_id.take();
            state.app_data_status = StatusState::Success(if skipped == 0 { "数据获取成功（HTTP）".into() }
                else { format!("数据获取完成（{} 张封面缺失或未上传）", skipped) });
        }
        _ => {}
    }
    if changed { state.sync_receive.progress(Instant::now()); }
    let ack = json!({"ok": true, "receivedComics": state.app_comics.len(), "receivedSources": state.app_sources.len(),
        "resolvedCovers": state.http_data_sync.as_ref().map(|sync| sync.resolved.len()).unwrap_or(0),
        "skippedCovers": state.http_data_sync.as_ref().map(|sync| sync.skipped).unwrap_or(0)}).to_string().into_bytes();
    drop(state);
    if let Some(id) = timer_id { crate::astrobox::psys_host_v4::timer::clear_timer(id); }
    if refresh {
        crate::ui::build::rerender_main_ui();
        let card_visible = ui_state().read().unwrap_or_else(|p| p.into_inner()).root_element_id.is_some();
        if card_visible {
            crate::ui::build::render_comic_data_card(crate::ui::COMIC_DATA_CARD_ID);
        }
    }
    Some(ProbeResponse { status: 200, headers: vec![("Content-Type".into(), "application/json; charset=utf-8".into()),
        ("Content-Length".into(), ack.len().to_string()), ("Cache-Control".into(), "no-store".into())], body: ack })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(sync: &mut HttpDataSync, value: Value) -> Result<Update, Failure> {
        sync.apply("POST", "metadata", "", &serde_json::to_vec(&value).unwrap())
    }
    fn ready() -> HttpDataSync {
        let mut sync = HttpDataSync::default();
        metadata(&mut sync, json!({"kind":"header", "comicCount":2, "sourceCount":0})).unwrap();
        metadata(&mut sync, json!({"kind":"comics", "offset":0, "items":[
            {"id":"first", "name":"Same name", "page_count":1, "chapters":0},
            {"id":"second", "name":"Same name", "page_count":2, "chapters":0}
        ]})).unwrap();
        metadata(&mut sync, json!({"kind":"done"})).unwrap();
        sync
    }
    #[test]
    fn probe_requires_exact_raw_bytes_including_nul_and_high_bytes() {
        let mut sync = HttpDataSync::default();
        assert!(sync.apply("POST", "probe", "", BINARY_PROBE).is_ok());
        assert!(sync.apply("POST", "probe", "", b"0,1,127,128,255,0,42,13,10").is_err());
        assert!(!sync.started);
    }
    #[test]
    fn metadata_replays_are_idempotent_and_limits_precede_allocation() {
        let mut sync = ready();
        assert!(matches!(metadata(&mut sync, json!({"kind":"header", "comicCount":2, "sourceCount":0})), Ok(Update::None)));
        assert!(metadata(&mut sync, json!({"kind":"header", "comicCount":3, "sourceCount":0})).is_err());
        let mut empty = HttpDataSync::default();
        assert!(metadata(&mut empty, json!({"kind":"header", "comicCount":MAX_ITEMS+1, "sourceCount":0})).is_err());
        assert!(!empty.started);
        assert!(sync.apply("POST", "complete", "", b"{}").is_err());
    }
    #[test]
    fn binary_cover_chunks_replay_once_and_same_names_keep_distinct_slots() {
        let mut sync = ready();
        let png = &http_probe::samples().unwrap().png;
        assert!(matches!(sync.apply("PUT", "covers/1", &format!("offset=0&total={}", png.len()), &png[..8]), Ok(Update::Progress)));
        assert!(matches!(sync.apply("PUT", "covers/1", &format!("offset=0&total={}", png.len()), &png[..8]), Ok(Update::None)));
        assert!(sync.apply("PUT", "covers/0", &format!("offset=0&total={}", png.len()), &png[..8]).is_err());
        assert!(matches!(sync.apply("PUT", "covers/1", &format!("offset=8&total={}", png.len()), &png[8..]), Ok(Update::Cover(1, _))));
        assert!(matches!(sync.apply("PUT", "covers/1", &format!("offset=8&total={}", png.len()), &png[8..]), Ok(Update::None)));
        assert!(matches!(sync.apply("POST", "skip", "", br#"{"id":"first"}"#), Ok(Update::Progress)));
        assert!(matches!(sync.apply("POST", "complete", "", b"{}"), Ok(Update::Complete(1))));
        assert!(matches!(sync.apply("POST", "complete", "", b"{}"), Ok(Update::None)));
    }
    #[test]
    fn invalid_cover_and_short_transfer_cannot_be_declared_complete() {
        let mut sync = ready();
        assert!(sync.apply("PUT", "covers/0", "offset=0&total=3", &[0, 128, 255]).is_err());
        assert!(sync.apply("POST", "complete", "", b"{}").is_err());
        assert!(sync.apply("PUT", "covers/1", "offset=0&total=2097153", &[1]).is_err());
        assert!(sync.cover.is_none());
        sync.apply("POST", "skip", "", br#"{"id":"first"}"#).unwrap();
        sync.apply("POST", "skip", "", br#"{"id":"second"}"#).unwrap();
        assert!(matches!(sync.apply("POST", "complete", "", b"{}"), Ok(Update::Complete(2))));
    }
    #[test]
    fn duplicate_ids_and_exhausted_metadata_budget_do_not_mutate_the_list() {
        let mut sync = HttpDataSync::default();
        metadata(&mut sync, json!({"kind":"header", "comicCount":2, "sourceCount":0})).unwrap();
        let item = json!({"id":"same", "name":"Book", "page_count":1, "chapters":0});
        assert!(metadata(&mut sync, json!({"kind":"comics", "offset":0, "items":[item.clone(),item.clone()]})).is_err());
        assert!(sync.comics.is_empty());
        sync.metadata_bytes = MAX_METADATA_BYTES;
        assert!(metadata(&mut sync, json!({"kind":"comics", "offset":0, "items":[item]})).is_err());
        assert!(sync.comics.is_empty());
        assert!(sync.ids.is_empty());
    }
}
