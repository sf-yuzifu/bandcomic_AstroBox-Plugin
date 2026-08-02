use super::COMIC_DATA_CARD_ID;
use super::handshake;
use super::state::*;
use crate::astrobox::psys_host::{self, device, dialog, interconnect, timer};
use crate::network::{fetch_source_config, fetch_source_name};
use image::ImageEncoder;
use serde_json::{Value, json};

use super::build::{self, build_main_ui};
use super::message::{hide_status, show_status};

pub fn ui_event_processor(
    event_type: crate::exports::astrobox::psys_plugin::event_v3::Event,
    event_id: &str,
    event_payload: &str,
) {
    tracing::debug!(
        "UI 事件: id={}, type={:?}, payload={}",
        event_id,
        event_type,
        event_payload
    );

    match event_id {
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
                    handle_domain_blur(text.to_string());
                }
            }
        }
        COOKIE_INPUT_EVENT => {
            if let Ok(value) = serde_json::from_str::<Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    tracing::debug!("Cookie 输入变化: {}", text);
                    handle_cookie_input(text.to_string());
                }
            }
        }
        SYNC_BUTTON_EVENT => {
            tracing::info!("同步按钮被点击");
            wit_bindgen::block_on(handle_sync());
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
            }
            switch_tab(TabPage::Upload);
        }
        UPLOAD_MODE_MULTI_EVENT => {
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.upload_mode = UploadMode::Multi;
            }
            switch_tab(TabPage::Upload);
        }
        UPLOAD_PICK_FILES_EVENT => {
            tracing::info!("选择文件按钮被点击");
            wit_bindgen::block_on(handle_pick_files());
        }
        UPLOAD_START_EVENT => {
            tracing::info!("上传按钮被点击");
            wit_bindgen::block_on(handle_upload_start());
        }
        UPLOAD_CLEAR_EVENT => {
            tracing::info!("清空列表按钮被点击");
            handle_upload_clear();
        }
        UPLOAD_ADD_CHAPTER_EVENT => {
            tracing::info!("添加章节按钮被点击");
            handle_add_chapter();
        }
        UPLOAD_PICK_COVER_EVENT => {
            tracing::info!("封面选择按钮被点击");
            wit_bindgen::block_on(handle_upload_pick_cover());
        }
        UPLOAD_PICK_MULTI_COVER_EVENT => {
            tracing::info!("多章节封面选择按钮被点击");
            wit_bindgen::block_on(handle_multi_cover());
        }
        TAB_SYNC_EVENT => {
            switch_tab(TabPage::Sync);
        }
        TAB_DATA_EVENT => {
            switch_tab(TabPage::Data);
        }
        TAB_UPLOAD_EVENT => {
            switch_tab(TabPage::Upload);
        }
        FETCH_APP_DATA_EVENT => {
            tracing::info!("获取快应用数据按钮被点击");
            wit_bindgen::block_on(handle_fetch_app_data());
        }
        _ => {
            if let Some(index_str) = event_id.strip_prefix(UPLOAD_MOVE_UP_PREFIX) {
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
                    wit_bindgen::block_on(handle_chapter_pick_files(chapter_index));
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_UPLOAD_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    tracing::info!("章节{}上传", chapter_index);
                    wit_bindgen::block_on(handle_chapter_upload(chapter_index));
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_CLEAR_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    handle_chapter_clear(chapter_index);
                }
            } else if let Some(index_str) = event_id.strip_prefix(CHAPTER_DELETE_PREFIX) {
                if let Ok(chapter_index) = index_str.parse::<usize>() {
                    handle_chapter_delete_chapter(chapter_index);
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
                if let Ok(index) = index_str.parse::<usize>() {
                    tracing::info!("删除漫画按钮被点击: index={}", index);
                    wit_bindgen::block_on(handle_delete_comic(index));
                }
            } else if let Some(index_str) = event_id.strip_prefix(DELETE_SOURCE_PREFIX) {
                if let Ok(index) = index_str.parse::<usize>() {
                    tracing::info!("删除漫画源按钮被点击: index={}", index);
                    wit_bindgen::block_on(handle_delete_source(index));
                }
            }
        }
    }
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
        psys_host::ui_v3::render(&root_id, ui);
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
        psys_host::ui_v3::render(&root_id, ui);
    }
}

async fn show_upload_status(status: StatusState) {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(timer_id) = state.upload_status_timer_id {
            let _ = timer::clear_timer(timer_id).await;
        }

        state.upload_status = status.clone();

        if matches!(&status, StatusState::Success(_) | StatusState::Error(_)) {
            let timer_id = timer::set_timeout(5000, HIDE_UPLOAD_STATUS_EVENT).await;
            state.upload_status_timer_id = Some(timer_id);
        }

        root_id = state.root_element_id.clone();
    }

    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui_v3::render(&root_id, ui);
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
        psys_host::ui_v3::render(&root_id, ui);
    }
}

