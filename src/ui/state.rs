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
    Upload,
}

#[derive(Debug, Clone)]
pub struct UploadFile {
    pub name: String,
    pub data: Vec<u8>,        // compressed image data (resized to TARGET_WIDTH)
    pub size: usize,          // compressed size
    pub original_size: usize, // original file size before compression
    pub thumbnail: Vec<u8>,   // tiny thumbnail for UI preview
}

#[derive(Debug, Clone)]
pub struct UploadItem {
    pub comic_name: String,
    pub cover: Option<UploadFile>,
    pub files: Vec<UploadFile>,
}

#[derive(Debug, Clone)]
pub struct ChapterItem {
    pub name: String,
    pub files: Vec<UploadFile>,
}

impl Default for ChapterItem {
    fn default() -> Self {
        ChapterItem {
            name: String::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UploadMode {
    Single,
    Multi,
}

#[derive(Debug, Clone)]
pub struct UploadSession {
    pub device_addr: String,
    pub comic_name: String,
    pub all_files: Vec<(String, Vec<String>)>,
    pub current_file: usize,
    pub current_chunk: usize,
    pub total_files: usize,
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
    pub upload_items: Vec<UploadItem>,
    pub upload_chapters: Vec<ChapterItem>,
    pub upload_comic_name_input: String,
    pub upload_mode: UploadMode,
    pub multi_cover: Option<UploadFile>,
    pub upload_progress: f32,
    pub upload_current_file: String,
    pub upload_status: StatusState,
    pub upload_status_timer_id: Option<u64>,
    pub upload_session: Option<UploadSession>,
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
            upload_items: Vec::new(),
            upload_chapters: Vec::new(),
            upload_comic_name_input: String::new(),
            upload_mode: UploadMode::Single,
            multi_cover: None,
            upload_progress: 0.0,
            upload_current_file: String::new(),
            upload_status: StatusState::Default,
            upload_status_timer_id: None,
            upload_session: None,
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

pub const TAB_UPLOAD_EVENT: &str = "tab_upload";
pub const UPLOAD_NAME_INPUT_EVENT: &str = "upload_name_input";
pub const UPLOAD_MODE_SINGLE_EVENT: &str = "upload_mode_single";
pub const UPLOAD_MODE_MULTI_EVENT: &str = "upload_mode_multi";
pub const UPLOAD_PICK_FILES_EVENT: &str = "upload_pick_files";
pub const UPLOAD_START_EVENT: &str = "upload_start";
pub const UPLOAD_CLEAR_EVENT: &str = "upload_clear";
pub const UPLOAD_MOVE_UP_PREFIX: &str = "upload_move_up_";
pub const UPLOAD_MOVE_DOWN_PREFIX: &str = "upload_move_down_";
pub const UPLOAD_DELETE_PREFIX: &str = "upload_delete_";
pub const UPLOAD_PICK_COVER_EVENT: &str = "upload_pick_cover";
pub const HIDE_UPLOAD_STATUS_EVENT: &str = "hide_upload_status";

// 多章节模式
pub const UPLOAD_ADD_CHAPTER_EVENT: &str = "upload_add_chapter";
pub const CHAPTER_NAME_INPUT_PREFIX: &str = "chapter_name_input_";
pub const CHAPTER_PICK_FILES_PREFIX: &str = "chapter_pick_files_";
pub const CHAPTER_UPLOAD_PREFIX: &str = "chapter_upload_";
pub const CHAPTER_CLEAR_PREFIX: &str = "chapter_clear_";
pub const CHAPTER_DELETE_PREFIX: &str = "chapter_delete_";
pub const CHAPTER_MOVE_UP_PREFIX: &str = "chapter_move_up_";
pub const CHAPTER_MOVE_DOWN_PREFIX: &str = "chapter_move_down_";
pub const CHAPTER_DEL_FILE_PREFIX: &str = "chapter_del_file_";

// 多章节封面（整本书一个）
pub const UPLOAD_PICK_MULTI_COVER_EVENT: &str = "upload_pick_multi_cover";
