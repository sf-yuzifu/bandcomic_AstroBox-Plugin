//! 任务创建、通知下发与进度回报服务 (HTTP-6)
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ui::state::{ui_state, StatusState};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskChapter {
    pub chapter_num: usize,
    pub title: String,
    pub page_count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskImageProfile {
    pub width: u32,
    pub quality: u8,
    pub if_png: bool,
    pub if_lvgl: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub task_id: String,
    pub source_key: String,
    pub comic_id: String,
    pub name: String,
    pub revision: String,
    pub chapters: Vec<TaskChapter>,
    pub cover_url: String,
    pub image_profile: TaskImageProfile,
    pub status: String,
    pub saved_pages: usize,
    pub total_pages: usize,
    pub total_chapters: usize,
}

#[derive(Default)]
struct TaskManager {
    tasks: HashMap<String, TaskInfo>,
    active_task_id: Option<String>,
    last_progress: Option<std::time::Instant>,
}

static TASKS: OnceLock<Mutex<TaskManager>> = OnceLock::new();

fn manager() -> &'static Mutex<TaskManager> {
    TASKS.get_or_init(|| Mutex::new(TaskManager::default()))
}

pub fn create_task(
    comic_id: String,
    name: String,
    revision: String,
    chapters: Vec<TaskChapter>,
    cover_url: String,
    image_profile: TaskImageProfile,
    total_chapters: usize,
) -> String {
    let task_id = format!(
        "task_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    let total_pages = chapters.iter().map(|c| c.page_count).sum();

    let task = TaskInfo {
        task_id: task_id.clone(),
        source_key: "LocalUpload".to_string(),
        comic_id,
        name,
        revision,
        chapters,
        cover_url,
        image_profile,
        status: "ready".to_string(),
        saved_pages: 0,
        total_pages,
        total_chapters,
    };

    let mut mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    mgr.tasks.insert(task_id.clone(), task);
    mgr.active_task_id = Some(task_id.clone());
    mgr.last_progress = Some(std::time::Instant::now());
    task_id
}

pub fn get_task(task_id: &str) -> Option<TaskInfo> {
    let mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    mgr.tasks.get(task_id).cloned()
}

pub fn owns_image_request(comic_id: &str) -> bool {
    let mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    mgr.active_task_id.as_ref().and_then(|id| mgr.tasks.get(id))
        .is_some_and(|task| task.comic_id == comic_id && matches!(task.status.as_str(), "ready" | "downloading"))
}

pub fn is_busy() -> bool {
    let mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if mgr.last_progress.map(|t| t.elapsed().as_secs() > 120).unwrap_or(true) { return false; }
    mgr.active_task_id.as_ref().and_then(|id| mgr.tasks.get(id))
        .map(|task| task.status == "ready" || task.status == "downloading").unwrap_or(false)
}

pub fn update_progress(task_id: &str, page: usize, total: usize) {
    let mut mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if mgr.active_task_id.as_deref() != Some(task_id) { return; }
    mgr.last_progress = Some(std::time::Instant::now());
    if let Some(task) = mgr.tasks.get_mut(task_id) {
        if task.status == "completed" || task.status == "failed" { return; }
        task.saved_pages = task.saved_pages.max(page.min(task.total_pages));
        task.status = "downloading".to_string();

        let progress_str = format!("设备正在保存: {}/{} 页", page, total);
        let mut ustate = ui_state().write().unwrap_or_else(|p| p.into_inner());
        ustate.upload_progress = if total > 0 {
            page as f32 / total as f32
        } else {
            0.0
        };
        ustate.upload_status = StatusState::Processing(progress_str);
    }
    crate::ui::build::rerender_main_ui();
}

pub fn finish_task(
    task_id: &str,
    success: bool,
    saved_pages: usize,
    total_pages: usize,
    error: Option<String>,
) {
    let mut mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if mgr.active_task_id.as_deref() != Some(task_id) { return; }
    if let Some(task) = mgr.tasks.get_mut(task_id) {
        if task.status == "completed" || task.status == "failed" { return; }
        let success = success && saved_pages == task.total_pages && total_pages == task.total_pages;
        task.saved_pages = saved_pages;
        task.total_pages = total_pages;
        task.status = if success {
            "completed".to_string()
        } else {
            "failed".to_string()
        };

        let mut ustate = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if success {
            ustate.upload_progress = 1.0;
            ustate.upload_status = StatusState::Success(format!(
                "🎉 《{}》已成功保存到手环！共 {} 页",
                task.name, saved_pages
            ));
        } else {
            let err_msg = error.unwrap_or_else(|| "下载异常中断".to_string());
            ustate.upload_status = StatusState::Error(format!("导入失败：{}", err_msg));
        }
    }
    crate::ui::build::rerender_main_ui();
}

fn json_resp(status: u16, val: &serde_json::Value) -> crate::http_probe::ProbeResponse {
    let body = val.to_string().into_bytes();
    crate::http_probe::ProbeResponse {
        status,
        headers: vec![
            ("Content-Type".into(), "application/json; charset=utf-8".into()),
            ("Content-Length".into(), body.len().to_string()),
            ("Cache-Control".into(), "no-store".into()),
        ],
        body,
    }
}

pub fn route_task(method: &str, path: &str, body: &[u8]) -> Option<crate::http_probe::ProbeResponse> {
    if !path.starts_with("/control/tasks/") {
        return None;
    }
    let rest = &path["/control/tasks/".len()..];

    if method == "GET" && !rest.contains('/') {
        let task_id = rest;
        let task = get_task(task_id);
        let Some(task) = task else {
            return Some(json_resp(404, &json!({ "code": 404, "message": "Task not found" })));
        };
        return Some(json_resp(200, &serde_json::to_value(task).unwrap()));
    }

    if method == "POST" {
        if let Some(task_id) = rest.strip_suffix("/progress") {
            let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let page = parsed.get("page").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let total = parsed.get("total").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            update_progress(task_id, page, total);
            return Some(json_resp(200, &json!({ "code": 200, "message": "OK" })));
        } else if let Some(task_id) = rest.strip_suffix("/result") {
            let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let success = parsed.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            let saved_pages = parsed.get("savedPages").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let total_pages = parsed.get("totalPages").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let error = parsed.get("error").and_then(|v| v.as_str()).map(str::to_string);
            finish_task(task_id, success, saved_pages, total_pages, error);
            return Some(json_resp(200, &json!({ "code": 200, "message": "OK" })));
        }
    }

    None
}
