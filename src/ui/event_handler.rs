use super::COMIC_DATA_CARD_ID;
use super::handshake;
use super::state::*;
use crate::astrobox::psys_host_v4::{self as psys_host, device, dialog, interconnect, timer};
use crate::network::fetch_source_catalog;
use crate::source_config::{CatalogPhase, CookieAction, SourceSync, SyncPhase};
use crate::transfer::{RecvFrontier, WindowedSender};
use crate::sync_receive::{CoverChunks, MAX_COVER_CHUNKS};
use std::time::Instant;
use super::data_browser::DataPhase;
use super::deletion::{DeletePhase, DELETE_QUERY_PREFIX, DELETE_TIMEOUT_EVENT, DELETE_TIMEOUT_MS};
use crate::image_processor::{ImageRole, PreparedImage};
use serde_json::{Value, json};

use super::build::{self, build_main_ui};
use super::message::{hide_status, show_status};

pub async fn ui_event_processor(
    event_type: psys_host::ui::Event,
    event_id: &str,
    event_payload: &str,
) {
    tracing::debug!(
        "UI 事件: id={}, type={:?}",
        event_id,
        event_type
    );

    // Pending picks and active uploads own the draft until they finish.
    // Navigation stays available to return to the result overview.
    let edits_upload = matches!(event_id,
        UPLOAD_NAME_INPUT_EVENT | UPLOAD_MODE_SINGLE_EVENT | UPLOAD_MODE_MULTI_EVENT |
        UPLOAD_PICK_FILES_EVENT | UPLOAD_PICK_COVER_EVENT | UPLOAD_PICK_MULTI_COVER_EVENT |
        UPLOAD_START_EVENT | UPLOAD_CLEAR_EVENT | UPLOAD_ADD_CHAPTER_EVENT |
        UPLOAD_COVER_FIRST_EVENT | UPLOAD_COVER_NONE_EVENT)
        || [UPLOAD_MOVE_UP_PREFIX, UPLOAD_MOVE_DOWN_PREFIX, UPLOAD_DELETE_PREFIX,
            CHAPTER_PICK_FILES_PREFIX, CHAPTER_UPLOAD_PREFIX, CHAPTER_CLEAR_PREFIX,
            CHAPTER_DELETE_PREFIX, CHAPTER_MOVE_UP_PREFIX, CHAPTER_MOVE_DOWN_PREFIX,
            CHAPTER_DEL_FILE_PREFIX, CHAPTER_NAME_INPUT_PREFIX]
            .iter().any(|prefix| event_id.starts_with(prefix));
    if edits_upload && ui_state().read().unwrap_or_else(|p| p.into_inner()).upload_locked() {
        return;
    }
    let starts_device_operation = matches!(event_id, FETCH_APP_DATA_EVENT | SYNC_BUTTON_EVENT | UPLOAD_START_EVENT | crate::http_server::BIND_EVENT)
        || [SOURCE_SYNC_PREFIX, CHAPTER_UPLOAD_PREFIX, DELETE_COMIC_PREFIX, DELETE_SOURCE_PREFIX, DELETE_QUERY_PREFIX].iter().any(|p| event_id.starts_with(p));
    if starts_device_operation && ui_state().read().unwrap_or_else(|p| p.into_inner()).source_sync_busy() { return; }
    if (matches!(event_id, FETCH_APP_DATA_EVENT | SYNC_BUTTON_EVENT | UPLOAD_START_EVENT | crate::http_server::BIND_EVENT) || event_id.starts_with(SOURCE_SYNC_PREFIX) || event_id.starts_with(CHAPTER_UPLOAD_PREFIX)) &&
        ui_state().read().unwrap_or_else(|p| p.into_inner()).deletes.requests.iter()
            .any(|r| matches!(r.phase, DeletePhase::Preparing | DeletePhase::Querying)) { return; }

    match event_id {
        COMIC_SEARCH_EVENT | SOURCE_SEARCH_EVENT | COMIC_SEARCH_CLEAR_EVENT | SOURCE_SEARCH_CLEAR_EVENT => {
            let clear = matches!(event_id, COMIC_SEARCH_CLEAR_EVENT | SOURCE_SEARCH_CLEAR_EVENT);
            let value = if clear { Some(String::new()) } else {
                serde_json::from_str::<Value>(event_payload).ok()
                    .and_then(|value| value.get("value").and_then(Value::as_str).map(str::to_string))
            };
            if let Some(value) = value {
                let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
                if matches!(event_id, COMIC_SEARCH_EVENT | COMIC_SEARCH_CLEAR_EVENT) {
                    state.comic_search = value;
                    state.comic_page_cursor = 0;
                } else {
                    state.source_search = value;
                    state.source_page_cursor = 0;
                }
                drop(state);
                build::rerender_main_ui();
            }
        }
        UPLOAD_OVERVIEW_EVENT => open_upload_view(UploadView::Overview),
        UPLOAD_INFO_EVENT => open_upload_view(UploadView::Info),
        UPLOAD_PAGES_EVENT => open_upload_view(UploadView::Pages(None)),
        UPLOAD_COVER_EVENT => open_upload_view(UploadView::Cover),
        UPLOAD_CONNECTION_EVENT => open_upload_view(UploadView::Connection),
        UPLOAD_COVER_FIRST_EVENT | UPLOAD_COVER_NONE_EVENT => {
            let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
            let follows = event_id == UPLOAD_COVER_FIRST_EVENT;
            if state.upload_mode == UploadMode::Single {
                state.single_cover_follows_first = follows;
                if !follows {
                    if let Some(item) = state.upload_items.first_mut() { item.cover = None; }
                }
            } else {
                state.multi_cover_follows_first = follows;
                if !follows { state.multi_cover = None; }
            }
            state.refresh_auto_covers();
            drop(state);
            rerender_upload_ui();
        }
        UPLOAD_PAGE_PREV_EVENT | UPLOAD_PAGE_NEXT_EVENT |
        COMIC_PAGE_PREV_EVENT | COMIC_PAGE_NEXT_EVENT |
        SOURCE_PAGE_PREV_EVENT | SOURCE_PAGE_NEXT_EVENT => {
            let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
            let (cursor, total, forward) = match event_id {
                COMIC_PAGE_PREV_EVENT | COMIC_PAGE_NEXT_EVENT =>
                    (state.comic_page_cursor, state.comic_matches().len(), event_id == COMIC_PAGE_NEXT_EVENT),
                SOURCE_PAGE_PREV_EVENT | SOURCE_PAGE_NEXT_EVENT =>
                    (state.source_page_cursor, state.source_matches().len(), event_id == SOURCE_PAGE_NEXT_EVENT),
                _ => {
                    let total = match state.upload_view {
                        UploadView::Overview if state.upload_mode == UploadMode::Multi => state.upload_chapters.len(),
                        UploadView::Pages(Some(index)) => state.upload_chapters.get(index).map(|c| c.files.len()).unwrap_or(0),
                        _ => state.upload_items.iter().map(|i| i.files.len()).sum(),
                    };
                    (state.upload_page_cursor, total, event_id == UPLOAD_PAGE_NEXT_EVENT)
                }
            };
            let current = page_window(cursor, total).0 / PAGE_WINDOW;
            let next = if forward { (current + 1).min(total.saturating_sub(1) / PAGE_WINDOW) }
                else { current.saturating_sub(1) };
            match event_id {
                COMIC_PAGE_PREV_EVENT | COMIC_PAGE_NEXT_EVENT => state.comic_page_cursor = next,
                SOURCE_PAGE_PREV_EVENT | SOURCE_PAGE_NEXT_EVENT => state.source_page_cursor = next,
                _ => state.upload_page_cursor = next,
            }
            drop(state);
            rerender_upload_ui();
        }
        crate::http_server::START_EVENT => crate::http_server::start().await,
        crate::http_server::STOP_EVENT => crate::http_server::stop().await,
        crate::http_server::IP_INPUT_EVENT => {
            if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    crate::http_server::edit_fallback_ip(text.to_string());
                }
            }
        }
        crate::http_server::IP_SAVE_EVENT => {
            if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    crate::http_server::update_fallback_ip(text.to_string());
                }
            }
        }
        crate::http_server::SETTINGS_EVENT => crate::http_server::toggle_settings(),
        crate::http_server::BIND_EVENT => {
            crate::http_server::start().await;
            let _ = crate::http_server::bind_device().await;
        }
        DOMAIN_INPUT_CHANGE_EVENT => {
            if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    tracing::debug!("域名输入变化: {}", text);
                    update_domain_state(text.to_string());
                }
            }
        }
        DOMAIN_INPUT_BLUR_EVENT => {
            if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    tracing::info!("域名输入框失去焦点，开始获取配置: {}", text);
                    handle_domain_blur(text.to_string()).await;
                }
            }
        }
        SOURCE_FETCH_EVENT => load_source_config(true).await,
        SOURCE_CATALOG_PREV | SOURCE_CATALOG_NEXT => {
            let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
            let form = &mut state.source_form;
            let total = form.catalog.as_ref().map_or(0, |c| c.entries.len());
            let last = total.saturating_sub(1) / PAGE_WINDOW;
            form.page = if event_id == SOURCE_CATALOG_NEXT { (form.page + 1).min(last) } else { form.page.saturating_sub(1) };
            drop(state); build::rerender_main_ui();
        }
        HIDE_STATUS_EVENT => {
            hide_status();
        }
        HIDE_APP_DATA_STATUS_EVENT => {
            hide_app_data_status();
        }
        HIDE_UPLOAD_STATUS_EVENT => {
            hide_upload_status();
        }
        UPLOAD_NAME_INPUT_EVENT => {
            if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.upload_comic_name_input = text.to_string();
                }
            }
        }
        UPLOAD_MODE_SINGLE_EVENT => {
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.upload_mode = UploadMode::Single;
                    state.upload_page_cursor = 0;
            }
            switch_tab(TabPage::Upload);
        }
        UPLOAD_MODE_MULTI_EVENT => {
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.upload_mode = UploadMode::Multi;
                    state.upload_page_cursor = 0;
            }
            switch_tab(TabPage::Upload);
        }
        UPLOAD_PICK_FILES_EVENT => {
            tracing::info!("选择文件按钮被点击");
            pick_and_stash(PickTarget::UploadItem, true).await;
        }
        UPLOAD_START_EVENT => {
            tracing::info!("上传按钮被点击");
            handle_upload_start().await;
        }
        UPLOAD_CLEAR_EVENT => {
            tracing::info!("清空列表按钮被点击");
            if confirm_clear("清空整理内容", "清空当前书名、封面和图片列表。设备上已保存的漫画不受影响。").await {
                handle_upload_clear();
            }
        }
        UPLOAD_ADD_CHAPTER_EVENT => {
            tracing::info!("添加章节按钮被点击");
            handle_add_chapter();
        }
        UPLOAD_PICK_COVER_EVENT => {
            tracing::info!("封面选择按钮被点击");
            pick_and_stash(PickTarget::CoverSingle, false).await;
        }
        UPLOAD_PICK_MULTI_COVER_EVENT => {
            tracing::info!("多章节封面选择按钮被点击");
            pick_and_stash(PickTarget::CoverMulti, false).await;
        }
        TAB_SYNC_EVENT => {
            switch_tab(TabPage::Sync);
        }
        TAB_DATA_EVENT => {
            refresh_data_connection().await;
            switch_tab(TabPage::Data);
        }
        TAB_UPLOAD_EVENT => {
            switch_tab(TabPage::Upload);
        }
        FETCH_APP_DATA_EVENT => {
            tracing::info!("获取快应用数据按钮被点击");
            handle_fetch_app_data().await;
        }
        _ => {
            if let Some(generation) = event_id.strip_prefix(SOURCE_SYNC_PREFIX).and_then(|s|s.parse::<u64>().ok()) {
                let current = ui_state().read().unwrap_or_else(|p|p.into_inner()).source_form.generation == generation;
                if current { handle_sync().await; }
            } else if handle_source_form_event(event_id, event_payload) {
                build::rerender_main_ui();
            } else if let Some(id) = event_id.strip_prefix(DELETE_QUERY_PREFIX) {
                query_delete_result(id).await;
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_EDIT_PREFIX) {
                if let Ok(index) = index_str.parse::<usize>() {
                    open_upload_view(UploadView::Pages(Some(index)));
                }
            } else if let Some(index_str) = event_id.strip_prefix(UPLOAD_MOVE_UP_PREFIX) {
                if let Ok(index) = index_str.parse::<usize>() {
                    handle_upload_move(index, -1);
                }
            } else if let Some(index_str) = event_id.strip_prefix(UPLOAD_MOVE_DOWN_PREFIX) {
                if let Ok(index) = index_str.parse::<usize>() {
                    handle_upload_move(index, 1);
                }
            } else if let Some(index_str) = event_id.strip_prefix(UPLOAD_DELETE_PREFIX) {
                if let Ok(index) = index_str.parse::<usize>() {
                    handle_upload_delete(index);
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_PICK_FILES_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    tracing::info!("章节{}选择文件", chapter_index);
                    pick_and_stash(PickTarget::Chapter(chapter_index), true).await;
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_UPLOAD_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    tracing::info!("章节{}上传", chapter_index);
                    handle_chapter_upload(chapter_index).await;
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_CLEAR_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    handle_chapter_clear(chapter_index);
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_DELETE_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    if confirm_clear("删除章节", "移除这一章的整理内容，其他章节和设备上已保存的内容保留。").await {
                        handle_chapter_delete_chapter(chapter_index);
                    }
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_MOVE_UP_PREFIX) {
                // format: chapter_move_up_{chapter_index}_{file_index}
                if let Some((ci_str, fi_str)) = index_str.split_once('_') {
                    if let (Ok(ci), Ok(fi)) = (ci_str.parse::<usize>(), fi_str.parse::<usize>()) {
                        handle_chapter_move_file(ci, fi, -1);
                    }
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_MOVE_DOWN_PREFIX) {
                if let Some((ci_str, fi_str)) = index_str.split_once('_') {
                    if let (Ok(ci), Ok(fi)) = (ci_str.parse::<usize>(), fi_str.parse::<usize>()) {
                        handle_chapter_move_file(ci, fi, 1);
                    }
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_DEL_FILE_PREFIX) {
                if let Some((ci_str, fi_str)) = index_str.split_once('_') {
                    if let (Ok(ci), Ok(fi)) = (ci_str.parse::<usize>(), fi_str.parse::<usize>()) {
                        handle_chapter_del_file(ci, fi);
                    }
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_NAME_INPUT_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                        if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                            handle_chapter_name_input(chapter_index, text.to_string());
                        }
                    }
                }
            } else if let Some(index_str) = event_id.strip_prefix(DELETE_COMIC_PREFIX) {
                if let Some((revision, index)) = parse_data_action(index_str) {
                    handle_delete_data(revision, index, false).await;
                }
            } else if let Some(index_str) = event_id.strip_prefix(DELETE_SOURCE_PREFIX) {
                if let Some((revision, index)) = parse_data_action(index_str) {
                    handle_delete_data(revision, index, true).await;
                }
            }
        }
    }
}

