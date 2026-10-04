//! Single-item deletion requests outlive library refreshes and handshake changes.
use super::state::{DataItem, DataTarget};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::Instant;

pub const DELETE_TIMEOUT_EVENT: &str = "delete_result_timeout:";
pub const DELETE_QUERY_PREFIX: &str = "delete_query_";
pub const DELETE_TIMEOUT_MS: u64 = 20_000;
const MAX_REQUESTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletePhase {
    Preparing, Waiting, Querying, Unknown, Success, Failed, Partial, LegacySent,
    VerifiedAbsent, VerifiedPresent,
}

impl DeletePhase {
    pub fn busy(self) -> bool { matches!(self, Self::Preparing | Self::Waiting | Self::Querying) }
    pub fn final_result(self) -> bool {
        matches!(self, Self::Success | Self::Failed | Self::Partial | Self::LegacySent | Self::VerifiedAbsent | Self::VerifiedPresent)
    }
}

#[derive(Debug, Clone)]
pub struct DeleteRequest {
    pub id: String,
    pub session: String,
    pub target: DataTarget,
    pub phase: DeletePhase,
    pub message: String,
    pub timer_id: Option<u64>,
    pub wait_generation: u64,
    pub deadline: Option<Instant>,
}

impl DeleteRequest {
    pub fn timed_out(&self, generation: u64, now: Instant) -> bool {
        self.wait_generation == generation && self.phase.busy() && self.deadline.is_some_and(|deadline| now >= deadline)
    }
    pub fn wire_message(&self, query_session: Option<&str>) -> Value {
        let mut msg = json!({"type": if query_session.is_some() { "delete_status" } else { "delete_item" },
            "protocol":1, "requestId":self.id, "session":query_session.unwrap_or(&self.session)});
        if query_session.is_some() { msg["requestSession"] = json!(self.session); }
        match &self.target.item {
            DataItem::Comic { id, name } => { msg["kind"] = json!("comic"); msg["comicId"] = json!(id); msg["name"] = json!(name); }
            DataItem::Source { key, name, .. } => { msg["kind"] = json!("source"); msg["sourceKey"] = json!(key); msg["name"] = json!(name); }
        }
        msg
    }
}

pub struct DeleteManager {
    pub requests: VecDeque<DeleteRequest>,
    nonce: u128,
    next: u64,
}

impl Default for DeleteManager {
    fn default() -> Self {
        Self { requests: VecDeque::new(), next: 0, nonce: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) }
    }
}

impl DeleteManager {
    pub fn busy(&self) -> bool { self.requests.iter().any(|r| r.phase.busy()) }
    pub fn blocks_owner(&self, addr: &str) -> bool {
        self.busy() || self.requests.iter().any(|r| r.target.owner.addr == addr && matches!(r.phase, DeletePhase::Unknown | DeletePhase::LegacySent))
    }
    pub fn get(&self, id: &str) -> Option<&DeleteRequest> { self.requests.iter().find(|r| r.id == id) }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut DeleteRequest> { self.requests.iter_mut().find(|r| r.id == id) }
    pub fn latest_for(&self, addr: &str) -> Option<&DeleteRequest> { self.requests.iter().rev().find(|r| r.target.owner.addr == addr) }

    pub fn begin(&mut self, target: DataTarget) -> Option<String> {
        if self.blocks_owner(&target.owner.addr) { return None; }
        if self.requests.len() >= MAX_REQUESTS {
            let index = self.requests.iter().position(|r| r.phase.final_result())?;
            self.requests.remove(index);
        }
        self.next += 1;
        let id = format!("del_{}_{}", self.nonce, self.next);
        self.requests.push_back(DeleteRequest { id: id.clone(), session: String::new(), target,
            phase: DeletePhase::Preparing, message: "正在准备删除…".into(), timer_id: None, wait_generation: 0, deadline: None });
        Some(id)
    }

    pub fn accept_result(&mut self, value: &Value, source_addr: Option<&str>) -> Option<String> {
        if value.get("protocol").and_then(Value::as_u64) != Some(1) { return None; }
        let id = value.get("requestId")?.as_str()?;
        let request = self.get_mut(id)?;
        if request.phase.final_result() || request.session.is_empty() ||
            value.get("session").and_then(Value::as_str) != Some(request.session.as_str()) ||
            source_addr.is_some_and(|addr| addr != request.target.owner.addr) { return None; }
        let matches = match &request.target.item {
            DataItem::Comic { id, .. } => value.get("kind").and_then(Value::as_str) == Some("comic") && value.get("comicId").and_then(Value::as_str) == Some(id),
            DataItem::Source { key, .. } => value.get("kind").and_then(Value::as_str) == Some("source") && value.get("sourceKey").and_then(Value::as_str) == Some(key),
        };
        if !matches { return None; }
        let phase = match value.get("status")?.as_str()? {
            "processing" => return None, // keep the original bounded deadline
            "success" => {
                let files = value.get("filesState").and_then(Value::as_str);
                let files_ok = match request.target.item { DataItem::Comic { .. } => matches!(files, Some("removed" | "missing")),
                    DataItem::Source { .. } => files == Some("not_applicable") };
                if !files_ok || value.get("indexState").and_then(Value::as_str) != Some("removed") { return None; }
                DeletePhase::Success
            }
            "failed" => DeletePhase::Failed,
            "partial" => DeletePhase::Partial,
            "unknown" => DeletePhase::Unknown,
            _ => return None,
        };
        request.phase = phase;
        request.message = value.get("message").and_then(Value::as_str).unwrap_or("设备已返回删除结果").chars().take(512).collect();
        Some(id.to_string())
    }

