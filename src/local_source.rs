//! Local漫画源与内容服务 (HTTP-3)
//!
//! 实现符合 docs/CUSTOM_SOURCE.md 及 todo.md 批次 11 规范的 LocalUpload 漫画源：
//! - `GET /config`：漫画源定义
//! - `GET /local/search/<text>/<page>`：本地漫画搜索
//! - `GET /local/album/<id>`：标准漫画详情
//! - `GET /local/album/<id>/cover`：封面图片
//! - `GET /local/photo/<id>/chapter/<chapter>`：章节图片列表
//! - `GET /local/photo/<id>/chapter/<chapter>/<page>.jpg`：正文单页图片（支持 width/quality/ifPNG/ifLVGL 参数）

#[cfg(test)]
use std::io::Cursor;
#[cfg(test)]
use image::{DynamicImage, ImageFormat};
use serde_json::json;
use crate::image_processor::{ImageRequest, ImageRole, PreparedImage, ImageProcessError};

use crate::http_probe::{samples, ProbeResponse};
use crate::ui::state::{ui_state, UploadMode};

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub struct QueryParams {
    pub width: u32,
    pub quality: u32,
    pub if_png: bool,
    pub if_lvgl: bool,
}

impl QueryParams {
    pub fn parse(query: &str) -> Self {
        let mut width = 480;
        let mut quality = 50;
        let mut if_png = false;
        let mut if_lvgl = false;

        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                match k {
                    "width" => {
                        if let Ok(w) = v.parse::<u64>() {
                            width = w.clamp(10, 4096) as u32;
                        }
                    }
                    "quality" => {
                        if let Ok(q) = v.parse::<u64>() {
                            quality = q.clamp(1, 100) as u32;
                        }
                    }
                    "ifPNG" => {
                        if_png = v == "1" || v.eq_ignore_ascii_case("true");
                    }
                    "ifLVGL" => {
                        if_lvgl = v == "1" || v.eq_ignore_ascii_case("true");
                    }
                    _ => {}
                }
            }
        }

        Self {
            width,
            quality,
            if_png,
            if_lvgl,
        }
    }
}

#[derive(Clone)]
pub struct LocalChapterData {
    pub chapter_number: usize,
    pub title: String,
    pub pages: Vec<LocalAsset>,
}

#[derive(Clone)]
pub enum LocalAsset { Disk(String), Memory(Vec<u8>) }
impl LocalAsset {
    fn read(&self) -> Result<Vec<u8>, ImageProcessError> {
        match self {
            Self::Disk(path) => crate::assets::read_master(path),
            Self::Memory(bytes) => Ok(bytes.clone()),
        }
    }
    fn from_upload(file: &crate::ui::state::UploadFile) -> Self {
        match &file.disk_path {
            Some(path) => Self::Disk(path.clone()),
            None => Self::Memory(file.data.clone()),
        }
    }
}

#[derive(Clone)]
pub struct LocalComicData {
    pub id: String,
    pub name: String,
    pub cover: Option<LocalAsset>,
    pub chapters: Vec<LocalChapterData>,
    pub revision: String,
}

static PUBLISHED: std::sync::Mutex<Vec<LocalComicData>> = std::sync::Mutex::new(Vec::new());