fn open_upload_view(view: UploadView) {
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    state.upload_view = view;
    state.upload_page_cursor = 0;
    drop(state);
    rerender_upload_ui();
}

async fn confirm_clear(title: &str, content: &str) -> bool {
    dialog::show_dialog(dialog::DialogType::Alert, dialog::DialogStyle::Website, dialog::DialogInfo {
        title: title.into(), content: content.into(), buttons: vec![
            dialog::DialogButton { id: "cancel".into(), primary: false, content: "取消".into() },
            dialog::DialogButton { id: "confirm".into(), primary: true, content: "确认".into() },
        ],
    }).await.clicked_btn_id == "confirm"
}

fn switch_tab(tab: TabPage) {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.current_tab = tab;
        root_id = state.root_element_id.clone();
    }
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

pub fn hide_upload_status() {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_status = StatusState::Default;
        state.upload_status_timer_id = None;
        root_id = state.root_element_id.clone();
    }
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

async fn show_upload_status(status: StatusState) {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(timer_id) = state.upload_status_timer_id {
            timer::clear_timer(timer_id);
        }

        state.upload_status = status.clone();

        // Keep results visible until the next action.
        state.upload_status_timer_id = None;

        root_id = state.root_element_id.clone();
    }

    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

fn rerender_upload_ui() {
    let root_id = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.root_element_id.clone()
    };

    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

/// 选图统一入口：事件内只等对话框，暂存原图后注册 30ms 定时器就返回。
/// 原生对话框关闭后宿主会立即拉取页面重渲染（on_ui_render）；若此时事件循环
/// 仍在跑解码/缩放这种数秒级 CPU 密集操作，宿主与插件互相等待就会触发
/// wasmtime "event loop cannot make further progress" 死锁。故解码缩放推迟到
/// PICK_PROCESS_EVENT 定时器事件里做（handle_pick_process），让宿主的重渲染
/// 在事件循环空转窗口内完成。
async fn pick_and_stash(target: PickTarget, multiple: bool) {
    let extensions = vec![
        "jpg".to_string(),
        "jpeg".to_string(),
        "png".to_string(),
        "webp".to_string(),
        "bmp".to_string(),
        "gif".to_string(),
    ];

    let filter = psys_host::dialog::FilterConfig {
        multiple,
        extensions,
        default_directory: String::new(),
        default_file_name: String::new(),
    };

    let pick_config = psys_host::dialog::PickConfig {
        read: true,
        copy_to: None,
    };

    let result = match psys_host::dialog::pick_file(pick_config, filter).await {
        Ok(result) => result,
        Err(reason) => {
            tracing::error!("选择图片失败: {}", reason);
            show_upload_status(StatusState::Error(format!("选择图片失败：{}", reason))).await;
            return;
        }
    };

    if result.data.is_empty() {
        tracing::info!("用户取消了文件选择");
        return;
    }

    show_upload_status(StatusState::Processing("正在整理选中的图片…".into())).await;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pending_pick = Some(PendingPick {
            name: result.name,
            data: result.data,
            target,
        });
    }

    timer::set_timeout(30, PICK_PROCESS_EVENT);
}

/// 定时器事件里真正处理选中的图片：解码/缩放/入列/重渲染
pub fn handle_pick_process() {
    let pick = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pending_pick.take()
    };
    let pick = match pick {
        Some(p) => p,
        None => return,
    };

    let (thumbnail, master) = match crate::image_processor::prepare_picked_image(&pick.data) {
        Ok(images) => images,
        Err(error) => {
            ui_state().write().unwrap_or_else(|p| p.into_inner()).upload_status =
                StatusState::Error(format!("选图《{}》：{}", pick.name, error));
            rerender_upload_ui();
            return;
        }
    };
    let master_len = master.len();
    let original_len = pick.data.len();

    // HTTP-4: 将母版保存到磁盘，内存中释放大尺寸图片缓冲
    let (disk_path, data) = match crate::assets::save_master(&master) {
        Ok(path) => (Some(path), Vec::new()),
        Err(e) => {
            tracing::warn!("母版落盘失败，回退到内存存储: {}", e);
            (None, master)
        }
    };

    let file = UploadFile {
        name: pick.name.clone(),
        disk_path,
        data,
        size: master_len,
        original_size: original_len,
        thumbnail,
    };

    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match pick.target {
            PickTarget::UploadItem => {
                let comic_name = if state.upload_comic_name_input.trim().is_empty() {
                    pick.name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&pick.name).to_string()
                } else {
                    state.upload_comic_name_input.trim().to_string()
                };
                match state.upload_mode {
                    UploadMode::Single => {
                        if state.upload_comic_name_input.trim().is_empty() {
                            state.upload_comic_name_input = comic_name.clone();
                        }
                        // Keep one editable book even after changing its name.
                        // An automatic cover references the first body page.
                        if let Some(item) = state.upload_items.first_mut() {
                            item.files.push(file);
                        } else {
                            state.upload_items.push(UploadItem { comic_name, cover: None, files: vec![file] });
                        }
                    }
                    UploadMode::Multi => {
                        // 多章模式用章节选图；万一触发到通用选图，归入最后一个章节
                        if state.upload_chapters.is_empty() {
                            state.upload_chapters.push(ChapterItem::default());
                        }
                        let last = state.upload_chapters.last_mut().unwrap();
                        if !last.files.iter().any(|f| f.name == file.name) {
                            last.files.push(file);
                        }
                    }
                }
            }
            PickTarget::Chapter(chapter_index) => {
                if chapter_index < state.upload_chapters.len() {
                    let chapter = &mut state.upload_chapters[chapter_index];
                    if !chapter.files.iter().any(|f| f.name == file.name) {
                        chapter.files.push(file);
                    }
                }
            }
            PickTarget::CoverSingle => {
                state.single_cover_follows_first = false;
                if let Some(first) = state.upload_items.first_mut() {
                    first.cover = Some(file);
                } else {
                    let name = state.upload_comic_name_input.clone();
                    state.upload_items.push(UploadItem { comic_name: name, cover: Some(file), files: vec![] });
                }
            }
            PickTarget::CoverMulti => {
                state.multi_cover_follows_first = false;
                state.multi_cover = Some(file);
            }
        }
        state.refresh_auto_covers();
        state.upload_status = StatusState::Default;
    }

    rerender_upload_ui();
}

fn handle_upload_move(index: usize, direction: i32) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.move_single_page(index, direction);
    }

    rerender_upload_ui();
}

fn handle_upload_delete(index: usize) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.delete_single_page(index);
    }

    rerender_upload_ui();
}

fn handle_upload_clear() {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_items.clear();
        state.upload_chapters.clear();
        state.multi_cover = None;
        state.upload_progress = 0.0;
        state.upload_current_file = String::new();
        state.upload_status = StatusState::Default;
        state.image_notices.clear();
        state.upload_comic_name_input.clear();
        state.single_cover_follows_first = true;
        state.multi_cover_follows_first = true;
        state.upload_view = UploadView::Overview;
        state.upload_page_cursor = 0;
    }

    rerender_upload_ui();
}

fn handle_add_chapter() {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_chapters.push(ChapterItem::default());
        state.upload_view = UploadView::Pages(Some(state.upload_chapters.len() - 1));
        state.upload_page_cursor = 0;
    }
    rerender_upload_ui();
}

fn handle_chapter_name_input(chapter_index: usize, value: String) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if chapter_index < state.upload_chapters.len() {
            state.upload_chapters[chapter_index].name = value;
        }
    }
}

fn handle_chapter_clear(chapter_index: usize) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if chapter_index < state.upload_chapters.len() {
            state.upload_chapters[chapter_index].files.clear();
            state.refresh_auto_covers();
        }
    }
    rerender_upload_ui();
}

fn handle_chapter_delete_chapter(chapter_index: usize) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if chapter_index < state.upload_chapters.len() {
            state.upload_chapters.remove(chapter_index);
            state.refresh_auto_covers();
            state.upload_view = UploadView::Overview;
            state.upload_page_cursor = 0;
        }
    }
    rerender_upload_ui();
}

fn handle_chapter_move_file(chapter_index: usize, file_index: usize, direction: i32) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if chapter_index >= state.upload_chapters.len() {
            return;
        }

        let chapter = &mut state.upload_chapters[chapter_index];
        let new_index = (file_index as i32 + direction) as usize;
        if new_index >= chapter.files.len() {
            return;
        }

        chapter.files.swap(file_index, new_index);
        state.refresh_auto_covers();
    }
    rerender_upload_ui();
}

fn handle_chapter_del_file(chapter_index: usize, file_index: usize) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if chapter_index >= state.upload_chapters.len() {
            return;
        }

        let chapter = &mut state.upload_chapters[chapter_index];
        if file_index >= chapter.files.len() {
            return;
        }
        chapter.files.remove(file_index);
        state.refresh_auto_covers();
    }
    rerender_upload_ui();
}

async fn handle_chapter_upload(chapter_index: usize) {
    if crate::jobs::is_busy() || PENDING_HTTP_IMPORT.lock().unwrap_or_else(|p| p.into_inner()).is_some() { return; }
    // Reuse the same connection flow
    let comic_name;
    let chapter_data;

    {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if chapter_index >= state.upload_chapters.len() {
            return;
        }
        let chapter = &state.upload_chapters[chapter_index];
        if chapter.files.is_empty() {
            drop(state);
            show_upload_status(StatusState::Error("该章节没有图片。".to_string())).await;
            return;
        }
        let main_name = if state.upload_comic_name_input.trim().is_empty() {
            "本地漫画".to_string()
        } else {
            state.upload_comic_name_input.trim().to_string()
        };
        let ch_name = if chapter.name.trim().is_empty() {
            format!("第{}章", chapter_index + 1)
        } else {
            chapter.name.clone()
        };
        comic_name = format!("{} - {}", main_name, ch_name);
        chapter_data = (ch_name, chapter.files.clone());
    }

    open_upload_view(UploadView::Overview);
    show_upload_status(StatusState::Processing("正在连接设备上的腕上漫画…".to_string())).await;

    let progress = upload_progress();
    let device_addr = match handshake::prepare_launch(handshake::MIN_UPLOAD_VERSION, &progress).await {
        Ok(addr) => addr,
        Err(msg) => {
            show_upload_status(StatusState::Error(msg)).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成后在 on_done 回调里继续上传流程
    handshake::begin_wait(
        device_addr.clone(),
        upload_progress(),
        move |result| async move { match result {
            Err(msg) => {
                show_upload_status(StatusState::Error(msg)).await;
            }
            Ok(settings_opt) => {
                let http_capable = {
                    let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
                    state.watch_http_import
                };
                if http_capable {
                    prepare_http_import(device_addr, settings_opt, Some(chapter_index)).await;
                } else {
                    chapter_upload_continue(
                        comic_name,
                        chapter_data,
                        device_addr,
                    ).await;
                }
            }
        } },
    );
}

type PendingHttpImport = (String, Option<WatchSettings>, Option<usize>);
static PENDING_HTTP_IMPORT: std::sync::Mutex<Option<PendingHttpImport>> = std::sync::Mutex::new(None);

pub async fn http_bind_timeout() {
    let had_pending = PENDING_HTTP_IMPORT.lock().unwrap_or_else(|p| p.into_inner()).take().is_some();
    if had_pending {
        show_upload_status(StatusState::Error("设备连接超时，请检查 AstroBox 连接或设置备用 IPv4 后重试".into())).await;
    }
}

async fn prepare_http_import(device_addr: String, settings: Option<WatchSettings>, chapter: Option<usize>) {
    *PENDING_HTTP_IMPORT.lock().unwrap_or_else(|p| p.into_inner()) = Some((device_addr.clone(), settings, chapter));
    crate::http_server::start().await;
    if let Err(error) = crate::http_server::bind_connected_device(device_addr).await {
        PENDING_HTTP_IMPORT.lock().unwrap_or_else(|p| p.into_inner()).take();
        show_upload_status(StatusState::Error(format!("本地连接失败：{}", error))).await;
    }
}

async fn start_http_import_task(device_addr: String, settings_opt: Option<WatchSettings>, chapter_index: Option<usize>) {
    let watch_settings = settings_opt.unwrap_or_else(current_watch_settings);
    let catalog = crate::local_source::get_catalog();
    if catalog.is_empty() {
        show_upload_status(StatusState::Error("无可导入的本地漫画内容".to_string())).await;
        return;
    }

    let connection = crate::http_server::status();
    let actual_endpoint = match connection.endpoint.filter(|_| connection.bound) {
        Some(endpoint) => endpoint,
        None => {
            show_upload_status(StatusState::Error("本地 HTTP 连接已失效，请重新上传".into())).await;
            return;
        }
    };
    let comic = crate::local_source::publish(catalog.into_iter().next().unwrap());

    let chapters = comic
        .chapters
        .iter()
        .filter(|c| chapter_index.map(|i| c.chapter_number == i + 1).unwrap_or(true))
        .map(|c| crate::jobs::TaskChapter {
            chapter_num: c.chapter_number,
            title: c.title.clone(),
            page_count: c.pages.len(),
        })
        .collect();

    let cover_url = if comic.cover.is_some() {
        format!("{}/local/album/{}/cover", actual_endpoint, comic.id)
    } else {
        String::new()
    };

    let image_profile = crate::jobs::TaskImageProfile {
        width: watch_settings.image_size.clamp(10, 4096),
        quality: watch_settings.image_quality.clamp(1, 100) as u8,
        if_png: watch_settings.image_use_png,
        if_lvgl: watch_settings.image_pre_transcode,
    };

    let task_id = crate::jobs::create_task(
        comic.id.clone(),
        comic.name.clone(),
        comic.revision.clone(),
        chapters,
        cover_url,
        image_profile,
        comic.chapters.iter().map(|c| c.chapter_number).max().unwrap_or(1),
    );

    let session = crate::http_server::status()
        .bind_status
        .unwrap_or_default();

    let msg = json!({
        "type": "import_http_task",
        "taskId": task_id,
        "session": session,
        "endpoint": actual_endpoint,
    })
    .to_string();

    show_upload_status(StatusState::Processing(format!(
        "任务已发送，等待设备保存《{}》…",
        comic.name
    )))
    .await;

    if let Err(e) =
        interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), msg).await
    {
        tracing::error!("发送 import_http_task 失败: {:?}", e);
        show_upload_status(StatusState::Error("发送任务失败，请检查连接。".to_string())).await;
    }
}