async fn handle_pick_files() {
    let extensions = vec![
        "jpg".to_string(),
        "jpeg".to_string(),
        "png".to_string(),
        "webp".to_string(),
        "bmp".to_string(),
        "gif".to_string(),
    ];

    let filter = psys_host::dialog::FilterConfig {
        multiple: true,
        extensions,
        default_directory: String::new(),
        default_file_name: String::new(),
    };

    let pick_config = psys_host::dialog::PickConfig {
        read: true,
        copy_to: None,
    };

    let result = psys_host::dialog::pick_file(&pick_config, &filter).await;

    if result.data.is_empty() {
        tracing::info!("用户取消了文件选择");
        return;
    }

    let name_input = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_comic_name_input.clone()
    };

    let comic_name = if name_input.trim().is_empty() {
        let name_without_ext = result.name.rsplit('.').nth(1).unwrap_or(&result.name);
        name_without_ext.to_string()
    } else {
        name_input.trim().to_string()
    };

    let thumbnail = resize_to_width(&result.data, THUMBNAIL_WIDTH);
    let compressed = resize_to_width(&result.data, MASTER_WIDTH);

    let compressed_len = compressed.len();

    let file = UploadFile {
        name: result.name.clone(),
        data: compressed,
        size: compressed_len,
        original_size: result.data.len(),
        thumbnail,
    };

    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match state.upload_mode {
            UploadMode::Single => {
                if state.upload_comic_name_input.trim().is_empty() {
                    state.upload_comic_name_input = comic_name.clone();
                }
                // first file becomes cover, rest go to pages
                let mut found = false;
                let file_name = file.name.clone();
                for item in state.upload_items.iter_mut() {
                    if item.comic_name == comic_name {
                        item.files.push(file.clone());
                        found = true;
                        break;
                    }
                }
                if !found {
                    let display_name = if state.upload_comic_name_input.trim().is_empty() {
                        file_name
                    } else {
                        state.upload_comic_name_input.trim().to_string()
                    };
                    let mut item = UploadItem {
                        comic_name: display_name,
                        cover: None,
                        files: vec![file],
                    };
                    // first file becomes cover
                    if !item.files.is_empty() {
                        item.cover = Some(item.files.remove(0));
                    }
                    state.upload_items.push(item);
                }
            }
            UploadMode::Multi => {
                // multi-mode now uses chapter-based picking via chapter_pick_files events
                // if somehow pick_files is triggered in multi mode, add to the last chapter or create one
                if state.upload_chapters.is_empty() {
                    state.upload_chapters.push(ChapterItem::default());
                }
                let last = state.upload_chapters.last_mut().unwrap();
                let already_exists = last.files.iter().any(|f| f.name == file.name);
                if !already_exists {
                    last.files.push(file);
                }
            }
        }
    }

    rerender_upload_ui();
}

fn handle_upload_move(index: usize, direction: i32) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let new_index = (index as i32 + direction) as usize;
        if new_index >= state.upload_items.len() {
            return;
        }

        state.upload_items.swap(index, new_index);
    }

    rerender_upload_ui();
}

fn handle_upload_delete(index: usize) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if index >= state.upload_items.len() {
            return;
        }

        state.upload_items.remove(index);
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
    }

    rerender_upload_ui();
}

fn handle_add_chapter() {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_chapters.push(ChapterItem::default());
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

async fn handle_chapter_pick_files(chapter_index: usize) {
    let extensions = vec![
        "jpg".to_string(),
        "jpeg".to_string(),
        "png".to_string(),
        "webp".to_string(),
        "bmp".to_string(),
        "gif".to_string(),
    ];

    let filter = psys_host::dialog::FilterConfig {
        multiple: true,
        extensions,
        default_directory: String::new(),
        default_file_name: String::new(),
    };

    let pick_config = psys_host::dialog::PickConfig {
        read: true,
        copy_to: None,
    };

    let result = psys_host::dialog::pick_file(&pick_config, &filter).await;

    if result.data.is_empty() {
        tracing::info!("用户取消了章节文件选择");
        return;
    }

    let thumbnail = resize_to_width(&result.data, THUMBNAIL_WIDTH);
    let compressed = resize_to_width(&result.data, MASTER_WIDTH);

    let compressed_len = compressed.len();

    let file = UploadFile {
        name: result.name.clone(),
        data: compressed,
        size: compressed_len,
        original_size: result.data.len(),
        thumbnail,
    };

    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if chapter_index < state.upload_chapters.len() {
            let chapter = &mut state.upload_chapters[chapter_index];
            let already_exists = chapter.files.iter().any(|f| f.name == file.name);
            if !already_exists {
                chapter.files.push(file);
            }
        }
    }

    rerender_upload_ui();
}

fn handle_chapter_clear(chapter_index: usize) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if chapter_index < state.upload_chapters.len() {
            state.upload_chapters[chapter_index].files.clear();
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
    }
    rerender_upload_ui();
}

async fn handle_upload_pick_cover() {
    let extensions = vec![
        "jpg".to_string(),
        "jpeg".to_string(),
        "png".to_string(),
        "webp".to_string(),
        "bmp".to_string(),
        "gif".to_string(),
    ];

    let filter = psys_host::dialog::FilterConfig {
        multiple: false,
        extensions,
        default_directory: String::new(),
        default_file_name: String::new(),
    };

    let pick_config = psys_host::dialog::PickConfig {
        read: true,
        copy_to: None,
    };

    let result = psys_host::dialog::pick_file(&pick_config, &filter).await;

    if result.data.is_empty() {
        tracing::info!("用户取消了封面选择");
        return;
    }

    let thumbnail = resize_to_width(&result.data, THUMBNAIL_WIDTH);
    let compressed = resize_to_width(&result.data, MASTER_WIDTH);

    let compressed_len = compressed.len();

    let cover_file = UploadFile {
        name: result.name.clone(),
        data: compressed,
        size: compressed_len,
        original_size: result.data.len(),
        thumbnail,
    };

    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(first) = state.upload_items.first_mut() {
            first.cover = Some(cover_file);
        }
    }
    rerender_upload_ui();
}

