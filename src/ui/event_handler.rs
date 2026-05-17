use super::state::*;
use super::{COMIC_DATA_CARD_ID};
use crate::astrobox::psys_host::{self, device, dialog, interconnect, register, thirdpartyapp, timer};
use crate::network::{fetch_source_config, fetch_source_name};
use serde_json::{json, Value};
use std::time::Duration;

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
        TAB_SYNC_EVENT => {
            switch_tab(TabPage::Sync);
        }
        TAB_DATA_EVENT => {
            switch_tab(TabPage::Data);
        }
        FETCH_APP_DATA_EVENT => {
            tracing::info!("获取快应用数据按钮被点击");
            wit_bindgen::block_on(handle_fetch_app_data());
        }
        _ => {
            if let Some(index_str) = event_id.strip_prefix(DELETE_COMIC_PREFIX) {
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

    let devices = device::get_connected_device_list().await;

    if devices.is_empty() {
        show_status(StatusState::Error("没有已连接的设备，请检查手表连接。".to_string())).await;
        return;
    }

    let device_addr = &devices[0].addr;

    let app_list = match thirdpartyapp::get_thirdparty_app_list(device_addr).await {
        Ok(apps) => apps,
        Err(e) => {
            tracing::error!("获取快应用列表失败: {:?}", e);
            show_status(StatusState::Error("无法获取快应用列表。".to_string())).await;
            return;
        }
    };

    let app = match app_list.iter().find(|a| a.package_name == WATCH_APP_PKG_NAME) {
        Some(a) => a,
        None => {
            show_status(StatusState::Error("请先安装腕上漫画快应用！".to_string())).await;
            return;
        }
    };

    if app.version_code < 181 {
        show_status(StatusState::Error("请先安装腕上漫画快应用的新版本！".to_string())).await;
        return;
    }

    let _ = register::register_interconnect_recv(device_addr, WATCH_APP_PKG_NAME).await;

    if let Err(e) = thirdpartyapp::launch_qa(device_addr, app, "/pages/index").await {
        tracing::error!("启动快应用失败: {:?}", e);
        show_status(StatusState::Error("启动快应用失败。".to_string())).await;
        return;
    }

    std::thread::sleep(Duration::from_secs(2));

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

        match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &cookie_str).await {
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

    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &source_msg_str).await {
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
    ).await;

    if dialog_result.clicked_btn_id != "confirm" {
        tracing::info!("用户取消删除: {}", comic_name);
        return;
    }

    show_app_data_status(StatusState::Processing(format!("正在删除: {}...", comic_name))).await;

    let devices = device::get_connected_device_list().await;

    if devices.is_empty() {
        show_app_data_status(StatusState::Error("没有已连接的设备。".to_string())).await;
        return;
    }

    let device_addr = &devices[0].addr;

    let app_list = match thirdpartyapp::get_thirdparty_app_list(device_addr).await {
        Ok(apps) => apps,
        Err(_) => {
            show_app_data_status(StatusState::Error("无法获取快应用列表。".to_string())).await;
            return;
        }
    };

    let app = match app_list.iter().find(|a| a.package_name == WATCH_APP_PKG_NAME) {
        Some(a) => a,
        None => {
            show_app_data_status(StatusState::Error("请先安装腕上漫画快应用！".to_string())).await;
            return;
        }
    };

    if let Err(e) = thirdpartyapp::launch_qa(device_addr, app, "/pages/index").await {
        tracing::error!("启动快应用失败: {:?}", e);
        show_app_data_status(StatusState::Error("启动快应用失败。".to_string())).await;
        return;
    }

    std::thread::sleep(Duration::from_secs(2));

    let _ = register::register_interconnect_recv(device_addr, WATCH_APP_PKG_NAME).await;

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

    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &delete_str).await {
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
    ).await;

    if dialog_result.clicked_btn_id != "confirm" {
        tracing::info!("用户取消删除漫画源: {}", source_name);
        return;
    }

    show_app_data_status(StatusState::Processing(format!("正在删除漫画源: {}...", source_name))).await;

    let devices = device::get_connected_device_list().await;

    if devices.is_empty() {
        show_app_data_status(StatusState::Error("没有已连接的设备。".to_string())).await;
        return;
    }

    let device_addr = &devices[0].addr;

    let app_list = match thirdpartyapp::get_thirdparty_app_list(device_addr).await {
        Ok(apps) => apps,
        Err(_) => {
            show_app_data_status(StatusState::Error("无法获取快应用列表。".to_string())).await;
            return;
        }
    };

    let app = match app_list.iter().find(|a| a.package_name == WATCH_APP_PKG_NAME) {
        Some(a) => a,
        None => {
            show_app_data_status(StatusState::Error("请先安装腕上漫画快应用！".to_string())).await;
            return;
        }
    };

    if let Err(e) = thirdpartyapp::launch_qa(device_addr, app, "/pages/index").await {
        tracing::error!("启动快应用失败: {:?}", e);
        show_app_data_status(StatusState::Error("启动快应用失败。".to_string())).await;
        return;
    }

    std::thread::sleep(Duration::from_secs(2));

    let _ = register::register_interconnect_recv(device_addr, WATCH_APP_PKG_NAME).await;

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

    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &delete_str).await {
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

            show_app_data_status(StatusState::Success(format!("已删除漫画源: {}", source_name))).await;

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

async fn handle_fetch_app_data() {
    show_app_data_status(StatusState::Processing("正在获取快应用数据...".to_string())).await;

    let devices = device::get_connected_device_list().await;

    if devices.is_empty() {
        show_app_data_status(StatusState::Error("没有已连接的设备。".to_string())).await;
        return;
    }

    let device_addr = &devices[0].addr;

    let app_list = match thirdpartyapp::get_thirdparty_app_list(device_addr).await {
        Ok(apps) => apps,
        Err(_) => {
            show_app_data_status(StatusState::Error("无法获取快应用列表。".to_string())).await;
            return;
        }
    };

    let app = match app_list.iter().find(|a| a.package_name == WATCH_APP_PKG_NAME) {
        Some(a) => a,
        None => {
            show_app_data_status(StatusState::Error("请先安装腕上漫画快应用！".to_string())).await;
            return;
        }
    };

    show_app_data_status(StatusState::Processing("正在启动快应用...".to_string())).await;

    if let Err(e) = thirdpartyapp::launch_qa(device_addr, app, "/pages/index").await {
        tracing::error!("启动快应用失败: {:?}", e);
        show_app_data_status(StatusState::Error("启动快应用失败。".to_string())).await;
        return;
    }

    std::thread::sleep(Duration::from_secs(2));

    let _ = register::register_interconnect_recv(device_addr, WATCH_APP_PKG_NAME).await;

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

    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &request_str).await {
        Ok(_) => {
            tracing::info!("数据请求已发送，等待手表回复...");
            show_app_data_status(StatusState::Processing("等待手表返回数据...".to_string())).await;
        }
        Err(e) => {
            tracing::error!("发送数据请求失败: {:?}", e);
            show_app_data_status(StatusState::Error("发送请求失败，请检查连接。".to_string())).await;
        }
    }
}