async fn chapter_upload_continue(
    comic_name: String,
    chapter_data: (String, Vec<UploadFile>),
    device_addr: String,
) {
    reset_upload_progress();
    let watch_settings = current_watch_settings();

    let (_ch_name, files) = chapter_data;
    let total = files.len();
    let page_count = files.len();

    let mut all_files: Vec<(String, String)> = Vec::new();
    let mut file_names: Vec<String> = Vec::new();

    // Process pages
    let mut page_num: u32 = 0;
    for (fi, file) in files.iter().enumerate() {
        page_num += 1;
        show_upload_status(StatusState::Processing(format!(
            "正在处理 {}/{}",
            fi + 1,
            total
        )))
        .await;

        let Some(product) = prepare_upload_image(file, &watch_settings, ImageRole::Page,
            &format!("单章《{}》第{}页", comic_name, page_num)).await else { return; };
        let name = product.format.page_name(page_num);
        let b64 = base64_encode(&product.bytes);
        file_names.push(name.clone());
        all_files.push((name, b64));
    }

    let mut header: Value = json!({
        "type": "import_comic_header",
        "name": comic_name,
        "mode": "single",
        "files": file_names,
        "page_count": page_count,
        "is_serial": true,
    });

    let mut chunked_files: Vec<(String, Vec<String>)> = Vec::with_capacity(all_files.len());
    for (file_key, b64_data) in all_files {
        let chunks: Vec<String> = b64_data
            .as_bytes()
            .chunks(CHUNK_SIZE)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        chunked_files.push((file_key, chunks));
    }

    // 窗口模式：按快应用协商能力构建滑窗会话；旧快应用无能力 → None 走逐片停等
    let windowed = build_windowed_upload(&chunked_files);
    if let Some(ref w) = windowed {
        // 窗口会话标记 + 全局总片数；旧快应用忽略未知字段天然兼容
        header["wchunks"] = json!(w.order.len());
    }

    let header_str = match serde_json::to_string(&header) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化头部消息失败: {}", e);
            show_upload_status(StatusState::Error("数据序列化失败。".to_string())).await;
            return;
        }
    };

    let total_files = chunked_files.len();
    let first_file_key = chunked_files[0].0.clone();
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_session = Some(UploadSession {
            device_addr: device_addr.clone(),
            comic_name: comic_name.clone(),
            all_files: chunked_files,
            current_file: 0,
            current_chunk: 0,
            total_files,
            awaiting: None,
            retry_count: 0,
            header_str: header_str.clone(),
            header_acked: false,
            header_retry: 0,
            windowed,
        });
        state.upload_current_file = first_file_key;
    }

    // 先发头部并等快应用确认（import_header_ack）后再发分片，
    // 防止安卓端乱序导致分片先于头部到达被丢弃
    show_upload_status(StatusState::Processing("正在发送数据...".to_string())).await;
    send_import_header(&device_addr, &header_str).await;
}

fn reset_upload_progress() {
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.upload_progress = 0.0;
    state.upload_current_file = String::new();
    state.image_notices.clear();
}

async fn handle_upload_start() {
    if crate::jobs::is_busy() || PENDING_HTTP_IMPORT.lock().unwrap_or_else(|p| p.into_inner()).is_some() { return; }
    reset_upload_progress();
    let empty = ui_state().read().unwrap_or_else(|p| p.into_inner()).page_count() == 0;
    if empty {
        show_upload_status(StatusState::Error("请先添加至少一页正文，封面不计入正文。".into())).await;
        return;
    }
    open_upload_view(UploadView::Overview);

    let upload_mode;
    let (is_single, items, chapters);

    {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        upload_mode = state.upload_mode.clone();
        is_single = upload_mode == UploadMode::Single;
        items = state.upload_items.clone();
        chapters = state.upload_chapters.clone();
    } // release read lock

    if is_single && items.is_empty() {
        show_upload_status(StatusState::Error("请先选择漫画文件。".to_string())).await;
        return;
    }
    if !is_single && chapters.is_empty() {
        show_upload_status(StatusState::Error("请先添加章节。".to_string())).await;
        return;
    }
    if !is_single {
        let has_any_files = chapters.iter().any(|c| !c.files.is_empty());
        if !has_any_files {
            show_upload_status(StatusState::Error(
                "所有章节都没有图片，请先添加图片。".to_string(),
            ))
            .await;
            return;
        }
    }

    show_upload_status(StatusState::Processing("正在连接快应用...".to_string())).await;

    let progress = upload_progress();
    let device_addr = match handshake::prepare_launch(handshake::MIN_UPLOAD_VERSION, &progress).await {
        Ok(addr) => addr,
        Err(msg) => {
            show_upload_status(StatusState::Error(msg)).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成后在 on_done 回调里继续上传流程
    handshake::begin_wait(
        device_addr.clone(),
        upload_progress(),
        move |result| async move { match result {
            Err(msg) => {
                show_upload_status(StatusState::Error(msg)).await;
            }
            Ok(settings_opt) => {
                let http_capable = {
                    let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
                    state.watch_http_import
                };
                if http_capable {
                    prepare_http_import(device_addr, settings_opt, None).await;
                } else {
                    upload_start_continue(device_addr).await;
                }
            }
        } },
    );
}

async fn upload_start_continue(device_addr: String) {
    let watch_settings = current_watch_settings();

    let (upload_mode, items, chapters, multi_cover) = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            state.upload_mode.clone(),
            state.upload_items.clone(),
            state.upload_chapters.clone(),
            state.multi_cover.clone(),
        )
    };

    let comic_name = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let raw = state.upload_comic_name_input.trim().to_string();
        if raw.is_empty() {
            items
                .first()
                .map(|i| i.comic_name.clone())
                .unwrap_or_default()
        } else {
            raw
        }
    };

    let is_single = upload_mode == UploadMode::Single;

    let mut all_files: Vec<(String, String)> = Vec::new();
    let mut header: Value;

    if is_single {
        let mut file_names: Vec<String> = Vec::new();

        for item in items.iter() {
            // Process cover first
            if let Some(ref cover_file) = item.cover {
                show_upload_status(StatusState::Processing(format!(
                    "正在处理 封面 ({}/{})",
                    all_files.len() + 1,
                    "-"
                )))
                .await;

                let Some(product) = prepare_upload_image(cover_file, &watch_settings, ImageRole::Cover, "单本封面").await else { return; };
                let b64 = base64_encode(&product.bytes);
                file_names.push("cover".to_string());
                all_files.push(("cover".to_string(), b64));
            }

            // Process pages
            let mut page_num: u32 = 0;
            for file in item.files.iter() {
                page_num += 1;
                show_upload_status(StatusState::Processing(format!(
                    "正在处理 ({}/{})",
                    all_files.len() + 1,
                    "-"
                )))
                .await;

                let Some(product) = prepare_upload_image(file, &watch_settings, ImageRole::Page,
                    &format!("单本第{}页", page_num)).await else { return; };
                let name = product.format.page_name(page_num);
                let b64 = base64_encode(&product.bytes);
                file_names.push(name.clone());
                all_files.push((name, b64));
            }
        }

        header = json!({
            "type": "import_comic_header",
            "name": comic_name,
            "mode": "single",
            "files": file_names,
        });
    } else {
        let mut chap_list: Vec<Value> = Vec::new();

        // Process shared book-level cover first
        if let Some(ref cover_file) = multi_cover {
            show_upload_status(StatusState::Processing("正在处理 封面".to_string())).await;

            let Some(product) = prepare_upload_image(cover_file, &watch_settings, ImageRole::Cover, "多章作品封面").await else { return; };
            let b64 = base64_encode(&product.bytes);
            all_files.push(("cover".to_string(), b64));
        }

        for (ci, chapter) in chapters.iter().enumerate() {
            if chapter.files.is_empty() {
                continue;
            }
            let mut chap_names: Vec<String> = Vec::new();
            let mut page_num: u32 = 0;

            // 章节目录/分片键/头部章节名统一为 "<章号>　<章名>"（全角空格 U+3000 分隔）：
            // 手表阅读页按 "<章号>　<章名>" 拼接章节目录路径（photo.ux），
            // 下载链路建目录（download.ux）与书架扫描（offline.ux）同此约定；
            // 元数据剥离前缀由手表端 updateComicsIndex 负责
            let ch_name_raw = if chapter.name.trim().is_empty() {
                format!("第{}章", ci + 1)
            } else {
                chapter.name.trim().to_string()
            };
            let ch_name_val = format!("{}　{}", ci + 1, ch_name_raw);

            // Process pages
            for (fi, file) in chapter.files.iter().enumerate() {
                page_num += 1;
                show_upload_status(StatusState::Processing(format!(
                    "正在处理 章节{} ({}/{})",
                    ci + 1,
                    fi + 1,
                    chapter.files.len()
                )))
                .await;

                let Some(product) = prepare_upload_image(file, &watch_settings, ImageRole::Page,
                    &format!("第{}章第{}页", ci + 1, page_num)).await else { return; };
                let name = product.format.page_name(page_num);
                let file_key = format!("{}/{}", ch_name_val, name);
                let b64 = base64_encode(&product.bytes);
                chap_names.push(name.clone());
                all_files.push((file_key, b64));
            }

            chap_list.push(json!({
                "name": ch_name_val,
                "files": chap_names,
            }));
        }

        header = json!({
            "type": "import_comic_header",
            "name": comic_name,
            "mode": "multi",
            "chapters": chap_list,
        });
    }

    let total = all_files.len();

    let mut chunked_files: Vec<(String, Vec<String>)> = Vec::with_capacity(total);
    for (file_key, b64_data) in all_files {
        let chunks: Vec<String> = b64_data
            .as_bytes()
            .chunks(CHUNK_SIZE)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        chunked_files.push((file_key, chunks));
    }

    // 窗口模式：按快应用协商能力构建滑窗会话；旧快应用无能力 → None 走逐片停等
    let windowed = build_windowed_upload(&chunked_files);
    if let Some(ref w) = windowed {
        // 窗口会话标记 + 全局总片数；旧快应用忽略未知字段天然兼容
        header["wchunks"] = json!(w.order.len());
    }

    let header_str = match serde_json::to_string(&header) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化头部消息失败: {}", e);
            show_upload_status(StatusState::Error("数据序列化失败。".to_string())).await;
            return;
        }
    };

    let first_file_key = chunked_files[0].0.clone();
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_session = Some(UploadSession {
            device_addr: device_addr.clone(),
            comic_name: comic_name.clone(),
            all_files: chunked_files,
            current_file: 0,
            current_chunk: 0,
            total_files: total,
            awaiting: None,
            retry_count: 0,
            header_str: header_str.clone(),
            header_acked: false,
            header_retry: 0,
            windowed,
        });
        state.upload_current_file = first_file_key;
    }

    // 先发头部并等快应用确认（import_header_ack）后再发分片，
    // 防止安卓端乱序导致分片先于头部到达被丢弃
    show_upload_status(StatusState::Processing("正在发送数据...".to_string())).await;
    send_import_header(&device_addr, &header_str).await;
}

async fn send_next_chunk() {
    // 取出 session 所有权来避免借用冲突
    let session = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_session.take()
    };

    let mut session = match session {
        Some(s) => s,
        None => return,
    };

    // 找到下一个要发送的分片
    loop {
        if session.current_file >= session.all_files.len() {
            // 全部发送完毕
            let device_addr = session.device_addr.clone();
            let comic_name = session.comic_name.clone();
            complete_upload(device_addr, comic_name).await;
            return;
        }

        let chunks_len = session.all_files[session.current_file].1.len();

        if session.current_chunk >= chunks_len {
            // 当前文件发送完毕，跳到下一个文件
            session.current_file += 1;
            session.current_chunk = 0;
            if session.current_file < session.all_files.len() {
                let next_key = session.all_files[session.current_file].0.clone();
                {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.upload_progress =
                        session.current_file as f32 / session.total_files as f32;
                    state.upload_current_file = next_key;
                }
            }
            continue;
        }

        let file_key = session.all_files[session.current_file].0.clone();
        let chunk = session.all_files[session.current_file].1[session.current_chunk].clone();
        let idx = session.current_chunk;
        let file_idx = session.current_file;
        session.current_chunk += 1;
        session.awaiting = Some((file_idx, idx));

        let msg = json!({
            "type": "import_comic_chunk",
            "name": session.comic_name,
            "file": file_key,
            "index": idx,
            "total": chunks_len,
            "data": chunk,
        });

        let device_addr = session.device_addr.clone();

        // 存回 session
        {
            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.upload_session = Some(session);
        }

        let chunk_str = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(_) => {
                reset_upload_progress();
                show_upload_status(StatusState::Error("序列化失败。".to_string())).await;
                return;
            }
        };

        match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), chunk_str).await {
            Ok(_) => {
                arm_ack_timeout().await;
                let status_text = {
                    let state = ui_state()
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(s) = &state.upload_session {
                        format!("正在发送 {}/{}", s.current_file + 1, s.total_files)
                    } else {
                        String::new()
                    }
                };
                if !status_text.is_empty() {
                    show_upload_status(StatusState::Processing(status_text)).await;
                }
            }
            Err(e) => {
                tracing::error!("发送分片失败: {:?}", e);
                disarm_ack_timeout().await;
                {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.upload_session = None;
                }
                reset_upload_progress();
                show_upload_status(StatusState::Error("发送中断，请重试。".to_string())).await;
            }
        }

        return;
    }
}

