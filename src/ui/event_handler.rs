use crate::astrobox::psys_host::{self, thirdpartyapp, interconnect, device, timer};
use crate::network::fetch_source_name;
use super::state::*;
use super::message::{show_status, hide_status};
use serde_json::json;
use std::time::Duration;

pub fn ui_event_processor(event_type: psys_host::ui::Event, event_id: &str, event_payload: &str) {
    tracing::debug!("UI 事件: id={}, type={:?}, payload={}", event_id, event_type, event_payload);

    match event_id {
        DOMAIN_INPUT_CHANGE_EVENT => {
            // 只更新状态，不触发网络请求
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    tracing::debug!("域名输入变化: {}", text);
                    update_domain_state(text.to_string());
                }
            }
        }
        DOMAIN_INPUT_BLUR_EVENT => {
            // 失去焦点时触发网络请求
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(event_payload) {
                if let Some(text) = value.get("value").and_then(|v| v.as_str()) {
                    tracing::info!("域名输入框失去焦点，开始获取配置: {}", text);
                    handle_domain_blur(text.to_string());
                }
            }
        }
        COOKIE_INPUT_EVENT => {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(event_payload) {
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
        _ => {}
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
    
    // 更新域名状态并清空已获取的配置
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.config.domain = input_value.clone();
        state.fetched_source_name = None;
    }
    
    // 如果输入包含点号，获取配置
    if input_value.contains('.') {
        tracing::info!("域名包含点号，开始获取配置: {}", input_value);
        // 使用 block_on 执行异步获取
        wit_bindgen::block_on(async move {
            fetch_domain_config_async(input_value).await;
        });
    } else {
        tracing::info!("域名不包含点号，跳过获取: {}", input_value);
    }
}

async fn fetch_domain_config_async(domain: String) {
    tracing::info!("fetch_domain_config_async 被调用: {}", domain);
    
    // 先显示处理状态
    show_status(StatusState::Processing("正在获取漫画源配置...".to_string())).await;
    
    // 让 UI 有机会更新（50ms延迟）
    timer::set_timeout(50, "").await;
    
    tracing::info!("调用 fetch_source_name: {}", domain);
    match fetch_source_name(&domain).await {
        Some(source_name) => {
            tracing::info!("获取配置成功: {}", source_name);
            // 更新状态
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.fetched_source_name = Some(source_name.clone());
                state.config.source_name = source_name.clone();
            }
            
            // 显示成功状态（会触发 UI 重新渲染）
            show_status(StatusState::Success(format!("获取成功：{}", source_name))).await;
        }
        None => {
            tracing::error!("获取配置失败: {}", domain);
            // 清空 source_name
            {
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.fetched_source_name = None;
                state.config.source_name = String::new();
            }
            
            // 显示错误状态（会触发 UI 重新渲染）
            show_status(StatusState::Error("无法获取漫画源配置，请检查域名是否正确。".to_string())).await;
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
    
    // 验证输入
    show_status(StatusState::Processing("正在验证输入...".to_string())).await;
    
    if cookie.is_empty() {
        show_status(StatusState::Error("Cookie 不能为空。".to_string())).await;
        return;
    }
    
    if domain.is_empty() {
        show_status(StatusState::Error("漫画源域名不能为空。".to_string())).await;
        return;
    }
    
    // 如果没有获取到 source_name，尝试获取
    if source_name.is_none() {
        show_status(StatusState::Processing("正在获取漫画源配置...".to_string())).await;
        
        match fetch_source_name(&domain).await {
            Some(name) => {
                source_name = Some(name.clone());
                
                // 更新状态
                let mut state = ui_state()
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.fetched_source_name = Some(name.clone());
                state.config.source_name = name;
            }
            None => {
                show_status(StatusState::Error("无法获取漫画源配置，请检查域名是否正确。".to_string())).await;
                return;
            }
        }
    }
    
    let source_name = source_name.unwrap();
    
    show_status(StatusState::Processing("正在检查快应用...".to_string())).await;
    
    // 获取已连接设备列表
    let devices = device::get_connected_device_list().await;
    
    if devices.is_empty() {
        show_status(StatusState::Error("没有已连接的设备，请检查手表连接。".to_string())).await;
        return;
    }
    
    let device_addr = &devices[0].addr;
    
    // 获取快应用列表
    let app_list = match thirdpartyapp::get_thirdparty_app_list(device_addr).await {
        Ok(apps) => apps,
        Err(e) => {
            tracing::error!("获取快应用列表失败: {:?}", e);
            show_status(StatusState::Error("无法获取快应用列表。".to_string())).await;
            return;
        }
    };
    
    // 查找腕上漫画应用
    let app = match app_list.iter().find(|a| a.package_name == WATCH_APP_PKG_NAME) {
        Some(a) => a,
        None => {
            show_status(StatusState::Error("请先安装腕上漫画快应用！".to_string())).await;
            return;
        }
    };
    
    // 检查版本
    if app.version_code < 153 {
        show_status(StatusState::Error("请先安装腕上漫画快应用的新版本！".to_string())).await;
        return;
    }
    
    // 启动快应用
    if let Err(e) = thirdpartyapp::launch_qa(device_addr, app, "/pages/index").await {
        tracing::error!("启动快应用失败: {:?}", e);
        show_status(StatusState::Error("启动快应用失败。".to_string())).await;
        return;
    }
    
    // 等待 2 秒
    std::thread::sleep(Duration::from_secs(2));
    
    show_status(StatusState::Processing("正在发送到手表...".to_string())).await;
    
    // 构造 Cookie 数据
    let cookie_data = json!({
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
    
    // 发送消息到快应用
    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME, &cookie_str).await {
        Ok(_) => {
            show_status(StatusState::Success("同步成功！".to_string())).await;
        }
        Err(e) => {
            tracing::error!("发送消息失败: {:?}", e);
            show_status(StatusState::Error("发送失败，请检查手表连接和应用是否打开。".to_string())).await;
        }
    }
}

pub fn handle_interconnect_message(_payload: &str) {
    // 腕上漫画插件不需要处理来自手表的消息
    tracing::info!("收到互联消息，但本插件不处理此类消息");
}