async fn handle_multi_cover() {
    let extensions = vec![
        "jpg".to_string(),
        "jpeg".to_string(),
        "png".to_string(),
        "webp".to_string(),
        "bmp".to_string(),
        "gif".to_string(),
    ];

    let filter = psys_host::dialog::FilterConfig {
        multiple: false,
        extensions,
        default_directory: String::new(),
        default_file_name: String::new(),
    };

    let pick_config = psys_host::dialog::PickConfig {
        read: true,
        copy_to: None,
    };

    let result = psys_host::dialog::pick_file(&pick_config, &filter).await;

    if result.data.is_empty() {
        tracing::info!("用户取消了封面选择");
        return;
    }

    let thumbnail = resize_to_width(&result.data, THUMBNAIL_WIDTH);
    let compressed = resize_to_width(&result.data, MASTER_WIDTH);

    let compressed_len = compressed.len();

    let cover_file = UploadFile {
        name: result.name.clone(),
        data: compressed,
        size: compressed_len,
        original_size: result.data.len(),
        thumbnail,
    };

    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.multi_cover = Some(cover_file);
    }
    rerender_upload_ui();
}

async fn handle_chapter_upload(chapter_index: usize) {
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

    show_upload_status(StatusState::Processing("正在连接快应用...".to_string())).await;

    let progress = upload_progress();
    let device_addr = match handshake::prepare_launch(0, &progress).await {
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
        move |result| match result {
            Err(msg) => {
                wit_bindgen::block_on(show_upload_status(StatusState::Error(msg)));
            }
            Ok(_) => {
                wit_bindgen::block_on(chapter_upload_continue(
                    comic_name,
                    chapter_data,
                    device_addr,
                ));
            }
        },
    )
    .await;
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
        // 预转码模式下页面以 .bin 命名落盘，快应用阅读时按 .bin 优先加载
        let name = if watch_settings.image_pre_transcode {
            format!("{}.bin", page_num)
        } else {
            format!("{}", page_num)
        };

        show_upload_status(StatusState::Processing(format!(
            "正在处理 {}/{}",
            fi + 1,
            total
        )))
        .await;

        // data 为 MASTER_WIDTH 母版，发送前按快应用设置再处理
        let b64 = base64_encode(&process_for_send(&file.data, &watch_settings, true));
        file_names.push(name.clone());
        all_files.push((name, b64));
    }

    let header: Value = json!({
        "type": "import_comic_header",
        "name": comic_name,
        "mode": "single",
        "files": file_names,
        "page_count": page_count,
        "is_serial": true,
    });

    let header_str = match serde_json::to_string(&header) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化头部消息失败: {}", e);
            show_upload_status(StatusState::Error("数据序列化失败。".to_string())).await;
            return;
        }
    };

    let mut chunked_files: Vec<(String, Vec<String>)> = Vec::with_capacity(all_files.len());
    for (file_key, b64_data) in all_files {
        let chunks: Vec<String> = b64_data
            .as_bytes()
            .chunks(CHUNK_SIZE)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        chunked_files.push((file_key, chunks));
    }

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
}

async fn handle_upload_start() {
    reset_upload_progress();

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
    let device_addr = match handshake::prepare_launch(0, &progress).await {
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
        move |result| match result {
            Err(msg) => {
                wit_bindgen::block_on(show_upload_status(StatusState::Error(msg)));
            }
            Ok(_) => {
                wit_bindgen::block_on(upload_start_continue(device_addr));
            }
        },
    )
    .await;
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
    let header: Value;

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

                // data 为 MASTER_WIDTH 母版，发送前按快应用设置再处理（封面不转码）
                let b64 =
                    base64_encode(&process_for_send(&cover_file.data, &watch_settings, false));
                file_names.push("cover".to_string());
                all_files.push(("cover".to_string(), b64));
            }

            // Process pages
            let mut page_num: u32 = 0;
            for file in item.files.iter() {
                page_num += 1;
                // 预转码模式下页面以 .bin 命名落盘，快应用阅读时按 .bin 优先加载
                let name = if watch_settings.image_pre_transcode {
                    format!("{}.bin", page_num)
                } else {
                    format!("{}", page_num)
                };

                show_upload_status(StatusState::Processing(format!(
                    "正在处理 ({}/{})",
                    all_files.len() + 1,
                    "-"
                )))
                .await;

                // data 为 MASTER_WIDTH 母版，发送前按快应用设置再处理
                let b64 = base64_encode(&process_for_send(&file.data, &watch_settings, true));
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

            let b64 = base64_encode(&process_for_send(&cover_file.data, &watch_settings, false));
            all_files.push(("cover".to_string(), b64));
        }

        for (ci, chapter) in chapters.iter().enumerate() {
            if chapter.files.is_empty() {
                continue;
            }
            let mut chap_names: Vec<String> = Vec::new();
            let mut page_num: u32 = 0;

            // Format: "章节序号　章节名称" (full-width space)
            let chapter_folder = if chapter.name.trim().is_empty() {
                format!("{}　第{}章", ci + 1, ci + 1)
            } else {
                format!("{}　{}", ci + 1, chapter.name.trim())
            };

            // Process pages
            for (fi, file) in chapter.files.iter().enumerate() {
                page_num += 1;
                // 预转码模式下页面以 .bin 命名落盘，快应用阅读时按 .bin 优先加载
                let name = if watch_settings.image_pre_transcode {
                    format!("{}.bin", page_num)
                } else {
                    format!("{}", page_num)
                };
                let file_key = format!("{}/{}", chapter_folder, name);

                show_upload_status(StatusState::Processing(format!(
                    "正在处理 章节{} ({}/{})",
                    ci + 1,
                    fi + 1,
                    chapter.files.len()
                )))
                .await;

                // data 为 MASTER_WIDTH 母版，发送前按快应用设置再处理
                let b64 = base64_encode(&process_for_send(&file.data, &watch_settings, true));
                chap_names.push(name.clone());
                all_files.push((file_key, b64));
            }

            let ch_name_val = if chapter.name.trim().is_empty() {
                format!("第{}章", ci + 1)
            } else {
                chapter.name.clone()
            };
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

    let header_str = match serde_json::to_string(&header) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化头部消息失败: {}", e);
            show_upload_status(StatusState::Error("数据序列化失败。".to_string())).await;
            return;
        }
    };

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
                interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &done_str).await
            {
                tracing::error!("发送完成消息失败: {:?}", e);
            }
            show_upload_status(StatusState::Success("上传完成！".to_string())).await;
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

        match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &chunk_str).await {
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

const MASTER_WIDTH: u32 = 1280;
const THUMBNAIL_WIDTH: u32 = 100;
const CHUNK_SIZE: usize = 5500;

fn base64_encode(data: &[u8]) -> String {
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

fn resize_to_width(data: &[u8], target_width: u32) -> Vec<u8> {
    let img = match image::load_from_memory(data) {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!("图片解码失败，使用原始数据: {}", e);
            return data.to_vec();
        }
    };

    let (w, h) = (img.width(), img.height());
    if w <= target_width {
        tracing::debug!("图片宽度 {} <= {}，无需缩放", w, target_width);
        return data.to_vec();
    }

    let new_h = (h as f64 * target_width as f64 / w as f64).round() as u32;
    let new_h = new_h.max(1);

    tracing::info!("缩放图片: {}x{} -> {}x{}", w, h, target_width, new_h);

    let resized = img.resize_exact(target_width, new_h, image::imageops::FilterType::Lanczos3);

    let mut buf = std::io::Cursor::new(Vec::new());
    match resized.write_to(&mut buf, image::ImageFormat::Png) {
        Ok(_) => {
            let result = buf.into_inner();
            tracing::info!("缩放完成: {} bytes -> {} bytes", data.len(), result.len());
            result
        }
        Err(e) => {
            tracing::warn!("PNG 编码失败，使用原始数据: {}", e);
            data.to_vec()
        }
    }
}

fn update_domain_state(input_value: String) {
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.config.domain = input_value;
}

fn handle_cookie_input(input_value: String) {
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.config.cookie = input_value;
}

fn handle_domain_blur(input_value: String) {
    tracing::info!("处理域名失去焦点: {}", input_value);

    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.config.domain = input_value.clone();
        state.fetched_source_name = None;
        state.fetched_source_config = None;
    }

    if input_value.contains('.') {
        tracing::info!("域名包含点号，开始获取配置: {}", input_value);
        wit_bindgen::block_on(async move {
            fetch_domain_config_async(input_value).await;
        });
    } else {
        tracing::info!("域名不包含点号，跳过获取: {}", input_value);
    }
}