/// 传输完成：清会话、发送 done，并明确设备保存结果尚待核实。
async fn complete_upload(device_addr: String, comic_name: String) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_progress = 1.0;
        state.upload_current_file.clear();
        state.upload_session = None;
    }

    disarm_ack_timeout().await;

    let done_msg = json!({
        "type": "import_comic_done",
        "name": comic_name,
    });
    let done_str = match serde_json::to_string(&done_msg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化完成消息失败: {}", e);
            show_upload_status(StatusState::Error("序列化失败。".to_string())).await;
            return;
        }
    };
    if let Err(e) =
        interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), done_str).await
    {
        tracing::error!("发送完成消息失败: {:?}", e);
        show_upload_status(StatusState::Error("图片已传输，但收尾消息发送失败，请在设备书架核实后重试。".into())).await;
        return;
    }
    show_upload_status(StatusState::Success("图片已发送，设备保存结果待核实。请在设备书架查看。".to_string())).await;
}

/// 依据快应用握手协商的导入窗口能力构建窗口会话；
/// 无能力（旧版快应用或未握手）返回 None，调用方走逐片停等旧路径
fn build_windowed_upload(chunked_files: &[(String, Vec<String>)]) -> Option<WindowedUpload> {
    let win = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.watch_import_window
    }?;
    let mut order = Vec::new();
    for (fi, (_, chunks)) in chunked_files.iter().enumerate() {
        for ci in 0..chunks.len() {
            order.push((fi, ci));
        }
    }
    Some(WindowedUpload {
        sender: WindowedSender::new(win),
        order,
    })
}

/// 把一组 gseq 渲染成分片消息（锁内只做查表与序列化，不做发送）
/// 返回 (device_addr, frames)；会话缺失/非窗口模式返回 None
fn build_windowed_frames(gseqs: &[usize]) -> Option<(String, Vec<String>)> {
    let (device_addr, frames, progress, current_file) = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let s = state.upload_session.as_mut()?;
        let w = s.windowed.as_mut()?;
        let total = w.order.len();
        let mut frames = Vec::with_capacity(gseqs.len());
        for &g in gseqs {
            let (fi, ci) = w.order[g];
            let msg = json!({
                "type": "import_comic_chunk",
                "name": s.comic_name.as_str(),
                "file": s.all_files[fi].0.as_str(),
                "index": ci,
                "total": s.all_files[fi].1.len(),
                "gseq": g,
                "data": s.all_files[fi].1[ci].as_str(),
            });
            frames.push(msg.to_string());
        }
        // 进度按累计确认前沿推进（文件粒度状态文本取前沿所在文件）
        let progress = w.sender.base() as f32 / total as f32;
        let front = w.sender.base().min(total.saturating_sub(1));
        let current_file = s.all_files[w.order[front].0].0.clone();
        (s.device_addr.clone(), frames, progress, current_file)
    };
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_progress = progress;
        state.upload_current_file = current_file;
    }
    Some((device_addr, frames))
}

/// 锁外发送一批窗口分片；发送失败中止会话（与逐片路径语义一致）
async fn send_windowed_frames(device_addr: String, frames: Vec<String>) {
    if frames.is_empty() {
        return;
    }
    let total = frames.len();
    for (i, frame) in frames.into_iter().enumerate() {
        if let Err(e) =
            interconnect::send_qaic_message(device_addr.clone(), WATCH_APP_PKG_NAME.into(), frame).await
        {
            tracing::error!("窗口分片发送失败 ({}/{}): {:?}", i + 1, total, e);
            disarm_ack_timeout().await;
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.upload_session = None;
            }
            reset_upload_progress();
            show_upload_status(StatusState::Error("发送中断，请重试。".to_string())).await;
            return;
        }
    }
    // 每批发完（含补窗/重发）都重新武装超时：整窗全丢时没有重复 ACK 可依赖，
    // 超时是唯一的重传驱动
    arm_ack_timeout().await;
    let status_text = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.upload_session.is_some() {
            format!(
                "正在发送 {} ({:.0}%)",
                state.upload_current_file,
                state.upload_progress * 100.0
            )
        } else {
            String::new()
        }
    };
    if !status_text.is_empty() {
        show_upload_status(StatusState::Processing(status_text)).await;
    }
}

/// 窗口模式：发完头部收到确认后首次泵满窗口
async fn pump_upload_window() {
    let gseqs = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.upload_session.as_mut() {
            Some(s) => match s.windowed.as_mut() {
                Some(w) => {
                    let total = w.order.len();
                    w.sender.pump(total)
                }
                None => return,
            },
            None => return,
        }
    };
    if let Some((device_addr, frames)) = build_windowed_frames(&gseqs) {
        send_windowed_frames(device_addr, frames).await;
    }
}

/// 窗口模式的累计 ACK：ack = 下一个仍缺失的连续 gseq。
/// 前进 → 补发新帧；停滞 → go-back-N 整窗重发一次；全部确认 → 收尾
async fn handle_windowed_chunk_ack(name: &str, ack: usize) {
    enum WAction {
        Nothing,
        Frames(Vec<usize>),
        Done { device_addr: String, comic_name: String },
    }
    let action = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.upload_session.as_mut() {
            Some(s) if s.comic_name == name => match s.windowed.as_mut() {
                Some(w) => {
                    let total = w.order.len();
                    let (gseqs, done) = w.sender.on_ack(ack, total);
                    s.retry_count = 0;
                    if done {
                        WAction::Done {
                            device_addr: s.device_addr.clone(),
                            comic_name: s.comic_name.clone(),
                        }
                    } else if gseqs.is_empty() {
                        WAction::Nothing
                    } else {
                        WAction::Frames(gseqs)
                    }
                }
                None => WAction::Nothing,
            },
            _ => WAction::Nothing,
        }
    };
    match action {
        WAction::Nothing => {}
        WAction::Done {
            device_addr,
            comic_name,
        } => {
            complete_upload(device_addr, comic_name).await;
        }
        WAction::Frames(gseqs) => {
            if let Some((device_addr, frames)) = build_windowed_frames(&gseqs) {
                send_windowed_frames(device_addr, frames).await;
            }
        }
    }
}

// 分片上限 32K（base64 字符 ≈ 24KB 二进制，加 JSON 外壳约 33K 字符）：
// 窗口化后单片放大以摊薄 BLE 往返；同方向链路在网桥插件上 24K 帧已实测可行，
// 43.7K 帧待真机验证——本值对应 33K 帧，若真机不稳回退 16384
const CHUNK_SIZE: usize = 32768;

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    let len = data.len();

    for i in (0..len).step_by(3) {
        let b1 = data[i];
        let b2 = if i + 1 < len { data[i + 1] } else { 0 };
        let b3 = if i + 2 < len { data[i + 2] } else { 0 };

        result.push(CHARS[(b1 >> 2) as usize] as char);
        result.push(CHARS[((b1 & 3) << 4 | b2 >> 4) as usize] as char);
        result.push(if i + 1 < len {
            CHARS[((b2 & 15) << 2 | b3 >> 6) as usize] as char
        } else {
            '='
        });
        result.push(if i + 2 < len {
            CHARS[(b3 & 63) as usize] as char
        } else {
            '='
        });
    }

    result
}

fn update_domain_state(input_value: String) {
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.source_form.set_input(input_value) && let Some(sync) = state.source_sync.as_mut().filter(|s| s.phase.busy()) {
        sync.phase = if sync.configs_sent { SyncPhase::Partial } else { SyncPhase::Cancelled };
        sync.message = if sync.configs_sent { "配置命令已发送，入口已变化；剩余命令停止发送，设备结果待核实" }
            else { "配置入口已变化，原快照命令未发送" }.into();
    }
    drop(state);
    build::rerender_main_ui();
}

fn handle_source_form_event(id: &str, payload: &str) -> bool {
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    if state.source_sync_busy() { return false; }
    for (prefix, select) in [(SOURCE_ALL_PREFIX, true), (SOURCE_NONE_PREFIX, false)] {
        if let Some(generation) = id.strip_prefix(prefix).and_then(|v| v.parse::<u64>().ok()) {
            let total = state.source_form.choices.len();
            let mut changed = false;
            for index in 0..total { changed |= state.source_form.edit_choice(generation, index, |c| c.selected = select); }
            return changed;
        }
    }
    let prefixes = [SOURCE_SELECT_PREFIX, SOURCE_COOKIE_INPUT_PREFIX, SOURCE_COOKIE_KEEP_PREFIX, SOURCE_COOKIE_UPDATE_PREFIX, SOURCE_COOKIE_CLEAR_PREFIX];
    let Some((prefix, value)) = prefixes.iter().find_map(|prefix| id.strip_prefix(prefix).map(|value| (*prefix, value))) else { return false; };
    let Some((generation, index)) = parse_data_action(value) else { return false; };
    match prefix {
        SOURCE_SELECT_PREFIX => {
            let parsed = serde_json::from_str::<Value>(payload).ok();
            let checked = parsed.as_ref().and_then(|v| v.get("checked").or_else(|| v.get("value"))).and_then(|v|
                v.as_bool().or_else(|| match v.as_str() { Some("true" | "1" | "on") => Some(true), Some("false" | "0" | "off") => Some(false), _ => None }));
            checked.is_some_and(|checked| state.source_form.edit_choice(generation, index, |c| c.selected = checked))
        }
        SOURCE_COOKIE_INPUT_PREFIX => {
            let value = serde_json::from_str::<Value>(payload).ok().and_then(|v| v.get("value").and_then(Value::as_str).map(str::to_string));
            value.is_some_and(|value| state.source_form.edit_choice(generation, index, |c| {
                // A delayed input from before a keep/clear action cannot reverse that action.
                if c.cookie_action == CookieAction::Update { c.cookie = value; }
            }))
        }
        _ => state.source_form.edit_choice(generation, index, |c| c.cookie_action = match prefix {
            SOURCE_COOKIE_KEEP_PREFIX => CookieAction::Keep, SOURCE_COOKIE_UPDATE_PREFIX => CookieAction::Update, _ => CookieAction::Clear,
        }),
    }
}

async fn handle_domain_blur(input_value: String) {
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    if state.source_sync_busy() { return; }
    // Bootstrap an initial blur-only host event. Later stale blur values never overwrite Change.
    if state.source_form.generation == 0 && state.source_form.input.is_empty() { state.source_form.set_input(input_value.clone()); }
    let matching = input_value == state.source_form.input ||
        crate::source_config::normalize_endpoint(&input_value).ok().is_some_and(|endpoint| state.source_form.endpoint.as_ref() == Some(&endpoint));
    drop(state);
    if matching { load_source_config(false).await; }
}

async fn load_source_config(force: bool) {
    let ticket = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if state.source_sync_busy() { return; }
        match state.source_form.begin_load(force) {
            Ok(ticket) => ticket,
            Err(error) => { state.source_form.phase = CatalogPhase::Error; state.source_form.message = error.to_string(); None }
        }
    };
    build::rerender_main_ui();
    let Some(ticket) = ticket else { return; };
    let result = fetch_source_catalog(&ticket.endpoint).await;
    let changed = ui_state().write().unwrap_or_else(|p| p.into_inner()).source_form.apply_load(&ticket, result);
    if changed { build::rerender_main_ui(); }
}

/// ACK 超时（毫秒）与最大重传次数
const ACK_TIMEOUT_MS: u64 = 3000;
const MAX_ACK_RETRIES: u32 = 5;

/// 连接手表快应用并完成握手。
/// 使用重构后的握手模块，借鉴 FetchBridge v3 协议：
/// - 会话状态持久化，复用已完成的握手
/// - 自动清理过期会话
/// - 分段异步等待，避免完全阻塞
/// - 解决安卓端乱序和握手错位问题
/// 同步渲染上传页状态栏（无定时器管理），
/// 在定时器事件等同步上下文中也能安全调用（spawn 在同步上下文不可靠）
fn render_upload_progress(msg: String) {
    let root_id = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_status = StatusState::Processing(msg);
        state.root_element_id.clone()
    };
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

/// 同步渲染数据页状态栏（无定时器管理）
fn render_app_data_progress(msg: String) {
    let root_id = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_data_status = StatusState::Processing(msg);
        state.root_element_id.clone()
    };
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

/// 生成一个把握手阶段进度转发到上传状态栏的回调
fn upload_progress() -> impl Fn(String) {
    |msg| render_upload_progress(msg)
}

/// 生成一个把握手阶段进度转发到数据页状态栏的回调
fn app_data_progress() -> impl Fn(String) {
    |msg| render_app_data_progress(msg)
}

/// 当前生效的快应用设置（未握手时使用与快应用一致的缺省值）
fn current_watch_settings() -> WatchSettings {
    // 降级到 UI 状态的缓存，最后用默认值
    // 握手完成后会自动更新到握手模块，这里保持 UI 状态兼容旧代码
    let state = ui_state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.watch_settings.clone().unwrap_or_default()
}

pub(crate) fn record_image_notices(context: &str, notices: &[String]) {
    if notices.is_empty() { return; }
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    for notice in notices {
        let notice = format!("{context}：{notice}");
        if state.image_notices.len() < 8 && !state.image_notices.contains(&notice) { state.image_notices.push(notice); }
    }
    drop(state);
    rerender_upload_ui();
}

async fn prepare_upload_image(file: &UploadFile, settings: &WatchSettings, role: ImageRole, context: &str) -> Option<PreparedImage> {
    let result = file.get_master_data().and_then(|bytes| crate::image_processor::prepare_image(&bytes, &settings.image_request(role)));
    match result {
        Ok(product) => { record_image_notices(context, &product.notices); Some(product) }
        Err(error) => {
            show_upload_status(StatusState::Error(format!("{context}《{}》：{error}", file.name))).await;
            None
        }
    }
}

/// 武装/重武装 ACK 超时定时器
async fn arm_ack_timeout() {
    let old = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_ack_timer_id.take()
    };
    if let Some(t) = old {
        timer::clear_timer(t);
    }
    let tid = timer::set_timeout(ACK_TIMEOUT_MS, UPLOAD_ACK_TIMEOUT_EVENT);
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.upload_ack_timer_id = Some(tid);
}

async fn disarm_ack_timeout() {
    let old = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_ack_timer_id.take()
    };
    if let Some(t) = old {
        timer::clear_timer(t);
    }
}

