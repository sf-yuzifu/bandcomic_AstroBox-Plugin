use std::sync::{OnceLock, RwLock};
use std::collections::HashMap;
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

#[derive(Debug, Clone, PartialEq)]
pub enum TabPage {
    Sync,
    Data,
}

#[derive(Debug, Clone)]
pub struct ComicInfo {
    pub name: String,
    pub page_count: usize,
    pub chapters: usize,
    pub cover_base64: String,
}

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub name: String,
    pub api_url: String,
}

pub struct UiState {
    pub root_element_id: Option<String>,
    pub config: PluginConfig,
    pub fetched_source_name: Option<String>,
    pub fetched_source_config: Option<Value>,
    pub current_status: StatusState,
    pub status_timer_id: Option<u64>,
    pub pending_domain_fetch: Option<String>,
    pub current_tab: TabPage,
    pub app_comic_count: Option<usize>,
    pub app_source_count: Option<usize>,
    pub app_comics: Vec<ComicInfo>,
    pub app_sources: Vec<SourceInfo>,
    pub app_data_status: StatusState,
    pub app_data_timer_id: Option<u64>,
    pub cover_chunk_buffers: HashMap<String, (usize, Vec<String>)>,
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
            current_tab: TabPage::Sync,
            app_comic_count: None,
            app_source_count: None,
            app_comics: Vec::new(),
            app_sources: Vec::new(),
            app_data_status: StatusState::Default,
            app_data_timer_id: None,
            cover_chunk_buffers: HashMap::new(),
        })
    })
}

pub const DOMAIN_INPUT_CHANGE_EVENT: &str = "domain_input_change";
pub const DOMAIN_INPUT_BLUR_EVENT: &str = "domain_input_blur";
pub const COOKIE_INPUT_EVENT: &str = "cookie_input";
pub const SYNC_BUTTON_EVENT: &str = "sync_button";
pub const HIDE_STATUS_EVENT: &str = "hide_status";

pub const TAB_SYNC_EVENT: &str = "tab_sync";
pub const TAB_DATA_EVENT: &str = "tab_data";
pub const FETCH_APP_DATA_EVENT: &str = "fetch_app_data";
pub const HIDE_APP_DATA_STATUS_EVENT: &str = "hide_app_data_status";

pub const NODE_DOMAIN_LABEL: &str = "domain_label";
pub const NODE_DOMAIN_INPUT: &str = "domain_input";
pub const NODE_SOURCE_NAME_LABEL: &str = "source_name_label";
pub const NODE_SOURCE_NAME_INPUT: &str = "source_name_input";
pub const NODE_COOKIE_LABEL: &str = "cookie_label";
pub const NODE_COOKIE_INPUT: &str = "cookie_input";
pub const NODE_STATUS_MESSAGE: &str = "status_message";
pub const NODE_SYNC_BUTTON: &str = "sync_button";

pub const DELETE_COMIC_PREFIX: &str = "delete_comic_";
pub const DELETE_SOURCE_PREFIX: &str = "delete_source_";