async fn fetch_domain_config_async(domain: String) {
    tracing::info!("fetch_domain_config_async 被调用: {}", domain);

    show_status(StatusState::Processing("正在获取漫画源配置...".to_string())).await;

    timer::set_timeout(50, "").await;

    tracing::info!("调用 fetch_source_name: {}", domain);
    match fetch_source_name(&domain).await {
        Some(source_name) => {
            tracing::info!("获取配置成功: {}", source_name);
            let full_config = fetch_source_config(&domain).await;
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.fetched_source_name = Some(source_name.clone());
                state.config.source_name = source_name.clone();
                state.fetched_source_config = full_config;
            }
            show_status(StatusState::Success(format!("获取成功：{}", source_name))).await;
        }
        None => {
            tracing::error!("获取配置失败: {}", domain);
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.fetched_source_name = None;
                state.fetched_source_config = None;
                state.config.source_name = String::new();
            }
            show_status(StatusState::Error(
                "无法获取漫画源配置，请检查域名是否正确。".to_string(),
            ))
            .await;
        }
    }
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
        psys_host::ui_v3::render(&root_id, ui);
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
        psys_host::ui_v3::render(&root_id, ui);
    }
}

/// 同步渲染同步页状态栏（无定时器管理）
fn render_sync_progress(msg: String) {
    let root_id = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.current_status = StatusState::Processing(msg);
        state.root_element_id.clone()
    };
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui_v3::render(&root_id, ui);
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

/// 生成一个把握手阶段进度转发到同步页状态栏的回调
fn sync_progress() -> impl Fn(String) {
    |msg| render_sync_progress(msg)
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

/// 按快应用设置处理待发送的图片：缩放到 imageSize 后，
/// 页面：imagePreTranscode 时转 LVGL indexed-8 bin，否则按 imageUsePng/imageQuality 编码；
/// 封面（is_page=false）：始终普通图片格式（与下载链路行为一致）。
fn process_for_send(data: &[u8], settings: &WatchSettings, is_page: bool) -> Vec<u8> {
    let img = match image::load_from_memory(data) {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!("发送前图片解码失败，使用原始数据: {}", e);
            return data.to_vec();
        }
    };

    let target = settings.image_size.clamp(100, 4096);
    let img = if img.width() > target {
        let new_h =
            ((img.height() as f64 * target as f64 / img.width() as f64).round() as u32).max(1);
        img.resize_exact(target, new_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    if is_page && settings.image_pre_transcode {
        return crate::lvgl::convert_to_lvgl_i8(&img);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    if settings.image_use_png {
        match img.write_to(&mut buf, image::ImageFormat::Png) {
            Ok(_) => buf.into_inner(),
            Err(e) => {
                tracing::warn!("PNG 编码失败，使用原始数据: {}", e);
                data.to_vec()
            }
        }
    } else {
        let rgb = img.to_rgb8();
        let quality = settings.image_quality.clamp(1, 100) as u8;
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        match encoder.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        ) {
            Ok(_) => buf.into_inner(),
            Err(e) => {
                tracing::warn!("JPEG 编码失败，使用原始数据: {}", e);
                data.to_vec()
            }
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
        let _ = timer::clear_timer(t).await;
    }
    let tid = timer::set_timeout(ACK_TIMEOUT_MS, UPLOAD_ACK_TIMEOUT_EVENT).await;
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
        let _ = timer::clear_timer(t).await;
    }
}

/// 发送导入头部并武装头部 ACK 超时定时器。
/// 必须等快应用回 import_header_ack 后才开始发分片：
/// 安卓端 QAIC 不保证消息顺序，分片可能反超头部被手表丢弃导致死锁。
async fn send_import_header(device_addr: &str, header_str: &str) {
    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, header_str).await {
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
        let _ = timer::clear_timer(t).await;
    }
    let tid = timer::set_timeout(ACK_TIMEOUT_MS, UPLOAD_HEADER_TIMEOUT_EVENT).await;
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
        let _ = timer::clear_timer(t).await;
    }
}