/// 发送导入头部并武装头部 ACK 超时定时器。
/// 必须等快应用回 import_header_ack 后才开始发分片：
/// 安卓端 QAIC 不保证消息顺序，分片可能反超头部被手表丢弃导致死锁。
async fn send_import_header(device_addr: &str, header_str: &str) {
    match interconnect::send_qaic_message(device_addr.into(), WATCH_APP_PKG_NAME.into(), header_str.into()).await {
        Ok(_) => {
            tracing::info!("头部消息发送成功，等待快应用确认");
            arm_header_timeout().await;
        }
        Err(e) => {
            tracing::error!("发送头部消息失败: {:?}", e);
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.upload_session = None;
            }
            reset_upload_progress();
            show_upload_status(StatusState::Error("发送失败，请重试。".to_string())).await;
        }
    }
}

async fn arm_header_timeout() {
    let old = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_header_timer_id.take()
    };
    if let Some(t) = old {
        timer::clear_timer(t);
    }
    let tid = timer::set_timeout(ACK_TIMEOUT_MS, UPLOAD_HEADER_TIMEOUT_EVENT);
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.upload_header_timer_id = Some(tid);
}

async fn disarm_header_timeout() {
    let old = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_header_timer_id.take()
    };
    if let Some(t) = old {
        timer::clear_timer(t);
    }
}

/// 头部 ACK 超时：重发头部（上限 3 次）；超限说明对端可能是
/// 无 header ACK 机制的旧版快应用，退回兼容模式直接发分片。
pub async fn handle_upload_header_timeout() {
        enum Action {
            Nothing,
            Resend,
            LegacyStart,
        }

        let (action, device_addr, header_str) = {
            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.upload_session.as_mut() {
                Some(s) if !s.header_acked => {
                    s.header_retry += 1;
                    if s.header_retry > 3 {
                        s.header_acked = true;
                        // 旧版兼容模式：退化为逐片会话（窗口帧依赖对端累计 ACK 能力）
                        s.windowed = None;
                        (Action::LegacyStart, s.device_addr.clone(), String::new())
                    } else {
                        (Action::Resend, s.device_addr.clone(), s.header_str.clone())
                    }
                }
                _ => (Action::Nothing, String::new(), String::new()),
            }
        };

        match action {
            Action::Nothing => {}
            Action::Resend => {
                tracing::warn!("头部 ACK 超时，重发头部消息");
                match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), header_str)
                    .await
                {
                    Ok(_) => {
                        arm_header_timeout().await;
                    }
                    Err(e) => {
                        tracing::error!("重发头部消息失败: {:?}", e);
                        disarm_header_timeout().await;
                        {
                            let mut state = ui_state()
                                .write()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.upload_session = None;
                        }
                        reset_upload_progress();
                        show_upload_status(StatusState::Error("发送失败，请重试。".to_string()))
                            .await;
                    }
                }
            }
            Action::LegacyStart => {
                tracing::warn!("未收到头部确认，按旧版兼容模式直接发送分片");
                send_next_chunk().await;
            }
        }
}

/// ACK 超时处理：窗口模式整窗 go-back-N 重发；逐片模式重传当前在途分片；
/// 超过重传上限则中止上传。由定时器事件 UPLOAD_ACK_TIMEOUT_EVENT 触发。
pub async fn handle_upload_ack_timeout() {
        enum Action {
            Nothing,
            Resend(usize, usize),
            WindowGbn,
            Abort,
        }

        let (action, device_addr) = {
            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.upload_session.as_mut() {
                Some(s) if s.windowed.is_some() => {
                    s.retry_count += 1;
                    if s.retry_count > MAX_ACK_RETRIES {
                        (Action::Abort, s.device_addr.clone())
                    } else {
                        (Action::WindowGbn, String::new())
                    }
                }
                Some(s) if s.awaiting.is_some() => {
                    s.retry_count += 1;
                    if s.retry_count > MAX_ACK_RETRIES {
                        (Action::Abort, s.device_addr.clone())
                    } else {
                        let (fi, ci) = s.awaiting.unwrap();
                        (Action::Resend(fi, ci), s.device_addr.clone())
                    }
                }
                _ => (Action::Nothing, String::new()),
            }
        };

        match action {
            Action::Nothing => {}
            Action::Abort => {
                tracing::error!("分片重传超过上限，中止上传");
                {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.upload_session = None;
                }
                reset_upload_progress();
                show_upload_status(StatusState::Error("发送中断，请重试。".to_string())).await;
            }
            Action::WindowGbn => {
                // 整窗回退到累计确认前沿重发（窗口模式无逐片 awaiting 可言）
                let gseqs = {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match state.upload_session.as_mut() {
                        Some(s) => match s.windowed.as_mut() {
                            Some(w) => {
                                let total = w.order.len();
                                w.sender.go_back_n(total)
                            }
                            None => Vec::new(),
                        },
                        None => Vec::new(),
                    }
                };
                if gseqs.is_empty() {
                    // 无在途帧（不应发生：有在途才会武装超时），重武装兜底
                    arm_ack_timeout().await;
                } else {
                    tracing::warn!("ACK 超时，整窗 go-back-N 重发 {} 帧", gseqs.len());
                    if let Some((device_addr, frames)) = build_windowed_frames(&gseqs) {
                        send_windowed_frames(device_addr, frames).await;
                    }
                }
            }
            Action::Resend(fi, ci) => {
                let (comic_name, file_key, chunk, total) = {
                    let state = ui_state()
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match &state.upload_session {
                        Some(s) => (
                            s.comic_name.clone(),
                            s.all_files[fi].0.clone(),
                            s.all_files[fi].1[ci].clone(),
                            s.all_files[fi].1.len(),
                        ),
                        None => return,
                    }
                };

                tracing::warn!("ACK 超时，重传分片: file={}, index={}", file_key, ci);

                let chunk_str = json!({
                    "type": "import_comic_chunk",
                    "name": comic_name,
                    "file": file_key,
                    "index": ci,
                    "total": total,
                    "data": chunk,
                })
                .to_string();

                match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), chunk_str)
                    .await
                {
                    Ok(_) => {
                        arm_ack_timeout().await;
                    }
                    Err(e) => {
                        tracing::error!("重传分片失败: {:?}", e);
                        disarm_ack_timeout().await;
                        {
                            let mut state = ui_state()
                                .write()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.upload_session = None;
                        }
                        reset_upload_progress();
                        show_upload_status(StatusState::Error("发送中断，请重试。".to_string()))
                            .await;
                    }
                }
            }
        }
}

async fn handle_sync() {
    let plan = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if state.source_sync_busy() { return; }
        if state.source_form.phase != CatalogPhase::Ready { drop(state); build::rerender_main_ui(); return; }
        if state.data_browser.busy() || state.deletes.busy() || state.upload_locked() {
            state.source_form.message = "其他设备操作正在进行，请完成后再发送源配置".into();
            drop(state); build::rerender_main_ui(); return;
        }
        match state.source_form.build_plan() {
            Ok(plan) => plan,
            Err(error) => { state.source_form.message = error.to_string(); drop(state); build::rerender_main_ui(); return; }
        }
    };
    // Construct and validate both payloads before any launch/send; never fetch here.
    let devices = device::get_connected_device_list().await;
    let Some(target) = devices.first() else {
        show_status(StatusState::Error("没有已连接的设备，请检查手表连接。".into())).await;
        return;
    };
    let id = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if !state.source_form.plan_current(&plan) { return; }
        state.source_sync_next = state.source_sync_next.wrapping_add(1);
        let id = state.source_sync_next;
        state.source_sync = Some(SourceSync { id, plan, device_name:target.name.clone(), device_addr:target.addr.clone(),
            phase:SyncPhase::Preparing, configs_sent:false, cookies_sent:false, message:"正在检查并连接捕获的目标设备…".into() });
        id
    };
    build::rerender_main_ui();
    let device_addr = match handshake::prepare_launch(handshake::MIN_MANAGEMENT_VERSION, &source_sync_progress(id)).await {
        Ok(addr) if addr == target.addr && source_sync_current(id) => addr,
        Ok(_) => { finish_source_sync(id, SyncPhase::Cancelled, "目标设备或配置入口已变化，命令未发送".into()); return; }
        Err(message) => { finish_source_sync(id, SyncPhase::Failed, message); return; }
    };
    handshake::begin_wait(device_addr, source_sync_progress(id), move |result| async move {
        if !source_sync_current(id) { return; }
        match result {
            Err(message) => finish_source_sync(id, SyncPhase::Failed, message),
            Ok(_) => send_source_sync(id).await,
        }
    });
}

fn source_sync_current(id: u64) -> bool { ui_state().read().unwrap_or_else(|p| p.into_inner()).source_sync_current(id) }

fn source_sync_progress(id: u64) -> impl Fn(String) + Send + 'static {
    move |message| {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if state.source_sync_current(id) && let Some(sync) = state.source_sync.as_mut() { sync.message = message; }
        drop(state); build::rerender_main_ui();
    }
}

fn finish_source_sync(id: u64, phase: SyncPhase, message: String) {
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    if let Some(sync) = state.source_sync.as_mut().filter(|s| s.id == id && s.phase.busy()) {
        sync.phase = phase; sync.message = message;
    }
    drop(state); build::rerender_main_ui();
}

async fn source_target_connected(sync: &SourceSync) -> bool {
    device::get_connected_device_list().await.first().is_some_and(|device| device.addr == sync.device_addr)
}

async fn send_source_sync(id: u64) {
    let sync = {
        let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
        if !state.source_sync_current(id) { return; }
        state.source_sync.as_ref().unwrap().clone()
    };
    if !source_target_connected(&sync).await {
        finish_source_sync(id, SyncPhase::Cancelled, "握手后连接已变化，配置与 Cookie 命令未发送".into()); return;
    }
    {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if !state.source_sync_current(id) { return; }
        let op = state.source_sync.as_mut().unwrap(); op.phase = SyncPhase::Sending; op.message = "正在发送固定快照中的所选源配置…".into();
    }
    build::rerender_main_ui();
    if interconnect::send_qaic_message(sync.device_addr.clone(), WATCH_APP_PKG_NAME.into(), sync.plan.configs_message.clone()).await.is_err() {
        finish_source_sync(id, SyncPhase::Failed, "配置命令发送异常，设备是否收到待核实；Cookie 命令未发送".into()); return;
    }
    {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if let Some(op) = state.source_sync.as_mut().filter(|s| s.id == id) { op.configs_sent = true; }
    }
    if !source_sync_current(id) { return; }
    if let Some(message) = &sync.plan.cookie_message {
        if !source_target_connected(&sync).await {
            finish_source_sync(id, SyncPhase::Partial, "配置命令已发送，但连接已变化；Cookie 未发送，设备保存结果待核实".into()); return;
        }
        if !source_sync_current(id) { return; }
        source_sync_progress(id)("所选源配置命令已发送，正在发送各 key 的 Cookie 修改…".into());
        if interconnect::send_qaic_message(sync.device_addr.clone(), WATCH_APP_PKG_NAME.into(), message.clone()).await.is_err() {
            finish_source_sync(id, SyncPhase::Partial, "配置命令已发送，Cookie 命令发送异常；设备保存结果待核实".into()); return;
        }
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if let Some(op) = state.source_sync.as_mut().filter(|s| s.id == id) { op.cookies_sent = true; }
    }
    finish_source_sync(id, SyncPhase::Sent, format!("已发送 {} 个所选源配置及 {} 项 Cookie 修改；设备保存结果待核实。可重新读取设备源列表确认配置。",
        sync.plan.keys.len(), sync.plan.cookie_count));
}

fn parse_data_action(value: &str) -> Option<(u64, usize)> {
    let (revision, index) = value.split_once('_')?;
    Some((revision.parse().ok()?, index.parse().ok()?))
}

pub async fn refresh_data_connection() {
    let devices = device::get_connected_device_list().await;
    ui_state().write().unwrap_or_else(|p| p.into_inner()).data_browser.checked_device =
        devices.first().map(|d| super::data_browser::DataDevice { name: d.name.clone(), addr: d.addr.clone() });
}

pub async fn handle_data_device_action() {
    refresh_data_connection().await;
    let timer_id = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        let changed = state.data_browser.busy() && state.data_browser.requested_device.as_ref()
            .is_some_and(|requested| state.data_browser.checked_device.as_ref()
                .is_none_or(|current| current.addr != requested.addr));
        if changed {
            let message = "读取目标设备已断开或连接已切换，请重新读取".to_string();
            state.data_browser.fail(message.clone());
            state.app_data_status = StatusState::Error(message);
            state.sync_receive.finish();
            state.http_data_sync = None;
            state.app_data_recv_timer_id.take()
        } else { None }
    };
    if let Some(id) = timer_id { timer::clear_timer(id); }
    let timers = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        let current = state.data_browser.checked_device.as_ref().map(|d| d.addr.clone());
        let mut timers = Vec::new();
        for request in &mut state.deletes.requests {
            if request.phase.busy() && current.as_deref() != Some(&request.target.owner.addr) {
                request.phase = if request.session.is_empty() { DeletePhase::Failed } else { DeletePhase::Unknown };
                request.message = if request.session.is_empty() { "设备连接已变化，删除命令未发送" }
                    else { "目标设备已断开或连接已切换，删除结果未确认；请重连后查询或读取核实" }.into();
                if let Some(id) = request.timer_id.take() { timers.push(id); }
            }
        }
        if let Some(sync) = state.source_sync.as_mut().filter(|s| s.phase.busy()) && current.as_deref() != Some(&sync.device_addr) {
            sync.phase = if sync.configs_sent { SyncPhase::Partial } else { SyncPhase::Cancelled };
            sync.message = if sync.configs_sent { "原设备连接已变化，已发送配置的保存结果待核实；剩余命令停止发送" }
                else { "原目标设备已断开或切换，配置与 Cookie 命令未发送" }.into();
        }
        timers
    };
    for id in timers { timer::clear_timer(id); }
    build::rerender_main_ui();
}

async fn fail_library_sync(message: String) {
    ui_state().write().unwrap_or_else(|p| p.into_inner()).data_browser.fail(message.clone());
    show_app_data_status(StatusState::Error(message)).await;
}