    pub fn verify_snapshot(&mut self, addr: &str, revision: u64, comics: &[super::state::ComicInfo], sources: &[super::state::SourceInfo]) {
        for request in &mut self.requests {
            if !matches!(request.phase, DeletePhase::Unknown | DeletePhase::LegacySent) || request.target.owner.addr != addr || request.target.revision == revision { continue; }
            let present = match &request.target.item {
                DataItem::Comic { id, .. } => comics.iter().any(|c| c.id == *id),
                DataItem::Source { key, name, api_url } => {
                    if request.phase == DeletePhase::LegacySent {
                        sources.iter().any(|s| s.name == *name && s.api_url == *api_url)
                    } else {
                    if key.is_empty() || sources.iter().any(|s| s.key.is_empty()) { continue; }
                    sources.iter().any(|s| s.key == *key)
                    }
                }
            };
            request.phase = if present { DeletePhase::VerifiedPresent } else { DeletePhase::VerifiedAbsent };
            request.message = if present { "重新读取确认条目仍在，可重新尝试删除" } else { "重新读取确认设备列表已无该条目" }.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::state::{ComicInfo, SourceInfo, UiState};
    use crate::ui::data_browser::{DataDevice, DataPhase};

    fn library() -> UiState {
        let mut state = UiState::default();
        let owner = DataDevice { name: "Watch A".into(), addr: "A".into() };
        state.data_browser.begin(owner.clone());
        state.accept_library_data();
        state.data_browser.checked_device = Some(owner.clone());
        state.app_comics = vec![ComicInfo { id: "one".into(), name: "Same".into(), ..Default::default() },
            ComicInfo { id: "two".into(), name: "Same".into(), cover_base64: "cover".into(), ..Default::default() }];
        state.app_sources = vec![SourceInfo { key:"a".into(), name:"Same source".into(), api_url:"api-a".into() },
            SourceInfo { key:"b".into(), name:"Same source".into(), api_url:"api-b".into() }];
        state.app_comic_count = Some(2); state.app_source_count = Some(2);
        state.sync_comics_seen = [0, 1].into_iter().collect(); state.sync_sources_seen = [0, 1].into_iter().collect();
        state.data_browser.lists_done = true;
        state.data_browser.lists_complete = true;
        state.data_browser.phase = DataPhase::Finished;
        state.data_browser.last_complete = Some((owner, 100));
        state.data_browser.covers_received = 1;
        state.data_browser.covers_skipped = Some(1);
        state
    }
    fn request(state: &mut UiState, source: bool) -> String {
        let target = state.capture_data_target(state.data_browser.revision, 1, source).unwrap();
        let id = state.deletes.begin(target).unwrap();
        let req = state.deletes.get_mut(&id).unwrap();
        req.session = "original-hs".into(); req.phase = DeletePhase::Waiting;
        id
    }
    fn result(id: &str, source: bool) -> Value {
        let mut msg = json!({"protocol":1,"requestId":id,"session":"original-hs", "status":"success", "indexState":"removed", "message":"done"});
        if source { msg["kind"] = json!("source"); msg["sourceKey"] = json!("b"); msg["filesState"] = json!("not_applicable"); }
        else { msg["kind"] = json!("comic"); msg["comicId"] = json!("two"); msg["filesState"] = json!("removed"); }
        msg
    }

    #[test]
    fn result_requires_request_session_identity_device_and_actual_commit() {
        let mut state = library(); let id = request(&mut state, false); let value = result(&id, false);
        for (field, wrong) in [("requestId",json!("unknown")), ("session",json!("other-hs")),
            ("comicId",json!("one")), ("kind",json!("source")), ("indexState",json!("retained")), ("filesState",json!("unknown"))] {
            let mut invalid = value.clone(); invalid[field] = wrong;
            assert!(state.deletes.accept_result(&invalid, Some("A")).is_none());
        }
        assert!(state.deletes.accept_result(&value, Some("B")).is_none());
        assert!(state.deletes.accept_result(&value, Some("A")).is_some());
        assert!(state.deletes.accept_result(&value, Some("A")).is_none(), "duplicate cannot settle twice");
    }

    #[test]
    fn successful_same_name_deletion_updates_only_id_and_invalidates_old_buttons_without_advancing_sync_time() {
        let mut state = library(); state.comic_search = "Same".into(); let id = request(&mut state, false);
        let target = state.deletes.get(&id).unwrap().target.clone();
        state.deletes.accept_result(&result(&id, false), None).unwrap();
        assert!(state.apply_delete_success(&target));
        assert_eq!(state.app_comics.len(), 1); assert_eq!(state.app_comics[0].id, "one");
        assert_eq!(state.app_comic_count, Some(1)); assert_eq!(state.sync_comics_seen.len(), 1);
        assert_eq!(state.data_browser.covers_received, 0); assert_eq!(state.data_browser.covers_skipped, Some(1));
        assert_eq!(state.data_browser.complete_time(), Some(100));
        assert_eq!(state.comic_search, "Same"); assert!(!state.data_target_current(&target));
        assert!(!state.apply_delete_success(&target));
        state.update_library_completeness(); assert!(state.data_browser.lists_complete);
    }

    #[test]
    fn same_name_sources_delete_exact_key_and_query_retains_original_session() {
        let mut state = library(); let id = request(&mut state, true);
        let req = state.deletes.get(&id).unwrap().clone();
        let query = req.wire_message(Some("new-hs"));
        assert_eq!(query["type"], "delete_status"); assert_eq!(query["session"], "new-hs");
        assert_eq!(query["requestSession"], "original-hs"); assert_eq!(query["sourceKey"], "b");
        state.deletes.accept_result(&result(&id, true), None).unwrap();
        assert!(state.apply_delete_success(&req.target));
        assert_eq!(state.app_sources.len(), 1); assert_eq!(state.app_sources[0].key, "a");
        assert_eq!(state.app_source_count, Some(1));
    }

    #[test]
    fn late_result_settles_original_request_but_cannot_edit_refreshed_or_other_device_snapshots() {
        for different_device in [false, true] {
            let mut state = library(); let id = request(&mut state, false);
            let target = state.deletes.get(&id).unwrap().target.clone();
            state.data_browser.begin(DataDevice { name:"new".into(), addr:if different_device { "B" } else { "A" }.into() });
            state.accept_library_data();
            state.app_comics.push(ComicInfo { id:"two".into(), name:"replacement".into(), ..Default::default() });
            state.data_browser.lists_complete = true; state.data_browser.phase = DataPhase::Finished;
            assert!(state.deletes.accept_result(&result(&id, false), None).is_some());
            assert!(!state.apply_delete_success(&target));
            assert_eq!(state.app_comics[0].name, "replacement");
        }
    }

    #[test]
    fn failure_partial_and_unknown_preserve_rows_and_only_unknown_allows_a_late_final_result() {
        for (status, phase) in [("failed",DeletePhase::Failed), ("partial",DeletePhase::Partial), ("unknown",DeletePhase::Unknown)] {
            let mut state = library(); let id = request(&mut state, false); let mut value = result(&id, false);
            value["status"] = json!(status); value["indexState"] = json!("retained");
            state.deletes.accept_result(&value, None).unwrap();
            assert_eq!(state.app_comics.len(), 2); assert_eq!(state.deletes.get(&id).unwrap().phase, phase);
            assert_eq!(state.deletes.accept_result(&result(&id, false), None).is_some(), status == "unknown");
        }
    }

    #[test]
    fn unknown_request_is_verified_only_by_new_complete_snapshot_of_its_owner() {
        let mut state = library(); let id = request(&mut state, false);
        state.deletes.get_mut(&id).unwrap().phase = DeletePhase::Unknown;
        let revision = state.data_browser.revision;
        state.deletes.verify_snapshot("B", revision + 1, &[], &[]);
        state.deletes.verify_snapshot("A", revision, &[], &[]);
        assert_eq!(state.deletes.get(&id).unwrap().phase, DeletePhase::Unknown);
        state.deletes.verify_snapshot("A", revision + 1, &state.app_comics, &state.app_sources);
        assert_eq!(state.deletes.get(&id).unwrap().phase, DeletePhase::VerifiedPresent);
        let target = state.capture_data_target(revision, 1, false).unwrap();
        assert!(state.deletes.begin(target).is_some(), "verified existing row can be retried with a new ID");
    }

    #[test]
    fn result_wait_has_a_real_deadline_and_old_generation_cannot_expire_new_query() {
        let mut state = library(); let id = request(&mut state, false);
        let now = Instant::now(); let req = state.deletes.get_mut(&id).unwrap();
        req.wait_generation = 2;
        req.deadline = Some(now + std::time::Duration::from_secs(20));
        assert!(!req.timed_out(2, now));
        assert!(!req.timed_out(1, now + std::time::Duration::from_secs(30)));
        assert!(req.timed_out(2, now + std::time::Duration::from_secs(20)));
        req.phase = DeletePhase::Success;
        assert!(!req.timed_out(2, now + std::time::Duration::from_secs(30)));
    }
}