/// 头部 ACK 超时：重发头部（上限 3 次）；超限说明对端可能是
/// 无 header ACK 机制的旧版快应用，退回兼容模式直接发分片。
pub fn handle_upload_header_timeout() {
    wit_bindgen::block_on(async {
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
                match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &header_str)
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
    });
}

/// ACK 超时处理：重传当前在途分片；超过重传上限则中止上传。
/// 由定时器事件 UPLOAD_ACK_TIMEOUT_EVENT 触发。
pub fn handle_upload_ack_timeout() {
    wit_bindgen::block_on(async {
        enum Action {
            Nothing,
            Resend(usize, usize),
            Abort,
        }

        let (action, device_addr) = {
            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.upload_session.as_mut() {
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

                match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &chunk_str)
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
    });
}

async fn handle_sync() {
    let (cookie, domain, mut source_name) = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            state.config.cookie.clone(),
            state.config.domain.clone(),
            state.fetched_source_name.clone().or_else(|| {
                if state.config.source_name.is_empty() {
                    None
                } else {
                    Some(state.config.source_name.clone())
                }
            }),
        )
    };

    tracing::info!("开始同步: domain={}, source_name={:?}", domain, source_name);

    show_status(StatusState::Processing("正在验证输入...".to_string())).await;

    if domain.is_empty() {
        show_status(StatusState::Error("漫画源域名不能为空。".to_string())).await;
        return;
    }

    if source_name.is_none() {
        show_status(StatusState::Processing("正在获取漫画源配置...".to_string())).await;

        match fetch_source_name(&domain).await {
            Some(name) => {
                source_name = Some(name.clone());
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.fetched_source_name = Some(name.clone());
                state.config.source_name = name;
            }
            None => {
                show_status(StatusState::Error(
                    "无法获取漫画源配置，请检查域名是否正确。".to_string(),
                ))
                .await;
                return;
            }
        }
    }

    let source_name = source_name.unwrap();

    show_status(StatusState::Processing("正在检查快应用...".to_string())).await;

    let progress = sync_progress();
    let device_addr = match handshake::prepare_launch(259, &progress).await {
        Ok(addr) => addr,
        Err(msg) => {
            show_status(StatusState::Error(msg)).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成后在 on_done 回调里继续同步流程
    handshake::begin_wait(
        device_addr.clone(),
        sync_progress(),
        move |result| match result {
            Err(msg) => {
                wit_bindgen::block_on(show_status(StatusState::Error(msg)));
            }
            Ok(_) => {
                wit_bindgen::block_on(sync_continue(cookie, domain, source_name, device_addr));
            }
        },
    )
    .await;
}

async fn sync_continue(cookie: String, domain: String, source_name: String, device_addr: String) {
    show_status(StatusState::Processing("正在发送到手表...".to_string())).await;

    if !cookie.is_empty() {
        let cookie_data = json!({
            "type": "cookie",
            &source_name: cookie
        });

        let cookie_str = match serde_json::to_string(&cookie_data) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("序列化 Cookie 数据失败: {}", e);
                show_status(StatusState::Error("数据序列化失败。".to_string())).await;
                return;
            }
        };

        match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &cookie_str).await {
            Ok(_) => {
                tracing::info!("Cookie 发送成功");
            }
            Err(e) => {
                tracing::error!("发送 Cookie 失败: {:?}", e);
                show_status(StatusState::Error("Cookie 发送失败。".to_string())).await;
                return;
            }
        }
    }

    show_status(StatusState::Processing("正在发送漫画源配置...".to_string())).await;

    let source_config = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.fetched_source_config.clone()
    };

    let source_config = match source_config {
        Some(config) => config,
        None => {
            tracing::info!("缓存中没有完整配置，重新获取");
            match fetch_source_config(&domain).await {
                Some(config) => config,
                None => {
                    show_status(StatusState::Success(
                        "Cookie 已同步，但漫画源配置获取失败。".to_string(),
                    ))
                    .await;
                    return;
                }
            }
        }
    };

    let source_msg = json!({
        "type": "source_config",
        "configs": source_config
    });

    let source_msg_str = match serde_json::to_string(&source_msg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化漫画源配置失败: {}", e);
            show_status(StatusState::Error("漫画源配置序列化失败。".to_string())).await;
            return;
        }
    };

    match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &source_msg_str).await {
        Ok(_) => {
            show_status(StatusState::Success("同步成功！".to_string())).await;
        }
        Err(e) => {
            tracing::error!("发送漫画源配置失败: {:?}", e);
            show_status(StatusState::Error("漫画源配置发送失败。".to_string())).await;
        }
    }
}

