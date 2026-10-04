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
pub struct ImportPlan {
    pub import_chapter_protocol: u32,
    pub book_id: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_comic_id: Option<String>,
    pub is_serial: bool,
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
    #[serde(flatten)]
    pub import: Option<ImportPlan>,
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
    import: Option<ImportPlan>,
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
        import,
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

pub fn abort_task(task_id: &str, reason: &str) {
    let mut mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(task) = mgr.tasks.get_mut(task_id) {
        if task.status != "completed" {
            task.status = "failed".to_string();
        }
    }
    if mgr.active_task_id.as_deref() == Some(task_id) {
        mgr.active_task_id = None;
    }
    let mut ustate = ui_state().write().unwrap_or_else(|p| p.into_inner());
    ustate.upload_status = StatusState::Error(reason.to_string());
    drop(ustate);
    crate::ui::build::rerender_main_ui();
}

pub fn cancel_active_task(reason: &str) {
    let mut mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(active_id) = mgr.active_task_id.take() {
        if let Some(task) = mgr.tasks.get_mut(&active_id) {
            if task.status != "completed" {
                task.status = "failed".to_string();
            }
        }
    }
    let mut ustate = ui_state().write().unwrap_or_else(|p| p.into_inner());
    ustate.upload_status = StatusState::Error(reason.to_string());
    drop(ustate);
    crate::ui::build::rerender_main_ui();
}

pub fn owns_image_request(comic_id: &str) -> bool {
    let mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    mgr.active_task_id.as_ref().and_then(|id| mgr.tasks.get(id))
        .is_some_and(|task| task.comic_id == comic_id && matches!(task.status.as_str(), "ready" | "downloading" | "waiting_result"))
}

pub fn is_busy() -> bool {
    let mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if mgr.last_progress.map(|t| t.elapsed().as_secs() > 120).unwrap_or(true) { return false; }
    mgr.active_task_id.as_ref().and_then(|id| mgr.tasks.get(id))
        .map(|task| matches!(task.status.as_str(), "ready" | "downloading" | "waiting_result")).unwrap_or(false)
}

pub fn update_progress(task_id: &str, page: usize, total: usize) {
    let mut mgr = manager().lock().unwrap_or_else(|p| p.into_inner());
    if mgr.active_task_id.as_deref() != Some(task_id) { return; }
    mgr.last_progress = Some(std::time::Instant::now());
    if let Some(task) = mgr.tasks.get_mut(task_id) {
        if task.status == "completed" || task.status == "failed" { return; }
        task.saved_pages = task.saved_pages.max(page.min(task.total_pages));
        let is_all_pages = total > 0 && task.saved_pages >= task.total_pages;
        task.status = if is_all_pages {
            "waiting_result".to_string()
        } else {
            "downloading".to_string()
        };

        let progress_str = if is_all_pages {
            "已传输全部图片，等待结果确认…".to_string()
        } else {
            format!("设备正在保存: {}/{} 页", page, total)
        };
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
    let is_active = mgr.active_task_id.as_deref() == Some(task_id);
    let (status_updated, is_success, task_name) = {
        let Some(task) = mgr.tasks.get_mut(task_id) else { return; };
        if task.status == "completed" || task.status == "failed" {
            return;
        }
        let is_success = success && saved_pages == task.total_pages && total_pages == task.total_pages;
        task.saved_pages = saved_pages;
        task.total_pages = total_pages;
        task.status = if is_success {
            "completed".to_string()
        } else {
            "failed".to_string()
        };
        (true, is_success, task.name.clone())
    };

    if status_updated && is_active {
        mgr.active_task_id = None;

        let mut ustate = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if is_success {
            ustate.upload_progress = 1.0;
            ustate.upload_status = StatusState::Success(format!(
                "🎉 《{}》已成功保存到手环！共 {} 页",
                task_name, saved_pages
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
            let task_status = get_task(task_id).map(|t| t.status).unwrap_or_else(|| "unknown".to_string());
            return Some(json_resp(200, &json!({ "code": 200, "message": "OK", "status": task_status })));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn dummy_profile() -> TaskImageProfile {
        TaskImageProfile {
            width: 100,
            quality: 75,
            if_png: false,
            if_lvgl: false,
        }
    }

    #[test]
    fn progress_updates_to_waiting_result_when_all_pages_reached() {
        let _guard = TEST_LOCK.lock().unwrap();
        let task_id = create_task(
            "comic_1".into(),
            "测试漫画".into(),
            "rev_1".into(),
            vec![TaskChapter { chapter_num: 1, title: "第1章".into(), page_count: 5 }],
            "".into(),
            dummy_profile(),
            1,
            None,
        );

        assert!(is_busy());
        assert!(owns_image_request("comic_1"));

        update_progress(&task_id, 3, 5);
        let task = get_task(&task_id).unwrap();
        assert_eq!(task.status, "downloading");
        assert_eq!(task.saved_pages, 3);

        update_progress(&task_id, 5, 5);
        let task = get_task(&task_id).unwrap();
        assert_eq!(task.status, "waiting_result");
        assert_eq!(task.saved_pages, 5);
        assert!(is_busy());
        assert!(owns_image_request("comic_1"));
    }

    #[test]
    fn finish_task_is_idempotent_and_cannot_regress_from_completed_to_failed() {
        let _guard = TEST_LOCK.lock().unwrap();
        let task_id = create_task(
            "comic_2".into(),
            "防倒退漫画".into(),
            "rev_2".into(),
            vec![TaskChapter { chapter_num: 1, title: "第1章".into(), page_count: 2 }],
            "".into(),
            dummy_profile(),
            1,
            None,
        );

        // 首次完成：成功
        finish_task(&task_id, true, 2, 2, None);
        let task = get_task(&task_id).unwrap();
        assert_eq!(task.status, "completed");
        assert!(!is_busy());

        // 重复回报：迟到的失败请求不能倒退终态
        finish_task(&task_id, false, 1, 2, Some("迟到的失败".into()));
        let task_after = get_task(&task_id).unwrap();
        assert_eq!(task_after.status, "completed");

        // route_task 处理重复回报同样返回 200 OK 且 status 为 completed
        let payload = json!({ "success": false, "savedPages": 1, "totalPages": 2 }).to_string();
        let resp = route_task("POST", &format!("/control/tasks/{}/result", task_id), payload.as_bytes()).unwrap();
        assert_eq!(resp.status, 200);
        let resp_json: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(resp_json["status"], "completed");
    }

    #[test]
    fn failed_task_stays_failed_and_clears_active() {
        let _guard = TEST_LOCK.lock().unwrap();
        let task_id = create_task(
            "comic_3".into(),
            "失败漫画".into(),
            "rev_3".into(),
            vec![TaskChapter { chapter_num: 1, title: "第1章".into(), page_count: 3 }],
            "".into(),
            dummy_profile(),
            1,
            None,
        );

        finish_task(&task_id, false, 1, 3, Some("网络中断".into()));
        let task = get_task(&task_id).unwrap();
        assert_eq!(task.status, "failed");
        assert!(!is_busy());

        // 重复失败汇报保持幂等
        finish_task(&task_id, false, 1, 3, Some("再次网络中断".into()));
        let task_after = get_task(&task_id).unwrap();
        assert_eq!(task_after.status, "failed");
    }

    #[test]
    fn abort_and_cancel_immediately_clear_busy_without_waiting() {
        let _guard = TEST_LOCK.lock().unwrap();
        let task_id = create_task(
            "comic_4".into(),
            "取消测试漫画".into(),
            "rev_4".into(),
            vec![TaskChapter { chapter_num: 1, title: "第1章".into(), page_count: 3 }],
            "".into(),
            dummy_profile(),
            1,
            None,
        );
        assert!(is_busy());

        abort_task(&task_id, "发送失败测试");
        assert!(!is_busy());
        let task = get_task(&task_id).unwrap();
        assert_eq!(task.status, "failed");

        // 测试 cancel_active_task
        let task_id_2 = create_task(
            "comic_5".into(),
            "活跃取消漫画".into(),
            "rev_5".into(),
            vec![TaskChapter { chapter_num: 1, title: "第1章".into(), page_count: 2 }],
            "".into(),
            dummy_profile(),
            1,
            None,
        );
        assert!(is_busy());
        cancel_active_task("超时清理");
        assert!(!is_busy());
        let task2 = get_task(&task_id_2).unwrap();
        assert_eq!(task2.status, "failed");
    }
}