async fn handle_delete_data(revision: u64, index: usize, source: bool) {
    let target = ui_state().read().unwrap_or_else(|p| p.into_inner()).capture_data_target(revision, index, source);
    let Some(target) = target else {
        show_app_data_status(StatusState::Error("列表已变化、未完整或正在同步，请重新读取后选择条目。".into())).await;
        return;
    };
    refresh_data_connection().await;
    if !data_delete_valid(&target) {
        data_delete_status(&target, StatusState::Error("当前连接与这份列表不匹配，请读取目标设备数据后再操作。".into())).await;
        return;
    }
    if source { handle_delete_source(target).await; } else { handle_delete_comic(target).await; }
}

fn data_delete_valid(target: &DataTarget) -> bool {
    let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
    state.data_target_current(target) && state.data_browser.checked_device.as_ref()
        .is_some_and(|device| device.addr == target.owner.addr)
}

fn delete_progress(id: &str) -> impl Fn(String) + Send + 'static {
    let id = id.to_string();
    move |message| {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if let Some(request) = state.deletes.get_mut(&id) && request.phase.busy() {
            request.message = message;
        }
        drop(state);
        build::rerender_main_ui();
    }
}

async fn data_delete_status(target: &DataTarget, status: StatusState) {
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.revision == target.revision {
        show_app_data_status(status).await;
    }
}

async fn prepare_data_delete(target: &DataTarget, id: &str) -> Option<String> {
    refresh_data_connection().await;
    if !data_delete_valid(target) {
        set_delete_phase(id, DeletePhase::Failed, "列表或连接已变化，删除命令未发送".into());
        return None;
    }
    match handshake::prepare_launch(handshake::MIN_MANAGEMENT_VERSION, &delete_progress(id)).await {
        Ok(addr) if addr == target.owner.addr && data_delete_valid(target) => Some(addr),
        Ok(_) => {
            set_delete_phase(id, DeletePhase::Failed, "目标设备已变化，删除命令未发送。".into());
            None
        }
        Err(message) => { set_delete_phase(id, DeletePhase::Failed, message); None }
    }
}

async fn handle_delete_comic(target: DataTarget) {
    let comic_name = target.item.name().to_string();

    let dialog_info = dialog::DialogInfo {
        title: format!("确认删除《{}》", comic_name),
        content: "此操作将删除该漫画的所有本地文件，不可恢复。".to_string(),
        buttons: vec![
            dialog::DialogButton {
                id: "cancel".to_string(),
                primary: false,
                content: "取消".to_string(),
            },
            dialog::DialogButton {
                id: "confirm".to_string(),
                primary: true,
                content: "确认删除".to_string(),
            },
        ],
    };

    let dialog_result = dialog::show_dialog(
        dialog::DialogType::Alert,
        dialog::DialogStyle::Website,
        dialog_info,
    )
    .await;

    if dialog_result.clicked_btn_id != "confirm" {
        tracing::info!("用户取消删除: {}", comic_name);
        return;
    }

    prepare_and_send_delete(target).await;
}

async fn handle_delete_source(target: DataTarget) {
    let source_name = target.item.name().to_string();

    let dialog_info = dialog::DialogInfo {
        title: format!("确认删除漫画源「{}」", source_name),
        content: "删除后需重新同步才能恢复，确定要删除吗？".to_string(),
        buttons: vec![
            dialog::DialogButton {
                id: "cancel".to_string(),
                primary: false,
                content: "取消".to_string(),
            },
            dialog::DialogButton {
                id: "confirm".to_string(),
                primary: true,
                content: "确认删除".to_string(),
            },
        ],
    };

    let dialog_result = dialog::show_dialog(
        dialog::DialogType::Alert,
        dialog::DialogStyle::Website,
        dialog_info,
    )
    .await;

    if dialog_result.clicked_btn_id != "confirm" {
        tracing::info!("用户取消删除漫画源: {}", source_name);
        return;
    }

    prepare_and_send_delete(target).await;
}

fn set_delete_phase(id: &str, phase: DeletePhase, message: String) {
    let timer_id = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        let Some(request) = state.deletes.get_mut(id) else { return; };
        if request.phase.final_result() { return; }
        request.phase = phase;
        request.message = message;
        request.deadline = None;
        request.timer_id.take()
    };
    if let Some(id) = timer_id { timer::clear_timer(id); }
    build::rerender_main_ui();
}

async fn prepare_and_send_delete(target: DataTarget) {
    let id = ui_state().write().unwrap_or_else(|p| p.into_inner()).deletes.begin(target.clone());
    let Some(id) = id else {
        show_app_data_status(StatusState::Error("已有删除请求等待确认，请先查询结果或重新读取核实。".into())).await;
        return;
    };
    let Some(device_addr) = prepare_data_delete(&target, &id).await else { return; };
    handshake::begin_wait(device_addr.clone(), delete_progress(&id), move |result| async move {
        match result {
            Err(message) => set_delete_phase(&id, DeletePhase::Failed, message),
            Ok(_) => send_delete_request(id, target, device_addr).await,
        }
    });
}

fn arm_delete_timeout(id: &str) {
    let generation = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        let Some(request) = state.deletes.get_mut(id) else { return; };
        request.wait_generation = request.wait_generation.wrapping_add(1);
        request.deadline = Some(Instant::now() + std::time::Duration::from_millis(DELETE_TIMEOUT_MS));
        request.wait_generation
    };
    let tid = timer::set_timeout(DELETE_TIMEOUT_MS, &format!("{DELETE_TIMEOUT_EVENT}{id}:{generation}"));
    let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
    if let Some(request) = state.deletes.get_mut(id) { request.timer_id = Some(tid); }
}

pub fn handle_delete_timeout(payload: &str) {
    let Some((id, generation)) = payload.rsplit_once(':') else { return; };
    let Ok(generation) = generation.parse::<u64>() else { return; };
    let request = ui_state().read().unwrap_or_else(|p| p.into_inner()).deletes.get(id).cloned();
    let Some(request) = request.filter(|r| r.wait_generation == generation && r.phase.busy()) else { return; };
    let now = Instant::now();
    if request.timed_out(generation, now) {
        set_delete_phase(id, DeletePhase::Unknown, "未收到设备最终结果，请查询原结果或重新读取核实。".into());
    } else if let Some(deadline) = request.deadline {
        if let Some(tid) = request.timer_id { timer::clear_timer(tid); }
        let remaining = deadline.saturating_duration_since(now).as_millis() as u64 + 1;
        let tid = timer::set_timeout(remaining, &format!("{DELETE_TIMEOUT_EVENT}{payload}"));
        if let Some(request) = ui_state().write().unwrap_or_else(|p| p.into_inner()).deletes.get_mut(id) { request.timer_id = Some(tid); }
    }
}

async fn send_delete_request(id: String, target: DataTarget, device_addr: String) {
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).deletes.get(&id)
        .is_none_or(|r| r.phase != DeletePhase::Preparing) { return; }
    refresh_data_connection().await;
    if !data_delete_valid(&target) {
        set_delete_phase(&id, DeletePhase::Failed, "列表或连接已变化，删除命令未发送。".into());
        return;
    }
    let negotiated = ui_state().read().unwrap_or_else(|p| p.into_inner()).watch_delete.clone().filter(|(addr, _)| *addr == device_addr);
    let message = if let Some((_, session)) = negotiated {
        let valid = {
            let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
            match &target.item {
                DataItem::Comic { id, .. } => !id.is_empty() && state.app_comics.iter().filter(|c| c.id == *id).count() == 1,
                DataItem::Source { key, .. } => !key.is_empty() && key != "using" && state.app_sources.iter().filter(|s| s.key == *key).count() == 1,
            }
        };
        if !valid { set_delete_phase(&id, DeletePhase::Failed, "目标缺少唯一 ID/key，请重新读取设备数据。".into()); return; }
        let message = {
            let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
            let Some(request) = state.deletes.get_mut(&id) else { return; };
            request.session = session;
            request.phase = DeletePhase::Waiting;
            request.message = "删除命令正在发送，等待设备实际文件及索引结果…".into();
            request.wire_message(None)
        };
        arm_delete_timeout(&id); // register before send; never await the result under BUSINESS_EVENTS
        message
    } else {
        let ambiguous = {
            let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
            match &target.item {
                DataItem::Comic { name, .. } => state.app_comics.iter().filter(|c| c.name == *name).count() != 1,
                DataItem::Source { name, .. } => state.app_sources.iter().filter(|s| s.name == *name || (!s.key.is_empty() && s.key == *name)).count() != 1,
            }
        };
        if ambiguous { set_delete_phase(&id, DeletePhase::Failed, "旧端删除协议按名称定位，同名条目请在设备端删除。".into()); return; }
        json!({"type": if matches!(target.item, DataItem::Comic { .. }) { "delete_comic" } else { "delete_source" }, "name":target.item.name()})
    };
    let modern = message["type"] == "delete_item";
    build::rerender_main_ui();
    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), message.to_string()).await {
        Ok(_) if !modern => set_delete_phase(&id, DeletePhase::LegacySent, "删除命令已发送，待刷新核实；请重新读取确认设备结果。".into()),
        Ok(_) => {},
        Err(error) => {
            tracing::warn!("删除发送异常: {:?}", error);
            set_delete_phase(&id, if modern { DeletePhase::Unknown } else { DeletePhase::LegacySent },
                "发送异常，设备结果未确认，请重新读取核实。".into());
        }
    }
}

async fn query_delete_result(id: &str) {
    let request = ui_state().read().unwrap_or_else(|p| p.into_inner()).deletes.get(id).cloned();
    let Some(request) = request.filter(|r| r.phase == DeletePhase::Unknown && !r.session.is_empty()) else { return; };
    refresh_data_connection().await;
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.checked_device.as_ref()
        .is_none_or(|device| device.addr != request.target.owner.addr) {
        set_delete_phase(id, DeletePhase::Unknown, "请先连接原目标设备，再查询删除结果。".into()); return;
    }
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.busy() { return; }
    set_delete_phase(id, DeletePhase::Querying, "正在连接原设备查询删除结果…".into());
    let addr = match handshake::prepare_launch(handshake::MIN_MANAGEMENT_VERSION, &delete_progress(id)).await {
        Ok(addr) if addr == request.target.owner.addr => addr,
        _ => { set_delete_phase(id, DeletePhase::Unknown, "无法连接原设备，请重新读取核实。".into()); return; }
    };
    let id = id.to_string();
    handshake::begin_wait(addr.clone(), delete_progress(&id), move |result| async move {
        if ui_state().read().unwrap_or_else(|p| p.into_inner()).deletes.get(&id)
            .is_none_or(|r| r.phase != DeletePhase::Querying) { return; }
        refresh_data_connection().await;
        let session = ui_state().read().unwrap_or_else(|p| p.into_inner()).watch_delete.clone()
            .filter(|(device, _)| *device == addr).map(|(_, session)| session);
        let connected = ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.checked_device.as_ref().is_some_and(|d| d.addr == addr);
        if result.is_err() || session.is_none() || !connected {
            set_delete_phase(&id, DeletePhase::Unknown, "原设备无法查询结果，请重新读取核实。".into()); return;
        }
        arm_delete_timeout(&id);
        let message = request.wire_message(session.as_deref()).to_string();
        if interconnect::send_qaic_message(addr, WATCH_APP_PKG_NAME.into(), message).await.is_err() {
            set_delete_phase(&id, DeletePhase::Unknown, "结果查询发送异常，请重新读取核实。".into());
        }
    });
}

/// 到有效进展的截止时间才检查，不每个封面片都clear/set宿主定时器。
async fn schedule_app_data_recv_timeout() {
    let (old, generation, remaining) = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = state.sync_receive.generation();
        (
            state.app_data_recv_timer_id.take(),
            generation,
            state.sync_receive.remaining(generation, Instant::now()),
        )
    };
    if let Some(t) = old {
        timer::clear_timer(t);
    }
    let Some(remaining) = remaining else { return; };
    let event = format!("{}{}", APP_DATA_RECV_TIMEOUT_EVENT, generation);
    let tid = timer::set_timeout(remaining.as_millis().max(1) as u64, &event);
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.sync_receive.generation() == generation && state.sync_receive.active() {
        state.app_data_recv_timer_id = Some(tid);
    } else {
        drop(state);
        timer::clear_timer(tid);
    }
}

async fn disarm_app_data_recv_timeout() -> u64 {
    let (old, generation) = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.sync_receive.finish();
        state.http_data_sync = None;
        (state.app_data_recv_timer_id.take(), state.sync_receive.generation())
    };
    if let Some(t) = old {
        timer::clear_timer(t);
    }
    generation
}

/// 只有无有效进展20s/30s才结束；旧会话/旧阶段的定时器不能中止新接收。
pub async fn handle_app_data_recv_timeout(generation: u64) {
        let cover_phase = {
            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.sync_receive.remaining(generation, Instant::now()) {
                None => return,
                Some(remaining) if !remaining.is_zero() => {
                    drop(state);
                    schedule_app_data_recv_timeout().await;
                    return;
                }
                _ => {}
            }
            let covers = state.sync_receive.cover_phase();
            state.cover_chunk_buffers.clear();
            state.pending_covers.clear();
            if let Some(frontier) = state.sync_recv.as_mut() {
                frontier.clear_pending();
            }
            covers
        };
        let finished = disarm_app_data_recv_timeout().await;
        if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != finished {
            return;
        }
        fail_library_sync(if cover_phase { "封面接收超时，请重试。" } else { "列表接收超时，请重试。" }.into()).await;
}