async fn handle_delete_comic(index: usize) {
    let comic_name = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_comics.get(index).map(|c| c.name.clone())
    };

    let comic_name = match comic_name {
        Some(name) if !name.is_empty() => name,
        _ => {
            show_app_data_status(StatusState::Error("找不到该漫画信息。".to_string())).await;
            return;
        }
    };

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
        &dialog_info,
    )
    .await;

    if dialog_result.clicked_btn_id != "confirm" {
        tracing::info!("用户取消删除: {}", comic_name);
        return;
    }

    show_app_data_status(StatusState::Processing(format!(
        "正在删除: {}...",
        comic_name
    )))
    .await;

    let progress = app_data_progress();
    let device_addr = match handshake::prepare_launch(0, &progress).await {
        Ok(addr) => addr,
        Err(msg) => {
            show_app_data_status(StatusState::Error(msg)).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成后在 on_done 回调里继续删除流程
    handshake::begin_wait(
        device_addr.clone(),
        app_data_progress(),
        move |result| match result {
            Err(msg) => {
                wit_bindgen::block_on(show_app_data_status(StatusState::Error(msg)));
            }
            Ok(_) => {
                wit_bindgen::block_on(delete_comic_continue(comic_name, index, device_addr));
            }
        },
    )
    .await;
}

async fn delete_comic_continue(comic_name: String, index: usize, device_addr: String) {
    let delete_msg = json!({
        "type": "delete_comic",
        "name": comic_name
    });

    let delete_str = match serde_json::to_string(&delete_msg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化删除消息失败: {}", e);
            show_app_data_status(StatusState::Error("序列化失败。".to_string())).await;
            return;
        }
    };

    match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &delete_str).await {
        Ok(_) => {
            tracing::info!("删除命令已发送: {}", comic_name);

            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if index < state.app_comics.len() {
                    state.app_comics.remove(index);
                    state.app_comic_count = Some(state.app_comics.len());
                }
            }

            show_app_data_status(StatusState::Success(format!("已删除: {}", comic_name))).await;

            let root_id: Option<String>;
            {
                let state = ui_state()
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                root_id = state.root_element_id.clone();
            }
            if let Some(root_id) = root_id {
                let ui = build_main_ui();
                psys_host::ui_v3::render(&root_id, ui);
            }

            build::render_comic_data_card(COMIC_DATA_CARD_ID);
        }
        Err(e) => {
            tracing::error!("发送删除命令失败: {:?}", e);
            show_app_data_status(StatusState::Error("发送删除命令失败。".to_string())).await;
        }
    }
}

async fn handle_delete_source(index: usize) {
    let source_name = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_sources.get(index).map(|s| s.name.clone())
    };

    let source_name = match source_name {
        Some(name) if !name.is_empty() => name,
        _ => {
            show_app_data_status(StatusState::Error("找不到该漫画源信息。".to_string())).await;
            return;
        }
    };

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
        &dialog_info,
    )
    .await;

    if dialog_result.clicked_btn_id != "confirm" {
        tracing::info!("用户取消删除漫画源: {}", source_name);
        return;
    }

    show_app_data_status(StatusState::Processing(format!(
        "正在删除漫画源: {}...",
        source_name
    )))
    .await;

    let progress = app_data_progress();
    let device_addr = match handshake::prepare_launch(0, &progress).await {
        Ok(addr) => addr,
        Err(msg) => {
            show_app_data_status(StatusState::Error(msg)).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成后在 on_done 回调里继续删除流程
    handshake::begin_wait(
        device_addr.clone(),
        app_data_progress(),
        move |result| match result {
            Err(msg) => {
                wit_bindgen::block_on(show_app_data_status(StatusState::Error(msg)));
            }
            Ok(_) => {
                wit_bindgen::block_on(delete_source_continue(source_name, index, device_addr));
            }
        },
    )
    .await;
}

async fn delete_source_continue(source_name: String, index: usize, device_addr: String) {
    let delete_msg = json!({
        "type": "delete_source",
        "name": source_name
    });

    let delete_str = match serde_json::to_string(&delete_msg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化删除消息失败: {}", e);
            show_app_data_status(StatusState::Error("序列化失败。".to_string())).await;
            return;
        }
    };

    match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &delete_str).await {
        Ok(_) => {
            tracing::info!("删除漫画源命令已发送: {}", source_name);

            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if index < state.app_sources.len() {
                    state.app_sources.remove(index);
                    state.app_source_count = Some(state.app_sources.len());
                }
            }

            show_app_data_status(StatusState::Success(format!(
                "已删除漫画源: {}",
                source_name
            )))
            .await;

            let root_id: Option<String>;
            {
                let state = ui_state()
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                root_id = state.root_element_id.clone();
            }
            if let Some(root_id) = root_id {
                let ui = build_main_ui();
                psys_host::ui_v3::render(&root_id, ui);
            }

            build::render_comic_data_card(COMIC_DATA_CARD_ID);
        }
        Err(e) => {
            tracing::error!("发送删除漫画源命令失败: {:?}", e);
            show_app_data_status(StatusState::Error("发送删除命令失败。".to_string())).await;
        }
    }
}

/// 列表数据接收超时（request_data 发出到 app_data_done）
const APP_DATA_RECV_TIMEOUT_MS: u64 = 20_000;
/// 封面接收超时（app_data_done 到 cover_done）
const COVER_RECV_TIMEOUT_MS: u64 = 30_000;

/// 武装/重武装拉取数据整体接收超时定时器
async fn arm_app_data_recv_timeout(timeout_ms: u64) {
    let old = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_data_recv_timer_id.take()
    };
    if let Some(t) = old {
        let _ = timer::clear_timer(t).await;
    }
    let tid = timer::set_timeout(timeout_ms, APP_DATA_RECV_TIMEOUT_EVENT).await;
    let mut state = ui_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.app_data_recv_timer_id = Some(tid);
}

async fn disarm_app_data_recv_timeout() {
    let old = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_data_recv_timer_id.take()
    };
    if let Some(t) = old {
        let _ = timer::clear_timer(t).await;
    }
}

