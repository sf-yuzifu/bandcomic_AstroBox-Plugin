use std::sync::{OnceLock, RwLock};
use std::collections::{HashMap, HashSet};
use serde_json::Value;

use crate::transfer::{RecvFrontier, WindowedSender};
use crate::sync_receive::{CoverChunks, SyncReceive};
use super::data_browser::{DataBrowser, DataDevice, matching_indices};
use crate::source_config::{SourceForm, SourceSync};

pub const WATCH_APP_PKG_NAME: &str = "moe.yzf.comic";
pub const CONFIG_KEY_COOKIE: &str = "savedCookie";
pub const CONFIG_KEY_DOMAIN: &str = "sourceDomain";
pub const CONFIG_KEY_SOURCE_NAME: &str = "sourceName";

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
    pub fn image_request(&self, role: crate::image_processor::ImageRole) -> crate::image_processor::ImageRequest {
        crate::image_processor::ImageRequest { role, width: self.image_size, quality: self.image_quality,
            if_png: self.image_use_png, if_lvgl: self.image_pre_transcode }
    }
    /// 快应用侧设置项的值可能是字符串或数字/布尔，做兼容解析
    pub fn from_json(v: &Value) -> Self {
        let d = WatchSettings::default();
        let get_u32 = |key: &str, def: u32| -> u32 {
            v.get(key)
                .and_then(|x| {
                    x.as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.parse::<u64>().ok()))
                })
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
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
    pub disk_path: Option<String>, // 磁盘文件路径（HTTP-4），释放内存常驻
    pub data: Vec<u8>,             // 内存兜底数据（无磁盘时或测试使用）
    pub size: usize,               // compressed size
    pub original_size: usize,      // original file size before compression
    pub thumbnail: Vec<u8>,        // tiny thumbnail for UI preview
}

impl UploadFile {
    pub fn get_master_data(&self) -> Result<Vec<u8>, crate::image_processor::ImageProcessError> {
        if let Some(path) = &self.disk_path {
            match crate::assets::read_master(path) {
                Ok(bytes) => return Ok(bytes),
                Err(error) if self.data.is_empty() => return Err(error),
                Err(_) => {}
            }
        }
        if !self.data.is_empty() {
            Ok(self.data.clone())
        } else {
            Err(crate::image_processor::ImageProcessError::new(crate::image_processor::ErrorStage::Read,
                "图片数据丢失（未在内存且无法读取磁盘）"))
        }
    }
}

#[derive(Debug, Clone)]
pub struct UploadItem {
    pub comic_name: String,
    pub cover: Option<UploadFile>,
    pub files: Vec<UploadFile>,
}

#[derive(Debug, Clone)]
pub struct ChapterItem {
    pub number: usize,
    pub name: String,
    pub files: Vec<UploadFile>,
}

