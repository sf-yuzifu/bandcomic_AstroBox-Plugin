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

/// 快应用通过握手（hs_pong）下发的 APP_SETTING。
/// 缺省值与快应用 app.ux 中 global.APP_SETTING 的默认值保持一致。
#[derive(Debug, Clone)]
pub struct WatchSettings {
    pub search_page_size: u32,
    pub image_quality: u32,
    pub image_size: u32,
    pub show_cover_in_search: bool,
    pub keep_default_zoom: bool,
    pub image_use_png: bool,
    pub image_pre_transcode: bool,
}

impl Default for WatchSettings {
    fn default() -> Self {
        WatchSettings {
            search_page_size: 10,
            image_quality: 50,
            image_size: 600,
            show_cover_in_search: false,
            keep_default_zoom: false,
            image_use_png: false,
            image_pre_transcode: false,
        }
    }
}

impl WatchSettings {
    /// 快应用侧设置项的值可能是字符串或数字/布尔，做兼容解析
    pub fn from_json(v: &Value) -> Self {
        let d = WatchSettings::default();
        let get_u32 = |key: &str, def: u32| -> u32 {
            v.get(key)
                .and_then(|x| {
                    x.as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.parse::<u64>().ok()))
                })
                .map(|n| n as u32)
                .unwrap_or(def)
        };
        let get_bool = |key: &str, def: bool| -> bool {
            v.get(key)
                .and_then(|x| {
                    x.as_bool()
                        .or_else(|| x.as_str().map(|s| s == "true" || s == "1"))
                })
                .unwrap_or(def)
        };
        WatchSettings {
            search_page_size: get_u32("searchPageSize", d.search_page_size),
            image_quality: get_u32("imageQuality", d.image_quality),
            image_size: get_u32("imageSize", d.image_size),
            show_cover_in_search: get_bool("showCoverInSearch", d.show_cover_in_search),
            keep_default_zoom: get_bool("keepDefaultZoom", d.keep_default_zoom),
            image_use_png: get_bool("imageUsePng", d.image_use_png),
            image_pre_transcode: get_bool("imagePreTranscode", d.image_pre_transcode),
        }
    }
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
    pub data: Vec<u8>,        // master image data (resized to MASTER_WIDTH)，发送前按快应用设置再处理
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
    /// 已发送、正在等待 ACK 的分片位置 (file_idx, chunk_idx)；None 表示当前无在途分片
    pub awaiting: Option<(usize, usize)>,
    /// 当前分片的重传次数，超过上限则中止上传
    pub retry_count: u32,
    /// 头部消息原文（用于 ACK 超时重发）
    pub header_str: String,
    /// 快应用是否已确认收到头部（import_header_ack）
    pub header_acked: bool,
    /// 头部重发次数，超过上限退回旧版兼容模式（直接发分片）
    pub header_retry: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ComicInfo {
    pub name: String,
    pub page_count: usize,
    pub chapters: usize,
    pub cover_base64: String,
}

#[derive(Debug, Clone, Default)]
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
    /// 拉取数据整体接收超时定时器 id（request_data 发出后武装）
    pub app_data_recv_timer_id: Option<u64>,
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
    /// 最近一次收到的握手应答 (session, 快应用设置)
    pub hs_pong: Option<(String, WatchSettings)>,
    /// 当前会话生效的快应用设置；None 表示对端是旧版快应用（未握手）
    pub watch_settings: Option<WatchSettings>,
    /// 上传分片 ACK 超时定时器 id
    pub upload_ack_timer_id: Option<u64>,
    /// 上传头部 ACK 超时定时器 id
    pub upload_header_timer_id: Option<u64>,
    /// 已拼完但漫画信息尚未到达（消息乱序）的封面，按漫画名暂存
    /// 值: (封面数据, 插入时间戳秒)，超过 30 秒未补挂的会被清理
    pub pending_covers: HashMap<String, (String, u64)>,
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
            app_data_recv_timer_id: None,
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
            hs_pong: None,
            watch_settings: None,
            upload_ack_timer_id: None,
            upload_header_timer_id: None,
            pending_covers: HashMap::new(),
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
/// 拉取数据整体接收超时定时器事件
pub const APP_DATA_RECV_TIMEOUT_EVENT: &str = "app_data_recv_timeout";
/// 握手：注册重试定时器事件
pub const HS_REGISTER_RETRY_EVENT: &str = "hs_register_retry";
/// 握手：ping 轮询定时器事件
pub const HS_PING_EVENT: &str = "hs_ping_poll";

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
/// 上传分片 ACK 超时重传定时器事件
pub const UPLOAD_ACK_TIMEOUT_EVENT: &str = "upload_ack_timeout";
/// 上传头部 ACK 超时重发定时器事件
pub const UPLOAD_HEADER_TIMEOUT_EVENT: &str = "upload_header_timeout";

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