/// 拉取数据整体接收超时：列表阶段超时报错；封面阶段超时降级为成功（封面可能不完整）
pub fn handle_app_data_recv_timeout() {
    wit_bindgen::block_on(async {
        let (has_data, status) = {
            let state = ui_state()
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                state.app_comics.iter().any(|c| !c.name.is_empty()),
                state.app_data_status.clone(),
            )
        };
        // 已结束（Success/Error）的会话不处理
        if matches!(status, StatusState::Processing(_)) {
            if has_data {
                show_app_data_status(StatusState::Success(
                    "数据获取完成（封面可能不完整）".to_string(),
                ))
                .await;
            } else {
                show_app_data_status(StatusState::Error("接收超时，请重试。".to_string())).await;
            }
        }
    });
}

async fn handle_fetch_app_data() {
    show_app_data_status(StatusState::Processing("正在获取快应用数据...".to_string())).await;

    // 新一轮数据会话：清空上一轮残留。
    // 清理动作必须在发起请求时完成——安卓端消息乱序，
    // 靠 app_data_header 到达时机清理会误删已收到的数据
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_comics.clear();
        state.app_sources.clear();
        state.app_comic_count = None;
        state.app_source_count = None;
        state.cover_chunk_buffers.clear();
        // 清理过期的 pending_covers（30秒未补挂的丢弃）
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        state.pending_covers.retain(|_, (_, ts)| now - *ts < 30);
        if !state.pending_covers.is_empty() {
            tracing::info!("清理了 {} 个过期封面缓存", state.pending_covers.len());
        }
    }

    let progress = app_data_progress();
    let device_addr = match handshake::prepare_launch(0, &progress).await {
        Ok(addr) => addr,
        Err(msg) => {
            show_app_data_status(StatusState::Error(msg)).await;
            return;
        }
    };

    // 握手等待由定时器事件驱动，完成（pong 到达）后才发 request_data，
    // 保证快应用确实已启动并能收到消息
    handshake::begin_wait(
        device_addr.clone(),
        app_data_progress(),
        move |result| match result {
            Err(msg) => {
                wit_bindgen::block_on(show_app_data_status(StatusState::Error(msg)));
            }
            Ok(_) => {
                wit_bindgen::block_on(fetch_app_data_send_request(device_addr));
            }
        },
    )
    .await;
}

async fn fetch_app_data_send_request(device_addr: String) {
    let request_msg = json!({
        "type": "request_data"
    });

    let request_str = match serde_json::to_string(&request_msg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            show_app_data_status(StatusState::Error("请求序列化失败。".to_string())).await;
            return;
        }
    };

    match interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &request_str).await {
        Ok(_) => {
            tracing::info!("数据请求已发送，等待手表回复...");
            arm_app_data_recv_timeout(APP_DATA_RECV_TIMEOUT_MS).await;
            show_app_data_status(StatusState::Processing("等待手表返回数据...".to_string())).await;
        }
        Err(e) => {
            tracing::error!("发送数据请求失败: {:?}", e);
            show_app_data_status(StatusState::Error("发送请求失败，请检查连接。".to_string()))
                .await;
        }
    }
}

/// 发送 app_data 消息的 ACK 确认
/// 快应用端等待此 ACK 后继续发送下一个消息，确保安卓端严格顺序
fn send_app_data_ack(device_addr: &str, index: usize) {
    let ack_msg = json!({
        "type": "app_data_ack",
        "index": index,
    });

    if let Ok(ack_str) = serde_json::to_string(&ack_msg) {
        wit_bindgen::block_on(async {
            let _ =
                interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &ack_str).await;
        });
    }
}