impl Default for ChapterItem {
    fn default() -> Self {
        ChapterItem {
            number: 1,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadView {
    Overview,
    Info,
    Pages(Option<usize>),
    Cover,
    Connection,
}

pub const PAGE_WINDOW: usize = 8;

pub fn page_window(cursor: usize, total: usize) -> (usize, usize) {
    let last = total.saturating_sub(1) / PAGE_WINDOW;
    let start = cursor.min(last) * PAGE_WINDOW;
    (start, (start + PAGE_WINDOW).min(total))
}

/// 图片选择目标（对话框返回后暂存，处理推迟到定时器事件）
#[derive(Debug, Clone, Copy)]
pub enum PickTarget {
    /// 上传列表（单本模式入 upload_items，多章模式入最后一个章节）
    UploadItem,
    /// 指定章节的图片
    Chapter(usize),
    /// 单本模式封面
    CoverSingle,
    /// 多章模式书级封面
    CoverMulti,
}

/// 已选待处理的图片（原图字节，处理时才解码缩放）
pub struct PendingPick {
    pub name: String,
    pub data: Vec<u8>,
    pub target: PickTarget,
}

/// 窗口模式上传会话（滑窗 + 累计 ACK，移植自 InterconnectFetch transfer.rs）
#[derive(Debug, Clone)]
pub struct WindowedUpload {
    pub sender: WindowedSender,
    /// gseq → (file_idx, chunk_idx) 扁平映射，跨文件统一编号
    pub order: Vec<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct UploadSession {
    pub device_addr: String,
    pub comic_name: String,
    pub session_id: String,
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
    /// 窗口模式；Some 时走滑动窗口 + 累计 ACK（awaiting 字段闲置），
    /// None 表示旧版逐片停等
    pub windowed: Option<WindowedUpload>,
}

#[derive(Debug, Clone, Default)]
pub struct ComicInfo {
    pub id: String,
    pub name: String,
    pub page_count: usize,
    pub chapters: usize,
    pub cover_base64: String,
}

#[derive(Debug, Clone, Default)]
pub struct SourceInfo {
    pub key: String,
    pub name: String,
    pub api_url: String,
}

pub struct UiState {
    pub root_element_id: Option<String>,
    pub source_form: SourceForm,
    pub source_sync: Option<SourceSync>,
    pub source_sync_next: u64,
    pub current_status: StatusState,
    pub status_timer_id: Option<u64>,
    pub current_tab: TabPage,
    pub app_comic_count: Option<usize>,
    pub app_source_count: Option<usize>,
    pub app_comics: Vec<ComicInfo>,
    pub app_sources: Vec<SourceInfo>,
    pub data_browser: DataBrowser,
    pub deletes: super::deletion::DeleteManager,
    /// Only an accepted handshake for this address may enable stable deletion.
    pub watch_delete: Option<(String, String)>,
    pub comic_search: String,
    pub source_search: String,
    pub app_data_status: StatusState,
    pub app_data_timer_id: Option<u64>,
    /// 无进展看门狗：收到有效数据只更新截止时间，不每片调用宿主定时器。
    pub app_data_recv_timer_id: Option<u64>,
    pub sync_receive: SyncReceive,
    pub http_data_sync: Option<crate::http_data_sync::HttpDataSync>,
    pub watch_sync_session: bool,
    pub sync_comics_seen: HashSet<usize>,
    pub sync_sources_seen: HashSet<usize>,
    pub cover_chunk_buffers: HashMap<String, CoverChunks>,
    pub upload_items: Vec<UploadItem>,
    pub upload_book_id: String,
    pub upload_target: Option<ImportTarget>,
    pub upload_chapters: Vec<ChapterItem>,
    pub upload_comic_name_input: String,
    pub upload_mode: UploadMode,
    pub upload_view: UploadView,
    pub upload_page_cursor: usize,
    pub comic_page_cursor: usize,
    pub source_page_cursor: usize,
    pub single_cover_follows_first: bool,
    pub multi_cover_follows_first: bool,
    pub multi_cover: Option<UploadFile>,
    pub upload_progress: f32,
    pub upload_current_file: String,
    pub upload_status: StatusState,
    pub image_notices: Vec<String>,
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
    /// 手表通过 hs_pong caps 声明的导入接收窗口；None 表示旧版快应用（逐片停等）
    pub watch_import_window: Option<usize>,
    /// 方向 B 滑窗接收前沿：手表→插件的同步帧按 gseq 乱序缓存、按序消费
    pub sync_recv: Option<RecvFrontier>,
    /// 已选待处理的图片（对话框与 CPU 密集处理解耦，见 handle_pick_process）
    pub pending_pick: Option<PendingPick>,
    /// 手表端是否支持原生 HTTP 导入能力（通过 hs_pong caps 协商）
    pub watch_http_import: bool,
    pub watch_http_data_sync: bool,
    /// 手表端是否支持旧互联导入结果回报（通过 hs_pong caps 协商）
    pub watch_import_result: bool,
    pub watch_chapter_import: bool,
    pub upload_result_session: Option<String>,
    pub upload_result_timer_id: Option<u64>,
}

static UI_STATE: OnceLock<RwLock<UiState>> = OnceLock::new();

pub fn ui_state() -> &'static RwLock<UiState> {
    UI_STATE.get_or_init(|| RwLock::new(UiState::default()))
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            root_element_id: None,
            source_form: SourceForm::default(),
            source_sync: None,
            source_sync_next: 0,
            current_status: StatusState::Default,
            status_timer_id: None,
            current_tab: TabPage::Upload,
            app_comic_count: None,
            app_source_count: None,
            app_comics: Vec::new(),
            app_sources: Vec::new(),
            data_browser: DataBrowser::default(),
            deletes: super::deletion::DeleteManager::default(),
            watch_delete: None,
            comic_search: String::new(),
            source_search: String::new(),
            app_data_status: StatusState::Default,
            app_data_timer_id: None,
            app_data_recv_timer_id: None,
            sync_receive: SyncReceive::default(),
            http_data_sync: None,
            watch_sync_session: false,
            sync_comics_seen: HashSet::new(),
            sync_sources_seen: HashSet::new(),
            cover_chunk_buffers: HashMap::new(),
            upload_items: Vec::new(),
            upload_book_id: new_book_id(),
            upload_target: None,
            upload_chapters: Vec::new(),
            upload_comic_name_input: String::new(),
            upload_mode: UploadMode::Single,
            upload_view: UploadView::Overview,
            upload_page_cursor: 0,
            comic_page_cursor: 0,
            source_page_cursor: 0,
            single_cover_follows_first: true,
            multi_cover_follows_first: true,
            multi_cover: None,
            upload_progress: 0.0,
            upload_current_file: String::new(),
            upload_status: StatusState::Default,
            image_notices: Vec::new(),
            upload_status_timer_id: None,
            upload_session: None,
            hs_pong: None,
            watch_settings: None,
            upload_ack_timer_id: None,
            upload_header_timer_id: None,
            pending_covers: HashMap::new(),
            watch_import_window: None,
            sync_recv: None,
            pending_pick: None,
            watch_http_import: false,
            watch_http_data_sync: false,
            watch_import_result: false,
            watch_chapter_import: false,
            upload_result_session: None,
            upload_result_timer_id: None,
        }
    }
}

pub const DOMAIN_INPUT_CHANGE_EVENT: &str = "domain_input_change";
pub const SOURCE_FETCH_EVENT: &str = "source_fetch";
pub const SOURCE_SYNC_PREFIX: &str = "source_sync_";
pub const SOURCE_SELECT_PREFIX: &str = "source_select_";
pub const SOURCE_COOKIE_INPUT_PREFIX: &str = "source_cookie_input_";
pub const SOURCE_COOKIE_KEEP_PREFIX: &str = "source_cookie_keep_";
pub const SOURCE_COOKIE_UPDATE_PREFIX: &str = "source_cookie_update_";
pub const SOURCE_COOKIE_CLEAR_PREFIX: &str = "source_cookie_clear_";
pub const SOURCE_ALL_PREFIX: &str = "source_all_";
pub const SOURCE_NONE_PREFIX: &str = "source_none_";
pub const SOURCE_CATALOG_PREV: &str = "source_catalog_prev";
pub const SOURCE_CATALOG_NEXT: &str = "source_catalog_next";
pub const SYNC_BUTTON_EVENT: &str = "sync_button";
pub const HIDE_STATUS_EVENT: &str = "hide_status";

pub const TAB_SYNC_EVENT: &str = "tab_sync";
pub const TAB_DATA_EVENT: &str = "tab_data";
pub const FETCH_APP_DATA_EVENT: &str = "fetch_app_data";
pub const COMIC_SEARCH_EVENT: &str = "comic_search";
pub const SOURCE_SEARCH_EVENT: &str = "source_search";
pub const COMIC_SEARCH_CLEAR_EVENT: &str = "comic_search_clear";
pub const SOURCE_SEARCH_CLEAR_EVENT: &str = "source_search_clear";
pub const HIDE_APP_DATA_STATUS_EVENT: &str = "hide_app_data_status";
/// 拉取数据整体接收超时定时器事件
pub const APP_DATA_RECV_TIMEOUT_EVENT: &str = "app_data_recv_timeout:";
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
/// 导入结果等待超时定时器事件与超时时间（毫秒）
pub const UPLOAD_RESULT_TIMEOUT_EVENT: &str = "upload_result_timeout:";
pub const UPLOAD_RESULT_TIMEOUT_MS: u64 = 25000;
/// 图片选取结果处理定时器事件（对话框关闭后延迟一拍再做解码缩放）
pub const PICK_PROCESS_EVENT: &str = "pick_process";

// 多章节模式
pub const UPLOAD_ADD_CHAPTER_EVENT: &str = "upload_add_chapter";
pub const CHAPTER_NAME_INPUT_PREFIX: &str = "chapter_name_input_";
pub const CHAPTER_NUMBER_INPUT_PREFIX: &str = "chapter_number_input_";
pub const IMPORT_TARGET_PREFIX: &str = "import_target_";
pub const IMPORT_TARGET_CLEAR: &str = "import_target_clear";
pub const CHAPTER_PICK_FILES_PREFIX: &str = "chapter_pick_files_";
pub const CHAPTER_UPLOAD_PREFIX: &str = "chapter_upload_";
pub const CHAPTER_CLEAR_PREFIX: &str = "chapter_clear_";
pub const CHAPTER_DELETE_PREFIX: &str = "chapter_delete_";
pub const CHAPTER_MOVE_UP_PREFIX: &str = "chapter_move_up_";
pub const CHAPTER_MOVE_DOWN_PREFIX: &str = "chapter_move_down_";
pub const CHAPTER_DEL_FILE_PREFIX: &str = "chapter_del_file_";

// 多章节封面（整本书一个）
pub const UPLOAD_PICK_MULTI_COVER_EVENT: &str = "upload_pick_multi_cover";

pub const UPLOAD_OVERVIEW_EVENT: &str = "upload_overview";
pub const UPLOAD_INFO_EVENT: &str = "upload_info";
pub const UPLOAD_PAGES_EVENT: &str = "upload_pages";
pub const UPLOAD_COVER_EVENT: &str = "upload_cover";
pub const UPLOAD_CONNECTION_EVENT: &str = "upload_connection";
pub const UPLOAD_COVER_FIRST_EVENT: &str = "upload_cover_first";
pub const UPLOAD_COVER_NONE_EVENT: &str = "upload_cover_none";
pub const UPLOAD_PAGE_PREV_EVENT: &str = "upload_page_prev";
pub const UPLOAD_PAGE_NEXT_EVENT: &str = "upload_page_next";
pub const CHAPTER_EDIT_PREFIX: &str = "chapter_edit_";
pub const COMIC_PAGE_PREV_EVENT: &str = "comic_page_prev";
pub const COMIC_PAGE_NEXT_EVENT: &str = "comic_page_next";
pub const SOURCE_PAGE_PREV_EVENT: &str = "source_page_prev";
pub const SOURCE_PAGE_NEXT_EVENT: &str = "source_page_next";

impl UiState {
    pub fn add_chapter(&mut self) {
        let number = self.upload_chapters.iter().map(|c| c.number).max().unwrap_or(0) + 1;
        self.upload_chapters.push(ChapterItem { number, ..ChapterItem::default() });
    }
    pub fn import_plan(&self, device_addr: &str, chapter: Option<usize>) -> Result<Option<crate::jobs::ImportPlan>, String> {
        if let Some(target) = &self.upload_target {
            if target.device_addr != device_addr { return Err("导入目标属于另一设备，请重新读取书架选择目标".into()); }
            if !self.watch_chapter_import { return Err("该快应用不支持按 ID 追加/替换，请更新快应用".into()); }
            if chapter.is_some() && !target.is_serial { return Err("章节追加目标必须是连载作品".into()); }
        }
        let numbers: Vec<_> = self.upload_chapters.iter().filter(|c| !c.files.is_empty()).map(|c| c.number).collect();
        if self.upload_mode == UploadMode::Multi && (numbers.iter().any(|n| *n == 0 || *n > 100000) ||
            numbers.iter().copied().collect::<HashSet<_>>().len() != numbers.len()) {
            return Err("真实章号必须为 1..100000 且不能重复".into());
        }
        if !self.watch_chapter_import { return Ok(None); }
        Ok(Some(crate::jobs::ImportPlan { import_chapter_protocol: 1, book_id: self.upload_book_id.clone(),
            target_comic_id: self.upload_target.as_ref().map(|t| t.id.clone()),
            operation: if chapter.is_some() { "upsert_chapters" } else { "replace_book" }.into(),
            is_serial: self.upload_mode == UploadMode::Multi }))
    }