async fn handle_fetch_app_data() {
    let operation = disarm_app_data_recv_timeout().await;
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != operation { return; }
    refresh_data_connection().await;
    let target = ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.checked_device.clone();
    let Some(target) = target else {
        ui_state().write().unwrap_or_else(|p| p.into_inner()).data_browser.begin_request(None);
        fail_library_sync("没有已连接的设备，请检查手表连接。".into()).await;
        return;
    };
    ui_state().write().unwrap_or_else(|p| p.into_inner()).data_browser.begin(target.clone());
    show_app_data_status(StatusState::Processing("正在获取快应用数据...".to_string())).await;

    // Clear transport buffers now. The visible list switches on the first valid
    // message, not on the header: legacy interconnect can deliver metadata first.
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sync_receive.generation() != operation { return; }
        // Keep the previous visible snapshot until the first valid incoming message.
        state.cover_chunk_buffers.clear();
        // 滑窗接收会话重置：新一轮 gseq 从 0 开始
        state.sync_recv = None;
        state.pending_covers.clear();
    }

    let progress = move |message: String| {
        let current = ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() == operation;
        if current { app_data_progress()(message); }
    };
    let device_addr = match handshake::prepare_launch(handshake::MIN_MANAGEMENT_VERSION, &progress).await {
        Ok(addr) if addr == target.addr => addr,
        Ok(_) => {
            fail_library_sync("准备期间目标设备已变化，请重新读取。".into()).await;
            return;
        }
        Err(msg) => {
            if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != operation { return; }
            fail_library_sync(msg).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成（pong 到达）后才发 request_data，
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != operation { return; }
    // 保证快应用确实已启动并能收到消息
    handshake::begin_wait(
        device_addr.clone(),
        move |message| {
            let current = ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() == operation;
            if current { app_data_progress()(message); }
        },
        move |result| async move {
            if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != operation { return; }
            match result {
            Err(msg) => {
                fail_library_sync(msg).await;
            }
            Ok(_) => {
                fetch_app_data_send_request(device_addr, operation).await;
            }
            }
        },
    );
}

async fn fetch_app_data_send_request(device_addr: String, operation: u64) {
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != operation { return; }
    refresh_data_connection().await;
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.checked_device.as_ref()
        .is_none_or(|device| device.addr != device_addr) {
        fail_library_sync("握手后连接已变化，读取请求未发送。".into()).await;
        return;
    }
    let use_http = ui_state().read().unwrap_or_else(|p| p.into_inner()).watch_http_data_sync;
    tracing::info!("获取快应用数据请求: use_http={}, addr={}", use_http, device_addr);
    let http = if use_http {
        crate::http_server::start().await;
        crate::http_server::data_sync_config()
    } else { None };
    let session = {
        let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
        (state.watch_sync_session || http.is_some()).then(|| format!("sync{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)))
    };
    let mut request_msg = json!({
        "type": "request_data"
    });
    if let Some(ref session) = session {
        request_msg["session"] = json!(session);
    }
    if let Some(ref http) = http {
        request_msg["http"] = http.clone();
    }

    let request_str = match serde_json::to_string(&request_msg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            fail_library_sync("请求序列化失败。".to_string()).await;
            return;
        }
    };

    show_app_data_status(StatusState::Processing("等待手表返回数据...".to_string())).await;
    let generation = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if state.sync_receive.generation() != operation { return; }
        state.sync_receive.start(Instant::now(), session);
        state.http_data_sync = http.as_ref().map(|_| crate::http_data_sync::HttpDataSync::default());
        state.sync_receive.generation()
    };
    schedule_app_data_recv_timeout().await;
    if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != generation { return; }
    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), request_str).await {
        Ok(_) => {
            tracing::info!("数据请求已发送，等待手表回复...");
        }
        Err(e) => {
            {
                let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
                if state.sync_receive.generation() != generation || state.data_browser.incoming { return; }
            }
            let finished = disarm_app_data_recv_timeout().await;
            if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != finished { return; }
            tracing::error!("发送数据请求失败: {:?}", e);
            fail_library_sync("发送请求失败，请检查连接。".to_string()).await;
        }
    }
}

/// 发送 app_data 消息的 ACK 确认
/// 快应用端等待此 ACK 后继续发送下一个，确保安卓端严格顺序；
/// 滑窗会话（windowed=true）由 sync_ack 统一累计确认，逐条 ACK 被抑制
/// （省 BLE 带宽、避免干扰窗口；不能在此读 ui_state——调用方可能正持锁）
async fn send_app_data_ack(device_addr: &str, index: usize, windowed: bool) {
    if windowed {
        return;
    }

    let ack_msg = json!({
        "type": "app_data_ack",
        "index": index,
    });

    if let Ok(ack_str) = serde_json::to_string(&ack_msg) {
        let _ = interconnect::send_qaic_message(device_addr.into(), WATCH_APP_PKG_NAME.into(), ack_str).await;
    }
}

pub async fn handle_interconnect_message(payload: &str) {
    tracing::debug!("收到互联消息，长度={}", payload.len());

    // 设备地址懒获取：只有需要回 ACK 的消息分支才查询，
    // 避免封面分片等高频消息每条都做一次设备列表 FFI
    let addr_cell = std::cell::OnceCell::new();

    let outer = match serde_json::from_str::<Value>(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("无法解析互联消息: {}", e);
            return;
        }
    };

    let source_addr = ["deviceAddr", "device_addr", "addr"].iter().find_map(|key|
        outer.get(key).and_then(Value::as_str).map(str::to_string));
    let parsed = if let Some(pt) = outer.get("payloadText") {
        if let Some(s) = pt.as_str() {
            tracing::info!("从 payloadText 字符串解包数据");
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| outer.clone())
        } else if pt.is_object() {
            tracing::info!("从 payloadText 对象解包数据");
            pt.clone()
        } else {
            outer.clone()
        }
    } else if let Some(data) = outer.get("data") {
        if let Some(s) = data.as_str() {
            tracing::info!("从 data 字符串解包数据");
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| outer.clone())
        } else if data.is_object() {
            tracing::info!("从 data 对象解包数据");
            data.clone()
        } else {
            outer.clone()
        }
    } else {
        tracing::info!("直接使用原始 payload 对象");
        outer
    };

    if parsed.get("type").and_then(Value::as_str) == Some("delete_result") {
        handle_delete_result(&parsed, source_addr.as_deref());
        return;
    }
    if parsed.get("type").and_then(Value::as_str) == Some("hs_pong") && let Some(source) = source_addr.as_deref() {
        if get_addr(&addr_cell).await.as_deref() != Some(source) { return; }
    }

    // 方向 B 滑窗接收：带 gseq 的同步帧先经 RecvFrontier 乱序缓存、按序还原后再派发；
    // 每帧（含重复帧）都回累计 ACK（sync_ack）。旧快应用帧无 gseq，直接走旧路径。
    if let Some(gseq) = parsed.get("gseq").and_then(|v| v.as_u64()) {
        let Ok(gseq) = usize::try_from(gseq) else { return; };
        let (ready, ack, session) = {
            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.sync_receive.matches_session(parsed.get("session").and_then(Value::as_str)) {
                return;
            }
            let session = state.sync_receive.session().map(str::to_string);
            let active = state.sync_receive.active();
            // 收尾后仍回旧序号ACK，恢复丢失的最终ACK；不接受新片复活已结束会话。
            if !active && state.sync_recv.is_none() { return; }
            let frontier = state.sync_recv.get_or_insert_with(RecvFrontier::new);
            let (ready, ack, _dup) = if active && valid_sync_message(&parsed) {
                frontier.insert(gseq, parsed)
            } else {
                (Vec::new(), frontier.ack(), true)
            };
            (ready, ack, session)
        };
        if let Some(ref addr) = get_addr(&addr_cell).await {
            let mut ack_msg = json!({
                "type": "sync_ack",
                "ack": ack,
            });
            if let Some(session) = session { ack_msg["session"] = json!(session); }
            if let Ok(ack_str) = serde_json::to_string(&ack_msg) {
                let _ = interconnect::send_qaic_message(addr.clone(), WATCH_APP_PKG_NAME.into(), ack_str).await;
            }
        }
        for msg in &ready {
            dispatch_sync_message(msg, true, &addr_cell).await;
        }
        return;
    }

    dispatch_sync_message(&parsed, false, &addr_cell).await;
}

async fn get_addr(cache: &std::cell::OnceCell<Option<String>>) -> Option<String> {
    if let Some(addr) = cache.get() {
        return addr.clone();
    }
    let devices = device::get_connected_device_list().await;
    let addr = devices.first().map(|device| device.addr.clone());
    let _ = cache.set(addr.clone());
    addr
}

fn sync_message_type(msg_type: &str) -> bool {
    matches!(msg_type, "app_data_header" | "app_data_comic" | "app_data_source" |
        "app_data_done" | "cover_data_chunk" | "cover_done")
}

fn valid_sync_message(msg: &Value) -> bool {
    match msg.get("type").and_then(Value::as_str) {
        Some("cover_data_chunk") => {
            let index = msg.get("index").and_then(Value::as_u64);
            let total = msg.get("total").and_then(Value::as_u64);
            matches!((index, total), (Some(index), Some(total)) if total > 0 && total <= MAX_COVER_CHUNKS as u64 && index < total)
                && msg.get("name").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
                && msg.get("data").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
        }
        Some("app_data_comic") => msg.get("comic").and_then(|v| v.get("name"))
            .and_then(Value::as_str).is_some_and(|name| !name.is_empty()) &&
            msg.get("index").and_then(Value::as_u64).unwrap_or(0) < 10_000,
        Some("app_data_source") => msg.get("source").and_then(|v| v.get("name"))
            .and_then(Value::as_str).is_some_and(|name| !name.is_empty()) &&
            msg.get("index").and_then(Value::as_u64).unwrap_or(0) < 10_000,
        Some("app_data_header") => ["comic_count", "source_count"].iter().all(|key|
            msg.get(key).and_then(Value::as_u64).is_some_and(|count| count <= 10_000)),
        Some(kind) => sync_message_type(kind),
        None => false,
    }
}