pub fn handle_interconnect_message(payload: &str) {
    tracing::info!("收到互联消息: {}", payload);

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
            let comic_count = parsed.get("comic_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let source_count = parsed.get("source_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            tracing::info!("收到数据头: comic_count={}, source_count={}", comic_count, source_count);

            let mut state = ui_state()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.app_comic_count = Some(comic_count);
            state.app_source_count = Some(source_count);
            state.app_comics = vec![ComicInfo {
                name: String::new(),
                page_count: 0,
                chapters: 0,
                cover_base64: String::new(),
            }; comic_count];
            state.app_sources = vec![SourceInfo {
                name: String::new(),
                api_url: String::new(),
            }; source_count];
            state.cover_chunk_buffers.clear();
            state.app_data_status = StatusState::Processing("接收中...".to_string());
        }
        Some("app_data_comic") => {
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let comic = parsed.get("comic");

            if let Some(comic) = comic {
                let info = ComicInfo {
                    name: comic.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    page_count: comic.get("page_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    chapters: comic.get("chapters").and_then(|v| v.as_u64()).unwrap_or(1) as usize,
                    cover_base64: String::new(),
                };
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if index < state.app_comics.len() {
                    state.app_comics[index] = info;
                }
            }
        }
        Some("app_data_source") => {
            let index = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let source = parsed.get("source");

            if let Some(source) = source {
                let info = SourceInfo {
                    name: source.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    api_url: source.get("apiUrl").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                };
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if index < state.app_sources.len() {
                    state.app_sources[index] = info;
                }
            }
        }
        Some("app_data_done") => {
            tracing::info!("数据接收完成，渲染 UI");

            let root_id: Option<String>;
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.app_data_status = StatusState::Success("数据获取成功！".to_string());
                root_id = state.root_element_id.clone();
            }
            if let Some(root_id) = root_id {
                let ui = build_main_ui();
                psys_host::ui_v3::render(&root_id, ui);
            }

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
                name, index + 1, total, data.len()
            );

            let (done, root_id) = {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());

                let buf = state.cover_chunk_buffers
                    .entry(name.to_string())
                    .or_insert_with(|| (total, vec![String::new(); total]));

                if buf.1.len() != total {
                    buf.1.resize(total, String::new());
                }
                buf.1[index] = data.to_string();

                let all_done = buf.1.iter().all(|s| !s.is_empty());
                if all_done {
                    let cover = buf.1.concat();
                    if let Some(comic) = state.app_comics.iter_mut().find(|c| c.name == name) {
                        comic.cover_base64 = cover;
                    }
                    state.cover_chunk_buffers.remove(name);
                    (true, state.root_element_id.clone())
                } else {
                    (false, state.root_element_id.clone())
                }
            };

            if done {
                if let Some(root_id) = root_id {
                    let ui = build_main_ui();
                    psys_host::ui_v3::render(&root_id, ui);
                }
                build::render_comic_data_card(COMIC_DATA_CARD_ID);
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