    pub fn select_import_target(&mut self, revision: u64, index: usize) -> bool {
        if self.upload_locked() || !self.data_browser.owner_matches_connection() { return false; }
        let Some(target) = self.capture_data_target(revision, index, false) else { return false; };
        let DataItem::Comic { id, name } = target.item else { return false; };
        if !id.starts_with("local_") { return false; }
        let comic = &self.app_comics[index];
        self.upload_target = Some(ImportTarget { device_addr: target.owner.addr, device_name: target.owner.name, id, name,
            is_serial: comic.chapters > 0, page_count: comic.page_count, chapters: comic.chapters,
            cover_base64: comic.cover_base64.clone() });
        self.current_tab = TabPage::Upload;
        self.upload_view = UploadView::Overview;
        true
    }
    pub fn source_sync_busy(&self) -> bool { self.source_sync.as_ref().is_some_and(|sync| sync.phase.busy()) }

    pub fn source_sync_current(&self, id: u64) -> bool {
        self.source_sync.as_ref().is_some_and(|sync| sync.id == id && sync.phase.busy() && self.source_form.plan_current(&sync.plan))
    }
    pub fn comic_matches(&self) -> Vec<usize> {
        matching_indices(self.app_comics.iter().map(|c| c.name.as_str()), &self.comic_search)
    }