/// 同步消息派发（type 命名空间）。新旧协议共用：
/// 旧快应用消息由 handle_interconnect_message 直接调用（windowed=false）；
/// 滑窗会话帧经 RecvFrontier 按序还原后逐帧调用（windowed=true，抑制逐条 ACK）
async fn dispatch_sync_message(parsed: &Value, windowed: bool, addr_cell: &std::cell::OnceCell<Option<String>>) {
    let msg_type = parsed.get("type").and_then(|v| v.as_str());
    if msg_type.is_some_and(sync_message_type) {
        // 安卓互联可能乱序：首个合法列表头也能确认正式数据前的 HTTP 回退。
        if msg_type == Some("app_data_header") {
            crate::http_data_sync::fallback(parsed.get("session").and_then(Value::as_str).unwrap_or(""));
        }
        let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
        if state.http_data_sync.is_some() || !state.sync_receive.active() || !valid_sync_message(parsed)
            || !state.sync_receive.matches_session(parsed.get("session").and_then(Value::as_str))
            || (!windowed && state.sync_receive.session().is_some()) { return; }
        drop(state);
        ui_state().write().unwrap_or_else(|p| p.into_inner()).accept_library_data();
    }

    match msg_type {
        Some("app_data_header") => {
            let comic_count = parsed
                .get("comic_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let source_count = parsed
                .get("source_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            tracing::info!(
                "收到数据头: comic_count={}, source_count={}",
                comic_count,
                source_count
            );

            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let fresh = state.app_comic_count.is_none() && state.app_source_count.is_none();
            state.app_comic_count = Some(comic_count);
            state.app_source_count = Some(source_count);
            state.update_library_completeness();
            if fresh { state.sync_receive.progress(Instant::now()); }
            // 安卓端消息可能乱序：comic/source/封面分片可能先于 header 到达，
            // 这里只按需扩容（不重建、不清缓冲区），数据按 index/名字落位
            if state.app_comics.len() < comic_count {
                state.app_comics.resize(comic_count, ComicInfo::default());
            }
            if state.app_sources.len() < source_count {
                state
                    .app_sources
                    .resize(source_count, SourceInfo::default());
            }
            state.app_data_status = StatusState::Processing("接收中...".to_string());
            drop(state);

            // 发送 ACK 确认，让快应用继续发下一个
            // 快应用 msgIndex = 0，ACK 后 msgIndex++ 变成 1，所以 ACK 序号 = 0
            if !windowed && let Some(ref addr) = get_addr(addr_cell).await {
                send_app_data_ack(addr, 0, false).await;
            }
        }
        Some("app_data_comic") => {
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let comic = parsed.get("comic");

            if let Some(comic) = comic {
                let mut info = ComicInfo {
                    id: comic.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: comic
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    page_count: comic
                        .get("page_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize,
                    chapters: comic.get("chapters").and_then(|v| v.as_u64()).unwrap_or(1) as usize,
                    cover_base64: String::new(),
                };
                let root_id = {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    // 乱序容忍：header 未到时按需扩容
                    if index >= state.app_comics.len() {
                        state.app_comics.resize(index + 1, ComicInfo::default());
                    }
                    // 封面先于漫画信息拼完的，补挂（取出并丢弃时间戳）
                    if let Some((cover, _ts)) = state.pending_covers.remove(&info.name) {
                        info.cover_base64 = cover;
                    }
                    state.app_comics[index] = info;
                    if state.sync_comics_seen.insert(index) { state.sync_receive.progress(Instant::now()); }
                    state.update_library_completeness();
                    state.data_browser.covers_received = state.app_comics.iter().filter(|c| !c.cover_base64.is_empty()).count();
                    // 接收进度提示
                    state.app_data_status = StatusState::Processing(format!(
                        "接收漫画 {}/{}",
                        state.sync_comics_seen.len(),
                        state.app_comic_count.unwrap_or(0)
                    ));
                    state.root_element_id.clone()
                };
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui::render(&root_id, ui);
                    build::render_comic_data_card(COMIC_DATA_CARD_ID);
                }
            }
            // 发送 ACK 确认，让快应用继续发下一个
            // 当前消息在快应用的 msgIndex = (index + 1)，所以 ACK 序号 = (index + 1)
            // 因为 header 占用 msgIndex 0，所以 comic 从 1 开始
            if !windowed && let Some(ref addr) = get_addr(addr_cell).await {
                send_app_data_ack(addr, 1 + index, false).await;
            }
        }
        Some("app_data_source") => {
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let source = parsed.get("source");

            if let Some(source) = source {
                let info = SourceInfo {
                    key: source.get("key").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: source
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    api_url: source
                        .get("apiUrl")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                };
                let root_id = {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if index >= state.app_sources.len() {
                        state.app_sources.resize(index + 1, SourceInfo::default());
                    }
                    state.app_sources[index] = info;
                    if state.sync_sources_seen.insert(index) { state.sync_receive.progress(Instant::now()); }
                    state.update_library_completeness();
                    // 接收进度提示
                    state.app_data_status = StatusState::Processing(format!(
                        "接收漫画源 {}/{}",
                        state.sync_sources_seen.len(),
                        state.app_source_count.unwrap_or(0)
                    ));
                    state.root_element_id.clone()
                };
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui::render(&root_id, ui);
                }
            }
            // 发送 ACK 确认，让快应用继续发下一个
            // header 占 1 + 前面 comic 占 N 个，所以当前 msgIndex = 1 + comics.len() + index
            // 从 state 获取 comic_count
            let total_comics = {
                let state = ui_state()
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.app_comic_count.unwrap_or(0)
            };
            let msg_index = 1 + total_comics + index;
            if !windowed && let Some(ref addr) = get_addr(addr_cell).await {
                send_app_data_ack(addr, msg_index, false).await;
            }
        }
        Some("app_data_done") => {
            tracing::info!("列表数据接收完成，渲染 UI");

            let (comic_count, source_count, root_id, fresh_done, finish_empty) = {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let comic_count = state.app_comic_count.unwrap_or(0);
                let fresh_done = !state.sync_receive.cover_phase();
                state.data_browser.lists_done = true;
                state.update_library_completeness();
                let finish_empty = comic_count == 0 && state.data_browser.lists_complete && !windowed;
                state.data_browser.phase = DataPhase::Covers;
                // 有漫画时还有封面要收：进入封面接收阶段；
                // 没有漫画则整个拉取流程结束
                state.app_data_status = if !finish_empty {
                    if fresh_done { state.sync_receive.covers(Instant::now()); }
                    StatusState::Processing(if comic_count > 0 { "正在接收封面..." } else { "正在完成同步..." }.to_string())
                } else {
                    state.finish_library_sync(None);
                    StatusState::Success("数据获取成功！".to_string())
                };
                (
                    comic_count,
                    state.app_source_count.unwrap_or(0),
                    state.root_element_id.clone(),
                    fresh_done,
                    finish_empty,
                )
            };

            // 封面阶段重新武装整体超时；无封面则整个流程结束，解除超时
            if !finish_empty {
                if fresh_done { schedule_app_data_recv_timeout().await; }
            } else {
                disarm_app_data_recv_timeout().await;
            }

            if let Some(root_id) = root_id {
                let ui = build_main_ui();
                psys_host::ui::render(&root_id, ui);
            }

            build::render_comic_data_card(COMIC_DATA_CARD_ID);

            // done 消息也要 ACK，表示可以开始发封面
            // msgIndex = 1 + comic_count + source_count = done 的位置
            let msg_index = 1 + comic_count + source_count;
            if !windowed && let Some(ref addr) = get_addr(addr_cell).await {
                send_app_data_ack(addr, msg_index, false).await;
            }
        }
        Some("cover_done") => {
            // 快应用已全部封面发送完毕（且每张都已被 ACK）：整个拉取流程结束
            tracing::info!("封面接收完成");
            let incomplete = {
                let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
                let incomplete = !state.cover_chunk_buffers.is_empty();
                state.cover_chunk_buffers.clear();
                state.pending_covers.clear();
                if let Some(frontier) = state.sync_recv.as_mut() { frontier.clear_pending(); }
                state.finish_library_sync(None);
                incomplete
            };
                let finished = disarm_app_data_recv_timeout().await;
                if ui_state().read().unwrap_or_else(|p| p.into_inner()).sync_receive.generation() != finished {
                    return;
                }
                let lists_complete = ui_state().read().unwrap_or_else(|p| p.into_inner()).data_browser.lists_complete;
                show_app_data_status(if !lists_complete {
                    StatusState::Error("列表接收不完整，请重新读取。".into())
                } else { StatusState::Success(if incomplete {
                    "数据获取完成（封面可能不完整）"
                } else { "数据获取成功！" }.to_string()) }).await;
            build::render_comic_data_card(COMIC_DATA_CARD_ID);
        }
        Some("cover_data_chunk") => {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let total = parsed.get("total").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let data = parsed.get("data").and_then(|v| v.as_str()).unwrap_or("");

            if name.is_empty() || data.is_empty() {
                return;
            }

            tracing::info!(
                "收到封面切片: name={}, {}/{}, len={}",
                name,
                index + 1,
                total,
                data.len()
            );

            let (done, root_id) = {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());

                if index == 0 && state.cover_chunk_buffers.get(name).is_some_and(|buf| buf.total != total) {
                    state.cover_chunk_buffers.remove(name);
                }
                let buf = state.cover_chunk_buffers.entry(name.to_string())
                    .or_insert_with(|| CoverChunks::new(total).unwrap());
                let (cover, fresh) = buf.insert(index, total, data);
                if fresh { state.sync_receive.progress(Instant::now()); }
                if let Some(cover) = cover {
                    // 漫画信息可能因乱序尚未到达：找不到时暂存，
                    // 等 app_data_comic 到达时补挂，避免封面被丢弃
                    // 记录当前时间戳，超过 30 秒未补挂的会被清理
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if let Some(comic) = state.app_comics.iter_mut().find(|c| c.name == name) {
                        comic.cover_base64 = cover;
                    } else {
                        state.pending_covers.insert(name.to_string(), (cover, now));
                    }
                    state.cover_chunk_buffers.remove(name);
                    state.data_browser.covers_received = state.app_comics.iter().filter(|c| !c.cover_base64.is_empty()).count();
                    // 无论挂载还是暂存都算拼完，都需要回 cover_ack
                    (true, state.root_element_id.clone())
                } else {
                    (false, state.root_element_id.clone())
                }
            };

            if done {
                // 回 cover_ack 通知快应用发下一张封面（无论挂载还是暂存都要回）；
                // 滑窗会话由 sync_ack 统一累计确认，抑制逐张 cover_ack
                if !windowed {
                    if let Some(ref addr) = get_addr(addr_cell).await {
                        let ack = json!({
                            "type": "cover_ack",
                            "name": name,
                        });
                        if let Ok(ack_str) = serde_json::to_string(&ack) {
                                let _ =
                                    interconnect::send_qaic_message(addr.clone(), WATCH_APP_PKG_NAME.into(), ack_str)
                                        .await;
                        }
                    }
                }
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui::render(&root_id, ui);
                }
                build::render_comic_data_card(COMIC_DATA_CARD_ID);
            }
        }
        Some("import_header_ack") => {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");

            // 快应用确认收到头部：开始发分片。重复 ACK（如重发头部导致）直接忽略
            let start = {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match state.upload_session.as_mut() {
                    Some(s) if s.comic_name == name && !s.header_acked => {
                        s.header_acked = true;
                        true
                    }
                    _ => false,
                }
            };

            if start {
                tracing::info!("头部已确认: name={}，开始发送分片", name);
                    disarm_header_timeout().await;
                    let windowed = {
                        let state = ui_state()
                            .read()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state
                            .upload_session
                            .as_ref()
                            .map(|s| s.windowed.is_some())
                            .unwrap_or(false)
                    };
                    if windowed {
                        pump_upload_window().await;
                    } else {
                        send_next_chunk().await;
                    }
            }
        }
        Some("hs_pong") => {
            let session = parsed
                .get("session")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !handshake::accepts_pong(&session) { return; }
            let settings = parsed
                .get("settings")
                .map(WatchSettings::from_json)
                .unwrap_or_default();
            tracing::info!("收到握手应答: session={}, settings={:?}", session, settings);

            if let Some(ref addr) = get_addr(addr_cell).await {
                if !handshake::accepts_pong_from(addr, &session) { return; }
                {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.watch_settings = Some(settings);
                    // 快应用能力协商：导入接收窗口（无 caps 为旧版，上传走逐片停等）
                    state.watch_import_window = parsed
                        .get("caps")
                        .and_then(|c| c.get("importWindow"))
                        .and_then(|v| v.as_u64())
                        .map(|n| (n as usize).clamp(1, 16));
                    state.watch_sync_session = parsed.get("caps").and_then(|c| c.get("syncSession"))
                        .and_then(Value::as_bool).unwrap_or(false);
                    state.watch_http_import = parsed.get("caps").and_then(|c| c.get("httpImport"))
                        .and_then(Value::as_bool).unwrap_or(false);
                    state.watch_http_data_sync = parsed.get("caps").and_then(|c| c.get("httpDataSync"))
                        .and_then(Value::as_u64) == Some(1);
                    state.watch_delete = (parsed.get("caps").and_then(|c| c.get("deleteProtocol"))
                        .and_then(Value::as_u64) == Some(1) && !session.is_empty())
                        .then(|| (addr.clone(), session.clone()));
                }
                // 完成挂起的握手会话，锁外直接等待业务续体。
                handshake::handle_hs_pong(addr, &session, &parsed).await;
            }
        }
        Some("import_chunk_ack") => {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");

            // 窗口会话：ack 字段为累计前沿（下一个仍缺失的 gseq）
            if let Some(ack) = parsed.get("ack").and_then(|v| v.as_u64()) {
                handle_windowed_chunk_ack(name, ack as usize).await;
                return;
            }

            let file = parsed.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            // 只接受与当前在途分片匹配的 ACK，忽略陈旧/重复 ACK，
            // 否则旧 ACK 会错误推进会话导致跳片、传输错位
            let advance = {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match state.upload_session.as_mut() {
                    Some(s) if s.comic_name == name => match s.awaiting {
                        Some((afi, aci))
                            if afi < s.all_files.len()
                                && s.all_files[afi].0 == file
                                && aci == index =>
                        {
                            s.awaiting = None;
                            s.retry_count = 0;
                            true
                        }
                        _ => {
                            tracing::warn!(
                                "忽略不匹配的分片 ACK: file={}, index={}（非当前在途分片）",
                                file,
                                index
                            );
                            false
                        }
                    },
                    _ => false,
                }
            };

            if advance {
                tracing::info!(
                    "收到 chunk ACK: name={}, file={}, index={}",
                    name,
                    file,
                    index
                );
                    disarm_ack_timeout().await;
                    send_next_chunk().await;
            }
        }
        Some("data_sync_transport") => {
            let reason = parsed.get("reason").and_then(Value::as_str).unwrap_or("未知原因");
            tracing::warn!("手表回退至互联传输: {}", reason);
            show_app_data_status(StatusState::Processing(format!("HTTP未通 ({})，回退互联传输...", reason))).await;
            if parsed.get("transport").and_then(Value::as_str) == Some("interconnect") {
                crate::http_data_sync::fallback(parsed.get("session").and_then(Value::as_str).unwrap_or(""));
            }
        }
        Some("data_sync_result") => {
            let current = {
                let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
                state.http_data_sync.is_some() && state.sync_receive.active() &&
                    state.sync_receive.matches_session(parsed.get("session").and_then(Value::as_str))
            };
            if current && parsed.get("success").and_then(Value::as_bool) == Some(false) {
                disarm_app_data_recv_timeout().await;
                show_app_data_status(StatusState::Error(format!("HTTP 数据同步失败：{}",
                    parsed.get("error").and_then(Value::as_str).unwrap_or("请重试")))).await;
            }
        }
        Some("gateway_bind_result") => {
            if !crate::http_server::handle_bind_result(&parsed).await { return; }
            let pending = PENDING_HTTP_IMPORT.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some((addr, settings, chapter)) = pending {
                if crate::http_server::status().bound {
                    start_http_import_task(addr, settings, chapter).await;
                } else {
                    let error = crate::http_server::status().bind_status.unwrap_or_else(|| "本地连接失败".into());
                    show_upload_status(StatusState::Error(error)).await;
                }
            }
        }
        _ => {
            tracing::info!("收到未处理的消息类型: {:?}", msg_type);
        }
    }
}

fn handle_delete_result(value: &Value, source_addr: Option<&str>) {
    let timer_id = {
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        let Some(id) = state.deletes.accept_result(value, source_addr) else { return; };
        let request = state.deletes.get(&id).unwrap().clone();
        if request.phase == DeletePhase::Success && !state.apply_delete_success(&request.target) {
            state.deletes.get_mut(&id).unwrap().message.push_str("；原请求已完成，当前列表未直接调整，请重新读取核实");
        }
        state.deletes.get_mut(&id).unwrap().timer_id.take()
    };
    if let Some(id) = timer_id { timer::clear_timer(id); }
    build::rerender_main_ui();
    build::render_comic_data_card(COMIC_DATA_CARD_ID);
}

pub async fn show_app_data_status(status: StatusState) {
    // V4 Timer 为同步接口；先更新状态并释放UI锁再操作定时器。
    let (root_id, old_timer) = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.app_data_status = status.clone();
        (state.root_element_id.clone(), state.app_data_timer_id.take())
    };
    if let Some(old_timer) = old_timer {
        timer::clear_timer(old_timer);
    }
    if matches!(status, StatusState::Success(_) | StatusState::Error(_)) {
        let new_timer = timer::set_timeout(5000, HIDE_APP_DATA_STATUS_EVENT);
        let mut state = ui_state().write().unwrap_or_else(|p| p.into_inner());
        if state.app_data_status == status {
            state.app_data_timer_id = Some(new_timer);
        } else {
            drop(state);
            timer::clear_timer(new_timer);
        }
    }

    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

pub fn hide_app_data_status() {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(state.app_data_status, StatusState::Processing(_)) { return; }
        state.app_data_status = StatusState::Default;
        state.app_data_timer_id = None;
        root_id = state.root_element_id.clone();
    }
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui::render(&root_id, ui);
    }
}

#[cfg(test)]
mod sync_message_tests {
    use super::*;

    #[test]
    fn cover_validation_rejects_invalid_slots_before_frontier_ack_or_allocation() {
        let valid = json!({"type": "cover_data_chunk", "name": "book", "index": 0, "total": 2, "data": "base64"});
        assert!(valid_sync_message(&valid));
        for (key, value) in [
            ("index", json!(2)), ("index", json!(-1)), ("total", json!(0)),
            ("total", json!(MAX_COVER_CHUNKS + 1)), ("name", json!("")), ("data", json!("")),
        ] {
            let mut malformed = valid.clone();
            malformed[key] = value;
            assert!(!valid_sync_message(&malformed));
        }
    }

    #[test]
    fn non_sync_and_malformed_metadata_cannot_renew_the_receive_watchdog() {
        assert!(!valid_sync_message(&json!({"type": "cookie"})));
        assert!(!valid_sync_message(&json!({"type": "app_data_comic", "comic": null})));
        assert!(!valid_sync_message(&json!({"type": "app_data_source", "source": "bad"})));
        assert!(valid_sync_message(&json!({"type": "app_data_comic", "comic": {"name": "book"}})));
        assert!(valid_sync_message(&json!({"type": "app_data_done"})));
        assert!(valid_sync_message(&json!({"type": "cover_done"})));
    }
}