pub fn handle_interconnect_message(payload: &str) {
    tracing::info!("收到互联消息: {}", payload);

    // 设备地址懒获取：只有需要回 ACK 的消息分支才查询，
    // 避免封面分片等高频消息每条都做一次设备列表 FFI
    let addr_cell = std::cell::OnceCell::new();
    let get_addr = || -> Option<String> {
        addr_cell
            .get_or_init(|| {
                let devices = device::get_connected_device_list();
                wit_bindgen::block_on(async {
                    let devices = devices.await;
                    if !devices.is_empty() {
                        Some(devices[0].addr.clone())
                    } else {
                        None
                    }
                })
            })
            .clone()
    };

    let outer = match serde_json::from_str::<Value>(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("无法解析互联消息: {}", e);
            return;
        }
    };

    let inner_str = if let Some(pt) = outer.get("payloadText").and_then(|v| v.as_str()) {
        tracing::info!("从 payloadText 解包数据");
        pt.to_string()
    } else {
        tracing::info!("直接使用原始 payload");
        payload.to_string()
    };

    let parsed = match serde_json::from_str::<Value>(&inner_str) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("无法解析内部 JSON: {}", e);
            return;
        }
    };

    let msg_type = parsed.get("type").and_then(|v| v.as_str());

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
            state.app_comic_count = Some(comic_count);
            state.app_source_count = Some(source_count);
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

            // 发送 ACK 确认，让快应用继续发下一个
            // 快应用 msgIndex = 0，ACK 后 msgIndex++ 变成 1，所以 ACK 序号 = 0
            if let Some(ref addr) = get_addr() {
                send_app_data_ack(addr, 0);
            }
        }
        Some("app_data_comic") => {
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let comic = parsed.get("comic");

            if let Some(comic) = comic {
                let mut info = ComicInfo {
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
                    // 接收进度提示
                    state.app_data_status = StatusState::Processing(format!(
                        "接收漫画 {}/{}",
                        index + 1,
                        state.app_comic_count.unwrap_or(0)
                    ));
                    state.root_element_id.clone()
                };
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui_v3::render(&root_id, ui);
                    build::render_comic_data_card(COMIC_DATA_CARD_ID);
                }
            }
            // 发送 ACK 确认，让快应用继续发下一个
            // 当前消息在快应用的 msgIndex = (index + 1)，所以 ACK 序号 = (index + 1)
            // 因为 header 占用 msgIndex 0，所以 comic 从 1 开始
            if let Some(ref addr) = get_addr() {
                send_app_data_ack(addr, 1 + index);
            }
        }
        Some("app_data_source") => {
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let source = parsed.get("source");

            if let Some(source) = source {
                let info = SourceInfo {
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
                    // 接收进度提示
                    state.app_data_status = StatusState::Processing(format!(
                        "接收漫画源 {}/{}",
                        index + 1,
                        state.app_source_count.unwrap_or(0)
                    ));
                    state.root_element_id.clone()
                };
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui_v3::render(&root_id, ui);
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
            if let Some(ref addr) = get_addr() {
                send_app_data_ack(addr, msg_index);
            }
        }
        Some("app_data_done") => {
            tracing::info!("列表数据接收完成，渲染 UI");

            let (comic_count, source_count, root_id) = {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let comic_count = state.app_comic_count.unwrap_or(0);
                // 有漫画时还有封面要收：进入封面接收阶段；
                // 没有漫画则整个拉取流程结束
                state.app_data_status = if comic_count > 0 {
                    StatusState::Processing("正在接收封面...".to_string())
                } else {
                    StatusState::Success("数据获取成功！".to_string())
                };
                (
                    comic_count,
                    state.app_source_count.unwrap_or(0),
                    state.root_element_id.clone(),
                )
            };

            // 封面阶段重新武装整体超时；无封面则整个流程结束，解除超时
            if comic_count > 0 {
                wit_bindgen::block_on(arm_app_data_recv_timeout(COVER_RECV_TIMEOUT_MS));
            } else {
                wit_bindgen::block_on(disarm_app_data_recv_timeout());
            }

            if let Some(root_id) = root_id {
                let ui = build_main_ui();
                psys_host::ui_v3::render(&root_id, ui);
            }

            build::render_comic_data_card(COMIC_DATA_CARD_ID);

            // done 消息也要 ACK，表示可以开始发封面
            // msgIndex = 1 + comic_count + source_count = done 的位置
            let msg_index = 1 + comic_count + source_count;
            if let Some(ref addr) = get_addr() {
                send_app_data_ack(addr, msg_index);
            }
        }
        Some("cover_done") => {
            // 快应用已全部封面发送完毕（且每张都已被 ACK）：整个拉取流程结束
            tracing::info!("封面接收完成");
            wit_bindgen::block_on(async {
                disarm_app_data_recv_timeout().await;
                show_app_data_status(StatusState::Success("数据获取成功！".to_string())).await;
            });
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

                let buf = state
                    .cover_chunk_buffers
                    .entry(name.to_string())
                    .or_insert_with(|| (total, vec![String::new(); total]));

                if buf.1.len() != total {
                    buf.1.resize(total, String::new());
                }
                buf.1[index] = data.to_string();

                let all_done = buf.1.iter().all(|s| !s.is_empty());
                if all_done {
                    let cover = buf.1.concat();
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
                    // 无论挂载还是暂存都算拼完，都需要回 cover_ack
                    (true, state.root_element_id.clone())
                } else {
                    (false, state.root_element_id.clone())
                }
            };

            if done {
                // 回 cover_ack 通知快应用发下一张封面（无论挂载还是暂存都要回）
                if let Some(ref addr) = get_addr() {
                    let ack = json!({
                        "type": "cover_ack",
                        "name": name,
                    });
                    if let Ok(ack_str) = serde_json::to_string(&ack) {
                        wit_bindgen::block_on(async {
                            let _ =
                                interconnect::send_qaic_message(addr, WATCH_APP_PKG_NAME, &ack_str)
                                    .await;
                        });
                    }
                }
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui_v3::render(&root_id, ui);
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
                wit_bindgen::block_on(async {
                    disarm_header_timeout().await;
                    send_next_chunk().await;
                });
            }
        }
        Some("hs_pong") => {
            let session = parsed
                .get("session")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let settings = parsed
                .get("settings")
                .map(WatchSettings::from_json)
                .unwrap_or_default();
            tracing::info!("收到握手应答: session={}, settings={:?}", session, settings);

            if let Some(ref addr) = get_addr() {
                {
                    let mut state = ui_state()
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.watch_settings = Some(settings);
                }
                // 完成挂起的握手会话（内部会调用业务续体，必须在 block_on 之外）
                handshake::handle_hs_pong(addr, &session, &parsed);
            }
        }
        Some("import_chunk_ack") => {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");
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
                wit_bindgen::block_on(async {
                    disarm_ack_timeout().await;
                    send_next_chunk().await;
                });
            }
        }
        _ => {
            tracing::info!("收到未处理的消息类型: {:?}", msg_type);
        }
    }
}

pub async fn show_app_data_status(status: StatusState) {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(old_timer) = state.app_data_timer_id {
            let _ = timer::clear_timer(old_timer).await;
        }

        state.app_data_status = status.clone();

        if matches!(status, StatusState::Success(_) | StatusState::Error(_)) {
            let new_timer = timer::set_timeout(5000, HIDE_APP_DATA_STATUS_EVENT).await;
            state.app_data_timer_id = Some(new_timer);
        }

        root_id = state.root_element_id.clone();
    }

    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui_v3::render(&root_id, ui);
    }
}

pub fn hide_app_data_status() {
    let root_id: Option<String>;
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.app_data_status = StatusState::Default;
        state.app_data_timer_id = None;
        root_id = state.root_element_id.clone();
    }
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        psys_host::ui_v3::render(&root_id, ui);
    }
}