    pub fn source_matches(&self) -> Vec<usize> {
        matching_indices(self.app_sources.iter().map(|s| s.name.as_str()), &self.source_search)
    }

    pub fn clamp_data_pages(&mut self) {
        self.comic_page_cursor = self.comic_page_cursor.min(self.comic_matches().len().saturating_sub(1) / PAGE_WINDOW);
        self.source_page_cursor = self.source_page_cursor.min(self.source_matches().len().saturating_sub(1) / PAGE_WINDOW);
    }

    pub fn accept_library_data(&mut self) {
        if self.data_browser.accept_data() {
            self.app_comics.clear();
            self.app_sources.clear();
            self.app_comic_count = None;
            self.app_source_count = None;
            self.sync_comics_seen.clear();
            self.sync_sources_seen.clear();
        }
    }

    pub fn update_library_completeness(&mut self) {
        self.data_browser.lists_complete = self.data_browser.lists_done &&
            self.app_comic_count.is_some_and(|count| self.sync_comics_seen.len() == count &&
                self.sync_comics_seen.iter().all(|index| *index < count)) &&
            self.app_source_count.is_some_and(|count| self.sync_sources_seen.len() == count &&
                self.sync_sources_seen.iter().all(|index| *index < count));
    }

    pub fn finish_library_sync(&mut self, skipped: Option<usize>) {
        self.update_library_completeness();
        let covers = self.app_comics.iter().filter(|c| !c.cover_base64.is_empty()).count();
        self.data_browser.finish(super::data_browser::now_seconds(), covers, skipped);
        self.clamp_data_pages();
        if self.data_browser.lists_complete && let Some(owner) = &self.data_browser.owner {
            self.deletes.verify_snapshot(&owner.addr, self.data_browser.revision, &self.app_comics, &self.app_sources);
        }
    }