pub fn publish(mut comic: LocalComicData) -> LocalComicData {
    comic.id = format!("book_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    comic.revision = comic.id.clone();
    let mut published = PUBLISHED.lock().unwrap_or_else(|p| p.into_inner());
    if published.len() >= 32 { published.remove(0); }
    published.push(comic.clone());
    comic
}

impl LocalComicData {
    pub fn total_pages(&self) -> usize {
        self.chapters.iter().map(|ch| ch.pages.len()).sum()
    }
}

/// 从 UI 状态中提取当前配置的本地漫画；若未选图则提供一份示例漫画用于快速自测与联调
pub fn get_catalog() -> Vec<LocalComicData> {
    let state = ui_state().read().unwrap_or_else(|p| p.into_inner());

    let mut comics = Vec::new();

    match state.upload_mode {
        UploadMode::Single => {
            for (i, item) in state.upload_items.iter().enumerate() {
                if item.files.is_empty() {
                    continue;
                }
                let name = if !state.upload_comic_name_input.trim().is_empty() {
                    state.upload_comic_name_input.trim().to_string()
                } else if item.comic_name.trim().is_empty() {
                    format!("本地漫画 {}", i + 1)
                } else {
                    item.comic_name.clone()
                };
                let id = format!("single_{}", i + 1);
                let pages: Vec<LocalAsset> = item
                    .files
                    .iter()
                    .map(LocalAsset::from_upload)
                    .collect();
                let cover = item
                    .cover
                    .as_ref()
                    .map(LocalAsset::from_upload);
                comics.push(LocalComicData {
                    id,
                    name,
                    cover,
                    chapters: vec![LocalChapterData {
                        chapter_number: 1,
                        title: "第1章".to_string(),
                        pages,
                    }],
                    revision: "rev_single".to_string(),
                });
            }
        }
        UploadMode::Multi => {
            let has_files = state.upload_chapters.iter().any(|c| !c.files.is_empty());
            if has_files {
                let name = if state.upload_comic_name_input.trim().is_empty() {
                    "本地多章节漫画".to_string()
                } else {
                    state.upload_comic_name_input.clone()
                };
                let mut chapters = Vec::new();
                for chapter in &state.upload_chapters {
                    if chapter.files.is_empty() {
                        continue;
                    }
                    let title = if chapter.name.trim().is_empty() {
                        format!("第{}章", chapter.number)
                    } else {
                        chapter.name.clone()
                    };
                    let pages: Vec<LocalAsset> = chapter
                        .files
                        .iter()
                        .map(LocalAsset::from_upload)
                        .collect();
                    chapters.push(LocalChapterData {
                        chapter_number: chapter.number,
                        title,
                        pages,
                    });
                }
                let cover = state
                    .multi_cover
                    .as_ref()
                    .map(LocalAsset::from_upload);
                comics.push(LocalComicData {
                    id: "multi_1".to_string(),
                    name,
                    cover,
                    chapters,
                    revision: "rev_multi".to_string(),
                });
            }
        }
    }

    if comics.is_empty() {
        // 提供一份内置示例漫画，确保新安装插件未选图时也能完整测试搜索、详情与图片阅读链路
        if let Ok(samples) = samples() {
            comics.push(LocalComicData {
                id: "sample_book".to_string(),
                name: "本地示例漫画".to_string(),
                cover: Some(LocalAsset::Memory(samples.png.clone())),
                chapters: vec![
                    LocalChapterData {
                        chapter_number: 1,
                        title: "第一章 探针".to_string(),
                        pages: vec![LocalAsset::Memory(samples.png.clone()), LocalAsset::Memory(samples.jpeg.clone())],
                    },
                    LocalChapterData {
                        chapter_number: 2,
                        title: "第二章 彩页".to_string(),
                        pages: vec![LocalAsset::Memory(samples.jpeg.clone())],
                    },
                ],
                revision: "rev_sample_01".to_string(),
            });
        }
    }

    comics
}

fn json_response(status: u16, value: &serde_json::Value) -> ProbeResponse {
    let body = value.to_string().into_bytes();
    ProbeResponse {
        status,
        headers: vec![
            ("Content-Type".into(), "application/json; charset=utf-8".into()),
            ("Content-Length".into(), body.len().to_string()),
            ("Cache-Control".into(), "no-store".into()),
        ],
        body,
    }
}

fn error_json(status: u16, msg: &str) -> ProbeResponse {
    json_response(status, &json!({ "code": status, "message": msg }))
}

/// HTTP 参数只负责适配；所有图片规则和缓存均由共用处理器执行。
pub fn process_image(
    data: &[u8],
    params: &QueryParams,
    allow_lvgl: bool,
) -> Result<PreparedImage, ImageProcessError> {
    crate::image_processor::prepare_image(data, &ImageRequest {
        role: if allow_lvgl { ImageRole::Page } else { ImageRole::Cover },
        width: params.width, quality: params.quality,
        if_png: params.if_png, if_lvgl: params.if_lvgl,
    })
}

fn image_response(asset: &LocalAsset, params: &QueryParams, is_page: bool, comic_id: &str, context: &str) -> ProbeResponse {
    let result = asset.read().and_then(|bytes| process_image(&bytes, params, is_page));
    match result {
        Ok(product) => {
            if crate::jobs::owns_image_request(comic_id) {
                crate::ui::event_handler::record_image_notices(context, &product.notices);
            }
            ProbeResponse {
                status: 200,
                headers: vec![("Content-Type".into(), product.format.mime().into()),
                    ("Content-Length".into(), product.bytes.len().to_string()), ("Cache-Control".into(), "no-store".into())],
                body: product.bytes,
            }
        }
        Err(error) => {
            let message = format!("{context}：{error}");
            if crate::jobs::owns_image_request(comic_id) {
                crate::ui::event_handler::record_image_notices("HTTP 图片", &[message.clone()]);
            }
            error_json(error.http_status(), &message)
        }
    }
}

/// 路由分发
pub fn route_local(
    method: &str,
    path: &str,
    query: &str,
    base_url: &str,
) -> Option<ProbeResponse> {
    if !path.starts_with("/local/") && path != "/config" {
        return None;
    }

    if method != "GET" {
        let mut resp = error_json(405, "Method not allowed");
        resp.headers.push(("Allow".into(), "GET".into()));
        return Some(resp);
    }

    // 1. GET /config
    if path == "/config" {
        return Some(json_response(
            200,
            &json!({
                "LocalUpload": {
                    "name": "本地漫画",
                    "apiUrl": base_url,
                    "detailPath": "/local/album/<id>",
                    "photoPath": "/local/photo/<id>/chapter/<chapter>",
                    "searchPath": "/local/search/<text>/<page>",
                    "type": "local"
                }
            }),
        ));
    }

    let published = PUBLISHED.lock().unwrap_or_else(|p| p.into_inner());
    let requested_id = path.split('/').nth(3).unwrap_or("");
    let found = published.iter().find(|c| c.id == requested_id).cloned();
    drop(published);
    let catalog = if let Some(comic) = found { vec![comic] } else { get_catalog() };

    // 2. GET /local/search/<text>/<page>
    if let Some(rest) = path.strip_prefix("/local/search/") {
        let parts: Vec<&str> = rest.split('/').collect();
        let (raw_text, page_str) = match parts.as_slice() {
            [text, page] => (*text, *page),
            [text] => (*text, "1"),
            _ => return Some(error_json(400, "Invalid search path format")),
        };

        let decoded_text = percent_decode(raw_text);
        let keyword = decoded_text.trim();
        let page: usize = page_str.parse().unwrap_or(1).max(1);

        let filtered: Vec<&LocalComicData> = catalog
            .iter()
            .filter(|c| {
                keyword.is_empty()
                    || keyword == "all"
                    || c.name.to_lowercase().contains(&keyword.to_lowercase())
            })
            .collect();

        // 默认一次返回全部结果（本地内容集合小），后续可分页
        let results: Vec<serde_json::Value> = filtered
            .iter()
            .map(|c| {
                let cover_url = if c.cover.is_some() {
                    format!("{}/local/album/{}/cover", base_url, c.id)
                } else {
                    String::new()
                };
                json!({
                    "comic_id": c.id,
                    "title": c.name,
                    "cover_url": cover_url
                })
            })
            .collect();

        return Some(json_response(
            200,
            &json!({
                "page": page,
                "has_more": false,
                "results": results
            }),
        ));
    }

    // 3. GET /local/album/<id>/cover
    if let Some(rest) = path.strip_prefix("/local/album/") {
        if let Some(id) = rest.strip_suffix("/cover") {
            let comic = catalog.iter().find(|c| c.id == id);
            let Some(comic) = comic else {
                return Some(error_json(404, "Comic not found"));
            };
            let Some(cover_bytes) = &comic.cover else {
                return Some(error_json(404, "Comic has no cover"));
            };
            let params = QueryParams::parse(query);
            // 封面不生成 LVGL indexed-8，保持普通图片格式
            return Some(image_response(cover_bytes, &params, false, &comic.id, &format!("《{}》封面", comic.name)));
        }

        // 4. GET /local/album/<id>
        let id = rest;
        let comic = catalog.iter().find(|c| c.id == id);
        let Some(comic) = comic else {
            return Some(error_json(404, "Comic not found"));
        };

        let cover_url = if comic.cover.is_some() {
            format!("{}/local/album/{}/cover", base_url, comic.id)
        } else {
            String::new()
        };

        return Some(json_response(
            200,
            &json!({
                "item_id": comic.id,
                "name": comic.name,
                "page_count": comic.total_pages(),
                "cover": cover_url,
                "tags": ["本地漫画"],
                "total_chapters": comic.chapters.iter().map(|ch| ch.chapter_number).max().unwrap_or(1)
            }),
        ));
    }

    // 5. GET /local/photo/<id>/chapter/<chapter>/<page>.jpg
    if let Some(rest) = path.strip_prefix("/local/photo/") {
        let parts: Vec<&str> = rest.split('/').collect();
        // 格式 A: <id>/chapter/<chapter>/<page>.<ext>
        // 格式 B: <id>/chapter/<chapter>
        if parts.len() == 4 && parts[1] == "chapter" {
            let id = parts[0];
            let chapter_idx: usize = parts[2].parse().unwrap_or(0);
            let page_part = parts[3];
            let page_idx_str = page_part.split('.').next().unwrap_or(page_part);
            let page_idx: usize = page_idx_str.parse().unwrap_or(0);

            let comic = catalog.iter().find(|c| c.id == id);
            let Some(comic) = comic else {
                return Some(error_json(404, "Comic not found"));
            };

            let chapter = comic
                .chapters
                .iter()
                .find(|ch| ch.chapter_number == chapter_idx);
            let Some(chapter) = chapter else {
                return Some(error_json(404, "Chapter not found"));
            };

            if page_idx == 0 || page_idx > chapter.pages.len() {
                return Some(error_json(404, "Page not found"));
            }

            let page_bytes = &chapter.pages[page_idx - 1];
            let params = QueryParams::parse(query);

            return Some(image_response(page_bytes, &params, true, &comic.id, &format!("《{}》第{}章第{}页", comic.name, chapter_idx, page_idx)));
        } else if parts.len() == 3 && parts[1] == "chapter" {
            // GET /local/photo/<id>/chapter/<chapter>
            let id = parts[0];
            let chapter_idx: usize = parts[2].parse().unwrap_or(0);

            let comic = catalog.iter().find(|c| c.id == id);
            let Some(comic) = comic else {
                return Some(error_json(404, "Comic not found"));
            };

            let chapter = comic
                .chapters
                .iter()
                .find(|ch| ch.chapter_number == chapter_idx);
            let Some(chapter) = chapter else {
                return Some(error_json(404, "Chapter not found"));
            };

            let images: Vec<serde_json::Value> = (1..=chapter.pages.len())
                .map(|p| {
                    json!({
                        "url": format!(
                            "{}/local/photo/{}/chapter/{}/{}.jpg?revision={}",
                            base_url, comic.id, chapter.chapter_number, p, comic.revision
                        )
                    })
                })
                .collect();

            return Some(json_response(
                200,
                &json!({
                    "title": chapter.title,
                    "images": images
                }),
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_route_enforces_width_and_image_errors_are_json_with_a_real_status() {
        let image = DynamicImage::new_rgba8(240, 480);
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        let comic = publish(LocalComicData { id: "errors".into(), name: "图片错误测试".into(), revision: "draft".into(),
            cover: Some(LocalAsset::Memory(output.into_inner())),
            chapters: vec![LocalChapterData { chapter_number: 1, title: "Chapter".into(),
                pages: vec![LocalAsset::Memory(b"<html>bad image</html>".to_vec()), LocalAsset::Disk("cache/missing-image-test.png".into())] }] });
        let base = "http://host:1234";
        let cover = route_local("GET", &format!("/local/album/{}/cover", comic.id), "width=1000&ifLVGL=1&ifPNG=1", base).unwrap();
        assert_eq!(image::load_from_memory(&cover.body).unwrap().width(), 80);
        assert!(cover.headers.contains(&("Content-Type".into(), "image/png".into())));
        for (page, status, stage) in [(1, 422, "解码"), (2, 500, "读取")] {
            let response = route_local("GET", &format!("/local/photo/{}/chapter/1/{page}.jpg", comic.id), "ifLVGL=1", base).unwrap();
            assert_eq!(response.status, status);
            assert!(response.headers.contains(&("Content-Type".into(), "application/json; charset=utf-8".into())));
            let error: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
            assert!(error["message"].as_str().unwrap().contains(stage));
            assert!(error["message"].as_str().unwrap().contains(&format!("第{page}页")));
        }
    }

    #[test]
    fn published_cover_is_independent_of_pages_and_later_uploads() {
        fn png(color: [u8; 3]) -> Vec<u8> {
            let image = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(96, 128, image::Rgb(color)));
            let mut buffer = Cursor::new(Vec::new());
            image.write_to(&mut buffer, ImageFormat::Png).unwrap();
            buffer.into_inner()
        }
        let red = png([255, 0, 0]);
        let blue = png([0, 0, 255]);
        let mut draft = LocalComicData {
            id: "cover_test".into(),
            name: "Same name".into(),
            cover: Some(LocalAsset::Memory(red)),
            chapters: vec![LocalChapterData {
                chapter_number: 1,
                title: "Chapter".into(),
                pages: vec![LocalAsset::Memory(blue.clone())],
            }],
            revision: "draft".into(),
        };
        let first = publish(draft.clone());
        draft.cover = Some(LocalAsset::Memory(blue));
        let second = publish(draft);
        let base = "http://192.168.1.100:51963";
        for (comic, expected) in [(&first, [255, 0, 0]), (&second, [0, 0, 255])] {
            let response = route_local("GET", &format!("/local/album/{}/cover", comic.id),
                "width=80&ifPNG=1&ifLVGL=1", base).unwrap();
            assert_eq!(response.status, 200);
            assert!(response.headers.contains(&("Content-Type".into(), "image/png".into())));
            let image = image::load_from_memory(&response.body).unwrap().to_rgb8();
            assert_eq!(image.width(), 80);
            assert_eq!(image.get_pixel(0, 0).0, expected);
        }
        let response = route_local("GET", &format!("/local/photo/{}/chapter/1/1.jpg", first.id),
            "width=80&ifPNG=1", base).unwrap();
        assert_eq!(response.status, 200);
        let image = image::load_from_memory(&response.body).unwrap().to_rgb8();
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255]);
    }

    #[test]
    fn sparse_chapter_detail_keeps_real_chapter_numbers_and_optional_cover() {
        let comic = publish(LocalComicData {
            id: "sparse_test".into(), name: "Sparse".into(), cover: None,
            revision: "draft".into(),
            chapters: vec![LocalChapterData {
                chapter_number: 5, title: "Fifth".into(),
                pages: vec![LocalAsset::Memory(samples().unwrap().png.clone())],
            }],
        });
        let response = route_local("GET", &format!("/local/album/{}", comic.id), "", "http://host:1234").unwrap();
        let detail: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(detail["total_chapters"], 5);
        assert_eq!(detail["page_count"], 1);
        assert_eq!(detail["cover"], "");
        let chapter = route_local("GET", &format!("/local/photo/{}/chapter/5", comic.id), "", "http://host:1234").unwrap();
        assert_eq!(chapter.status, 200);
    }

    #[test]
    fn query_params_parsing_handles_defaults_and_clamps() {
        let p = QueryParams::parse("width=600&quality=85&ifPNG=1");
        assert_eq!(p.width, 600);
        assert_eq!(p.quality, 85);
        assert!(p.if_png);
        assert!(!p.if_lvgl);

        let p2 = QueryParams::parse("ifLVGL=1&width=9999&quality=200");
        assert_eq!(p2.width, 4096);
        assert_eq!(p2.quality, 100);
        assert!(p2.if_lvgl);
    }

    #[test]
    fn out_of_range_http_and_watch_quality_normalize_before_narrowing() {
        let master = samples().unwrap().png.as_slice();
        for value in [0u64, 300, u64::MAX] {
            let query = QueryParams::parse(&format!("width={value}&quality={value}"));
            let watch = crate::ui::state::WatchSettings::from_json(&json!({"imageSize":value,"imageQuality":value}));
            let http = process_image(master, &query, true).unwrap();
            let legacy = crate::image_processor::prepare_image(master, &watch.image_request(ImageRole::Page)).unwrap();
            assert_eq!(http, legacy);
        }
    }

    #[test]
    fn local_config_endpoint_and_search_returns_valid_protocol() {
        let base = "http://192.168.1.100:51963";
        let config_resp = route_local("GET", "/config", "", base).unwrap();
        assert_eq!(config_resp.status, 200);
        let val: serde_json::Value = serde_json::from_slice(&config_resp.body).unwrap();
        assert_eq!(val["LocalUpload"]["apiUrl"], base);
        assert_eq!(val["LocalUpload"]["type"], "local");

        // Search
        let search_resp = route_local("GET", "/local/search/all/1", "", base).unwrap();
        assert_eq!(search_resp.status, 200);
        let s_val: serde_json::Value = serde_json::from_slice(&search_resp.body).unwrap();
        assert_eq!(s_val["page"], 1);
        let results = s_val["results"].as_array().unwrap();
        assert!(!results.is_empty());
        let item_id = results[0]["comic_id"].as_str().unwrap();

        // Album detail
        let album_resp = route_local("GET", &format!("/local/album/{}", item_id), "", base).unwrap();
        assert_eq!(album_resp.status, 200);
        let a_val: serde_json::Value = serde_json::from_slice(&album_resp.body).unwrap();
        assert_eq!(a_val["item_id"], item_id);
        assert!(a_val["page_count"].as_u64().unwrap() > 0);

        // Photo list
        let photo_resp = route_local("GET", &format!("/local/photo/{}/chapter/1", item_id), "", base).unwrap();
        assert_eq!(photo_resp.status, 200);
        let p_val: serde_json::Value = serde_json::from_slice(&photo_resp.body).unwrap();
        assert!(!p_val["images"].as_array().unwrap().is_empty());
    }

    #[test]
    fn local_image_request_supports_jpeg_png_and_lvgl() {
        let base = "http://192.168.1.100:51963";
        // JPEG page
        let jpeg_resp = route_local("GET", "/local/photo/sample_book/chapter/1/1.jpg", "width=80&quality=60", base).unwrap();
        assert_eq!(jpeg_resp.status, 200);
        assert!(jpeg_resp.headers.contains(&("Content-Type".into(), "image/jpeg".into())));

        // PNG page
        let png_resp = route_local("GET", "/local/photo/sample_book/chapter/1/1.jpg", "width=80&ifPNG=1", base).unwrap();
        assert_eq!(png_resp.status, 200);
        assert!(png_resp.headers.contains(&("Content-Type".into(), "image/png".into())));

        // LVGL page
        let lvgl_resp = route_local("GET", "/local/photo/sample_book/chapter/1/1.jpg", "width=32&ifLVGL=1", base).unwrap();
        assert_eq!(lvgl_resp.status, 200);
        assert!(lvgl_resp.headers.contains(&("Content-Type".into(), "application/octet-stream".into())));
        assert_eq!(lvgl_resp.body.len(), 4 + 256 * 4 + 32 * 32);

        // Out of bounds page returns 404 JSON
        let err_resp = route_local("GET", "/local/photo/sample_book/chapter/1/999.jpg", "", base).unwrap();
        assert_eq!(err_resp.status, 404);
    }
}
