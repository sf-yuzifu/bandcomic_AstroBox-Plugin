use crate::astrobox::psys_host::timer;
use super::state::{ui_state, StatusState, HIDE_STATUS_EVENT};
use super::build::build_main_ui;

/// 显示状态消息并更新 UI（用于异步上下文）
pub async fn show_status(status: StatusState) {
    let root_id = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        
        // 清除之前的定时器
        if let Some(timer_id) = state.status_timer_id {
            let _ = timer::clear_timer(timer_id).await;
        }
        
        state.current_status = status.clone();
        
        // 如果是成功或错误状态，3秒后自动隐藏
        if matches!(&status, StatusState::Success(_) | StatusState::Error(_)) {
            let timer_id = timer::set_timeout(3000, HIDE_STATUS_EVENT).await;
            state.status_timer_id = Some(timer_id);
        }
        
        state.root_element_id.clone()
    };
    
    // 立即重新渲染 UI
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        crate::astrobox::psys_host::ui::render(&root_id, ui);
    }
}

pub fn hide_status() {
    let root_id = {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        
        state.current_status = StatusState::Default;
        state.status_timer_id = None;
        state.root_element_id.clone()
    };
    
    // 立即重新渲染 UI
    if let Some(root_id) = root_id {
        let ui = build_main_ui();
        crate::astrobox::psys_host::ui::render(&root_id, ui);
    }
}

/// 返回状态文本和颜色
pub fn get_status_text(status: &StatusState) -> (String, String) {
    match status {
        StatusState::Processing(msg) => {
            (msg.clone(), "#1890ff".to_string())
        }
        StatusState::Success(msg) => {
            (msg.clone(), "#52c41a".to_string())
        }
        StatusState::Error(msg) => {
            (format!("错误：{}", msg), "#ff4d4f".to_string())
        }
        StatusState::Default => {
            ("请输入漫画源域名和 Cookie。".to_string(), "#666666".to_string())
        }
    }
}