    pub fn capture_data_target(&self, revision: u64, index: usize, source: bool) -> Option<DataTarget> {
        if self.data_browser.revision != revision || self.data_browser.busy() || !self.data_browser.lists_complete { return None; }
        let owner = self.data_browser.owner.clone()?;
        let item = if source {
            let s = self.app_sources.get(index)?;
            DataItem::Source { key: s.key.clone(), name: s.name.clone(), api_url: s.api_url.clone() }
        } else {
            let c = self.app_comics.get(index)?;
            DataItem::Comic { id: c.id.clone(), name: c.name.clone() }
        };
        if item.name().is_empty() { return None; }
        Some(DataTarget { revision, owner, item })
    }

    pub fn apply_delete_success(&mut self, target: &DataTarget) -> bool {
        if !self.data_target_current(target) || !self.data_browser.owner_matches_connection() { return false; }
        match &target.item {
            DataItem::Comic { id, .. } => {
                if self.app_comics.iter().filter(|c| c.id == *id).count() != 1 { return false; }
                let skipped = self.app_comics.iter().find(|c| c.id == *id).is_some_and(|c| c.cover_base64.is_empty());
                self.app_comics.retain(|c| c.id != *id);
                if skipped { self.data_browser.covers_skipped = self.data_browser.covers_skipped.map(|n| n.saturating_sub(1)); }
                self.data_browser.covers_received = self.app_comics.iter().filter(|c| !c.cover_base64.is_empty()).count();
                self.app_comic_count = Some(self.app_comics.len());
                self.sync_comics_seen = (0..self.app_comics.len()).collect();
            }
            DataItem::Source { key, .. } => {
                if key.is_empty() || self.app_sources.iter().filter(|s| s.key == *key).count() != 1 { return false; }
                self.app_sources.retain(|s| s.key != *key);
                self.app_source_count = Some(self.app_sources.len());
                self.sync_sources_seen = (0..self.app_sources.len()).collect();
            }
        }
        self.data_browser.revision = self.data_browser.revision.wrapping_add(1);
        self.clamp_data_pages();
        true
    }

