use std::sync::{OnceLock, RwLock};
use serde_json::Value;

pub const WATCH_APP_PKG_NAME: &str = "moe.yzf.comic";
pub const CONFIG_KEY_COOKIE: &str = "savedCookie";
pub const CONFIG_KEY_DOMAIN: &str = "sourceDomain";
pub const CONFIG_KEY_SOURCE_NAME: &str = "sourceName";

#[derive(Debug, Clone)]
pub struct PluginConfig {
    pub cookie: String,
    pub domain: String,
    pub source_name: String,
}

impl Default for PluginConfig {
    fn default() -> Self {
        PluginConfig {
            cookie: String::new(),
            domain: String::new(),
            source_name: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatusState {
    Default,
    Processing(String),
    Success(String),
    Error(String),
}

pub struct UiState {
    pub root_element_id: Option<String>,  // 保存根元素ID，用于重新渲染
    pub config: PluginConfig,
    pub fetched_source_name: Option<String>,
    pub fetched_source_config: Option<Value>,
    pub current_status: StatusState,
    pub status_timer_id: Option<u64>,
    pub pending_domain_fetch: Option<String>, // 用于防抖
}

static UI_STATE: OnceLock<RwLock<UiState>> = OnceLock::new();

pub fn ui_state() -> &'static RwLock<UiState> {
    UI_STATE.get_or_init(|| {
        RwLock::new(UiState {
            root_element_id: None,
            config: PluginConfig::default(),
            fetched_source_name: None,
            fetched_source_config: None,
            current_status: StatusState::Default,
            status_timer_id: None,
            pending_domain_fetch: None,
        })
    })
}

// 事件 ID 常量
pub const DOMAIN_INPUT_CHANGE_EVENT: &str = "domain_input_change";
pub const DOMAIN_INPUT_BLUR_EVENT: &str = "domain_input_blur";
pub const COOKIE_INPUT_EVENT: &str = "cookie_input";
pub const SYNC_BUTTON_EVENT: &str = "sync_button";
pub const HIDE_STATUS_EVENT: &str = "hide_status";

// UI 节点 ID 常量
pub const NODE_DOMAIN_LABEL: &str = "domain_label";
pub const NODE_DOMAIN_INPUT: &str = "domain_input";
pub const NODE_SOURCE_NAME_LABEL: &str = "source_name_label";
pub const NODE_SOURCE_NAME_INPUT: &str = "source_name_input";
pub const NODE_COOKIE_LABEL: &str = "cookie_label";
pub const NODE_COOKIE_INPUT: &str = "cookie_input";
pub const NODE_STATUS_MESSAGE: &str = "status_message";
pub const NODE_SYNC_BUTTON: &str = "sync_button";