    pub fn data_target_current(&self, target: &DataTarget) -> bool {
        self.data_browser.revision == target.revision && !self.data_browser.busy() && self.data_browser.lists_complete &&
            self.data_browser.owner.as_ref().is_some_and(|owner| owner.addr == target.owner.addr) &&
            match &target.item {
                DataItem::Comic { id, name } => self.app_comics.iter().any(|c| c.id == *id && c.name == *name),
                DataItem::Source { key, name, api_url } => self.app_sources.iter().any(|s| s.key == *key && s.name == *name && s.api_url == *api_url),
            }
    }

    pub fn upload_locked(&self) -> bool {
        self.pending_pick.is_some() || matches!(self.upload_status, StatusState::Processing(_))
    }

    pub fn page_count(&self) -> usize {
        match self.upload_mode {
            UploadMode::Single => self.upload_items.iter().map(|i| i.files.len()).sum(),
            UploadMode::Multi => self.upload_chapters.iter().map(|c| c.files.len()).sum(),
        }
    }

    pub fn single_page_location(&self, mut index: usize) -> Option<(usize, usize)> {
        for (item, entry) in self.upload_items.iter().enumerate() {
            if index < entry.files.len() { return Some((item, index)); }
            index -= entry.files.len();
        }
        None
    }

    pub fn move_single_page(&mut self, index: usize, direction: i32) {
        let Some(next) = index.checked_add_signed(direction as isize) else { return; };
        let (Some((item, page)), Some((other, next_page))) =
            (self.single_page_location(index), self.single_page_location(next)) else { return; };
        if item == other {
            self.upload_items[item].files.swap(page, next_page);
        } else {
            let replacement = self.upload_items[other].files[next_page].clone();
            let previous = std::mem::replace(&mut self.upload_items[item].files[page], replacement);
            self.upload_items[other].files[next_page] = previous;
        }
        self.refresh_auto_covers();
    }

    pub fn delete_single_page(&mut self, index: usize) {
        if let Some((item, page)) = self.single_page_location(index) {
            self.upload_items[item].files.remove(page);
            self.refresh_auto_covers();
        }
    }

    pub fn refresh_auto_covers(&mut self) {
        if self.single_cover_follows_first {
            let first = self.upload_items.iter().flat_map(|i| &i.files).next().cloned();
            if let Some(item) = self.upload_items.first_mut() { item.cover = first; }
        }
        if self.multi_cover_follows_first {
            self.multi_cover = self.upload_chapters.iter().flat_map(|c| &c.files).next().cloned();
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImportTarget {
    pub device_addr: String,
    pub device_name: String,
    pub id: String,
    pub name: String,
    pub is_serial: bool,
    pub page_count: usize,
    pub chapters: usize,
    pub cover_base64: String,
}

pub fn new_book_id() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("book_{}_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

pub fn sanitize_import_name(name: &str) -> String {
    name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

#[derive(Debug, Clone)]
pub enum DataItem {
    Comic { id: String, name: String },
    Source { key: String, name: String, api_url: String },
}

impl DataItem {
    pub fn name(&self) -> &str {
        match self { Self::Comic { name, .. } | Self::Source { name, .. } => name }
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Comic { id, .. }, Self::Comic { id: other, .. }) => id == other,
            (Self::Source { key, .. }, Self::Source { key: other, .. }) => !key.is_empty() && key == other,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DataTarget {
    pub revision: u64,
    pub owner: DataDevice,
    pub item: DataItem,
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    fn file(name: &str) -> UploadFile {
        UploadFile { name: name.into(), disk_path: None, data: vec![1], size: 1,
            original_size: 1, thumbnail: vec![] }
    }

    #[test]
    fn page_edit_preserves_book_and_tracks_cover_without_removing_body() {
        let mut state = UiState::default();
        state.upload_items = vec![UploadItem { comic_name: "测试书".into(), cover: None,
            files: vec![file("1"), file("2"), file("3")] }];
        state.single_cover_follows_first = true;
        state.refresh_auto_covers();
        assert_eq!(state.upload_items[0].files.len(), 3);
        assert_eq!(state.upload_items[0].cover.as_ref().unwrap().name, "1");
        state.move_single_page(1, -1);
        assert_eq!(state.upload_items[0].cover.as_ref().unwrap().name, "2");
        state.delete_single_page(0);
        assert_eq!(state.upload_items[0].comic_name, "测试书");
        assert_eq!(state.upload_items[0].files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["1", "3"]);
        state.single_cover_follows_first = false;
        state.upload_items[0].cover = Some(file("独立封面"));
        state.delete_single_page(0);
        assert_eq!(state.upload_items[0].cover.as_ref().unwrap().name, "独立封面");
        state.move_single_page(0, -1);
        state.move_single_page(0, 1);
        assert_eq!(state.upload_items[0].files[0].name, "3");
        state.upload_items.clear();
        state.single_cover_follows_first = true;
    }

    #[test]
    fn pagination_clamps_after_last_page_removal() {
        assert_eq!(page_window(12, 0), (0, 0));
        assert_eq!(page_window(12, 17), (16, 17));
        assert_eq!(page_window(2, 16), (8, 16));
    }

    #[test]
    fn chapter_identity_survives_removal_and_book_identity_survives_repeated_plans() {
        let mut state = UiState::default();
        state.upload_mode = UploadMode::Multi;
        state.watch_chapter_import = true;
        state.add_chapter(); state.add_chapter();
        state.upload_chapters[0].files.push(file("1"));
        state.upload_chapters[1].files.push(file("2"));
        let first = state.import_plan("A", Some(0)).unwrap().unwrap();
        state.upload_chapters.remove(0);
        assert_eq!(state.upload_chapters[0].number, 2);
        let next = state.import_plan("A", Some(0)).unwrap().unwrap();
        assert_eq!(first.book_id, next.book_id);
        assert_eq!(next.operation, "upsert_chapters");
        state.add_chapter(); assert_eq!(state.upload_chapters[1].number, 3);
        assert_eq!(state.import_plan("A", None).unwrap().unwrap().operation, "replace_book");
        assert!(serde_json::to_value(next).unwrap()["isSerial"].as_bool().unwrap());
    }

    #[test]
    fn chapter_import_requires_unique_numbers_capability_and_target_device() {
        let mut state = UiState::default(); state.upload_mode = UploadMode::Multi;
        state.add_chapter(); state.add_chapter();
        for chapter in &mut state.upload_chapters { chapter.files.push(file("page")); }
        assert!(state.import_plan("A", Some(0)).unwrap().is_none());
        state.upload_target = Some(ImportTarget { device_addr: "A".into(), id: "local_old".into(), name: "旧书".into(), is_serial: true, ..Default::default() });
        assert!(state.import_plan("A", Some(0)).is_err());
        state.watch_chapter_import = true;
        assert!(state.import_plan("B", Some(0)).is_err());
        assert_eq!(state.import_plan("A", Some(0)).unwrap().unwrap().target_comic_id.as_deref(), Some("local_old"));
        state.upload_chapters[1].number = 1; assert!(state.import_plan("A", None).is_err());
        state.upload_chapters[1].number = 0; assert!(state.import_plan("A", None).is_err());
        assert_eq!(sanitize_import_name("章:/?名"), "章___名");
    }

    #[test]
    fn missing_disk_master_is_a_read_error_and_valid_memory_fallback_is_explicit() {
        let mut file = file("missing");
        file.disk_path = Some("cache/missing-ui-image-test.png".into());
        file.data.clear();
        assert!(file.get_master_data().unwrap_err().to_string().contains("读取母版图片失败"));
        file.data = vec![1, 2, 3];
        assert_eq!(file.get_master_data().unwrap(), vec![1, 2, 3]);
    }
}

#[cfg(test)]
mod data_tests {
    use super::*;
    use super::super::data_browser::DataPhase;

    fn device(addr: &str) -> DataDevice { DataDevice { name: addr.into(), addr: addr.into() } }

    #[test]
    fn filtered_pages_keep_slots_and_covers_do_not_reset_queries_or_page() {
        let mut state = UiState::default();
        state.app_comics = (0..40).map(|i| ComicInfo { id: i.to_string(),
            name: format!("{} {i}", if i % 2 == 0 { "Book" } else { "Other" }), ..Default::default() }).collect();
        state.app_sources = vec![SourceInfo { name: "中文源".into(), api_url: "test".into(), ..Default::default() }];
        state.comic_search = " BOOK ".into();
        state.source_search = "中文".into();
        state.comic_page_cursor = 2;
        let matches = state.comic_matches();
        assert_eq!(matches.len(), 20);
        let (start, end) = page_window(state.comic_page_cursor, matches.len());
        assert_eq!(&matches[start..end], &[32, 34, 36, 38]);
        state.app_comics[32].cover_base64 = "cover".into();
        state.clamp_data_pages();
        assert_eq!(state.comic_page_cursor, 2);
        assert_eq!(state.source_matches(), vec![0]);
        state.app_comics.truncate(10);
        state.clamp_data_pages();
        assert_eq!(state.comic_page_cursor, 0);
        assert_eq!(state.comic_search, " BOOK ");
    }

    #[test]
    fn dialog_target_survives_filtering_but_rejects_new_snapshot_and_different_owner() {
        let mut state = UiState::default();
        state.data_browser.begin(device("A"));
        state.accept_library_data();
        state.data_browser.phase = DataPhase::Finished;
        state.data_browser.lists_complete = true;
        state.app_comics = vec![ComicInfo { id: "one".into(), name: "Same".into(), ..Default::default() },
            ComicInfo { id: "two".into(), name: "Same".into(), ..Default::default() }];
        let target = state.capture_data_target(state.data_browser.revision, 1, false).unwrap();
        assert!(matches!(&target.item, DataItem::Comic { id, .. } if id == "two"));
        state.comic_search = "none".into();
        state.app_comics.swap(0, 1);
        assert!(state.data_target_current(&target));
        state.data_browser.lists_complete = false;
        assert!(!state.data_target_current(&target));
        assert!(state.capture_data_target(target.revision, 0, false).is_none());
        state.data_browser.lists_complete = true;
        state.data_browser.owner = Some(device("B"));
        assert!(!state.data_target_current(&target));
        state.data_browser.owner = Some(device("A"));
        state.data_browser.begin(device("A"));
        assert!(!state.data_target_current(&target));
        assert!(state.capture_data_target(target.revision, 1, false).is_none());
    }

    #[test]
    fn declared_list_length_is_not_evidence_of_received_metadata() {
        let mut state = UiState::default();
        state.data_browser.begin(device("A"));
        state.accept_library_data();
        state.app_comic_count = Some(2);
        state.app_source_count = Some(1);
        state.app_comics.resize(2, ComicInfo::default());
        state.app_sources.resize(1, SourceInfo::default());
        state.sync_comics_seen.insert(1);
        state.sync_sources_seen.insert(0);
        state.data_browser.lists_done = true;
        state.finish_library_sync(None);
        assert!(!state.data_browser.lists_complete);
        assert!(state.data_browser.complete_time().is_none());
        state.sync_comics_seen.insert(0);
        state.update_library_completeness();
        assert!(state.data_browser.lists_complete);
        state.sync_comics_seen.remove(&1);
        state.sync_comics_seen.insert(2);
        state.update_library_completeness();
        assert!(!state.data_browser.lists_complete, "same count with an out-of-range slot is not complete");
    }

    #[test]
    fn first_unordered_data_switches_owner_once_and_preserves_filters() {
        let mut state = UiState::default();
        state.data_browser.owner = Some(device("A"));
        state.app_comics.push(ComicInfo { name: "old".into(), ..Default::default() });
        state.comic_search = "new".into();
        state.data_browser.begin(device("B"));
        assert_eq!(state.app_comics.len(), 1);
        state.accept_library_data();
        state.app_comics.push(ComicInfo { name: "new".into(), ..Default::default() });
        state.accept_library_data();
        assert_eq!(state.app_comics.len(), 1);
        assert_eq!(state.app_comics[0].name, "new");
        assert_eq!(state.data_browser.owner.as_ref().unwrap().addr, "B");
        assert_eq!(state.comic_matches(), vec![0]);
    }
}
