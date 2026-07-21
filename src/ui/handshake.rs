//! 握手协议实现
//! 参考 FetchBridge v3 握手协议规范，解决安卓端消息乱序和握手错位问题

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tracing;

use crate::astrobox::psys_host::{device, interconnect, thirdpartyapp, timer};
use crate::ui::state::{WatchSettings, WATCH_APP_PKG_NAME};

/// 单次握手尝试的最长等待时间（毫秒）
const HANDSHAKE_TIMEOUT_MS: u64 = 2500;
/// 最大重试次数
const MAX_HANDSHAKE_ATTEMPTS: u32 = 5;
/// 启动快应用后等待时间（毫秒）- 安卓端需要更长时间完成启动
const LAUNCH_DELAY_MS: u64 = 3000;
/// 注册重试间隔（毫秒）
const REGISTER_RETRY_DELAY_MS: u64 = 2000;
/// 会话空闲超时（10分钟）- 保持与 FetchBridge 一致
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// 握手状态
#[derive(Debug, Clone)]
struct HandshakeSession {
    /// 会话打开状态
    open: bool,
    /// 最后一次活动时间
    last_seen: Instant,
    /// 快应用设置（握手成功后获取）
    settings: Option<WatchSettings>,
}

impl Default for HandshakeSession {
    fn default() -> Self {
        Self {
            open: false,
            last_seen: Instant::now(),
            settings: None,
        }
    }
}

/// 全局握手状态
struct HandshakeState {
    /// 按设备地址管理会话
    sessions: HashMap<String, HandshakeSession>,
}

impl Default for HandshakeState {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

static HANDSHAKE_STATE: OnceLock<Mutex<HandshakeState>> = OnceLock::new();

fn state() -> &'static Mutex<HandshakeState> {
    HANDSHAKE_STATE.get_or_init(|| Mutex::new(HandshakeState::default()))
}

/// 触摸会话，更新活跃时间
fn touch_session(device_addr: &str, open: Option<bool>, settings: Option<WatchSettings>) -> bool {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    let key = device_addr.to_string();

    // 清理过期会话
    guard
        .sessions
        .retain(|_, s| now.duration_since(s.last_seen) <= SESSION_IDLE_TIMEOUT);

    let session = guard.sessions.entry(key).or_insert_with(HandshakeSession::default);
    session.last_seen = now;
    if let Some(open) = open {
        session.open = open;
    }
    if let Some(settings) = settings {
        session.settings = Some(settings);
    }
    session.open
}

/// 检查会话是否已打开
pub fn is_session_open(device_addr: &str) -> bool {
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    guard
        .sessions
        .get(&device_addr.to_string())
        .map(|s| s.open && s.last_seen.elapsed() <= SESSION_IDLE_TIMEOUT)
        .unwrap_or(false)
}

/// 获取已协商的快应用设置
pub fn get_settings(device_addr: &str) -> Option<WatchSettings> {
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let session = guard.sessions.get(&device_addr.to_string())?;
    if !session.open || session.last_seen.elapsed() > SESSION_IDLE_TIMEOUT {
        return None;
    }
    session.settings.clone()
}

/// 记录会话活动（保活）
pub fn record_activity(device_addr: &str) {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    if let Some(session) = guard.sessions.get_mut(&device_addr.to_string()) {
        session.last_seen = now;
        session.open = true;
    }
}

/// 处理 incoming hs_pong 消息
/// 参考 FetchBridge 协议：任何一端收到 count < 2 都回显 count+1
/// 这里 bandcomic 快应用使用 hs_ping/hs_pong 自定义流程
pub fn handle_hs_pong(device_addr: &str, session_id: &str, parsed: &Value) {
    let settings = parsed
        .get("settings")
        .map(WatchSettings::from_json)
        .unwrap_or_default();
    tracing::info!("握手应答收到: session={}, settings={:?}", session_id, settings);
    touch_session(device_addr, Some(true), Some(settings));
}

/// 连接设备并完成握手
/// 遵循 FetchBridge 设计：
/// - 预先检查快应用是否存在并启动
/// - 注册互联接收
/// - 发起握手，使用三握手机制，处理安卓端不稳定情况
/// - 如果已有有效缓存会话直接复用（避免重复握手）
pub async fn connect_and_handshake(min_version: u32) -> Result<Option<WatchSettings>, String> {
    let devices = device::get_connected_device_list().await;
    if devices.is_empty() {
        return Err("没有已连接的设备，请检查手表连接。".to_string());
    }
    let device_addr = devices[0].addr.clone();

    // 复用已有会话（如果存在且未超时）
    if is_session_open(&device_addr) {
        tracing::info!("复用已存在的有效握手会话");
        return Ok(get_settings(&device_addr));
    }

    let app_list = thirdpartyapp::get_thirdparty_app_list(&device_addr)
        .await
        .map_err(|_| "无法获取快应用列表。".to_string())?;
    let app = app_list
        .iter()
        .find(|a| a.package_name == WATCH_APP_PKG_NAME)
        .ok_or_else(|| "请先安装腕上漫画快应用！".to_string())?;
    if app.version_code < min_version {
        return Err("请先安装腕上漫画快应用的新版本！".to_string());
    }
    if thirdpartyapp::launch_qa(&device_addr, app, "/pages/index")
        .await
        .is_err()
    {
        return Err("启动快应用失败。".to_string());
    }

    // 动态等待快应用启动：先尝试注册互联，成功说明已启动，失败则等待后重试
    // 这样比固定延时更精准，适配不同性能的设备
    let mut registered = false;
    let mut total_wait_ms = 0u64;
    const MAX_WAIT_MS: u64 = 5000; // 最多等待 5 秒
    const RETRY_INTERVAL_MS: u64 = 500;

    for attempt in 1..=10 {
        if crate::astrobox::psys_host::register::register_interconnect_recv(&device_addr, WATCH_APP_PKG_NAME)
            .await
            .is_ok()
        {
            tracing::info!("互联注册成功 (第{}次尝试，总等待 {}ms)", attempt, total_wait_ms);
            registered = true;
            break;
        }

        if total_wait_ms >= MAX_WAIT_MS {
            break;
        }

        tracing::debug!("互联注册失败 (第{}次尝试)，等待 {}ms 后重试", attempt, RETRY_INTERVAL_MS);
        timer::set_timeout(RETRY_INTERVAL_MS, "").await;
        total_wait_ms += RETRY_INTERVAL_MS;
    }

    if !registered {
        return Err("无法连接快应用，请确保手表快应用已打开后重试。".to_string());
    }

    // 发起握手 ping
    for attempt in 1..=MAX_HANDSHAKE_ATTEMPTS {
        tracing::info!("握手尝试 {}/{}", attempt, MAX_HANDSHAKE_ATTEMPTS);

        // 每次尝试用新的 session ID
        let session = format!(
            "hs{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );

        // 重置会话状态
        touch_session(&device_addr, Some(false), None);

        let ping_str = json!({
            "type": "hs_ping",
            "session": session,
        })
        .to_string();

        if let Err(e) = interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &ping_str).await {
            tracing::warn!("握手 ping 发送失败(第{}次): {:?}", attempt, e);
            timer::set_timeout(1000, "").await;
            continue;
        }

        // 等待 pong，分段等待避免完全阻塞
        let mut got_response = false;
        for _ in 0..(HANDSHAKE_TIMEOUT_MS / 100) {
            timer::set_timeout(100, "").await;
            if is_session_open(&device_addr) {
                got_response = true;
                break;
            }
        }

        if got_response {
            let settings = get_settings(&device_addr);
            tracing::info!("握手成功(第{}次尝试): settings={:?}", attempt, settings);
            return Ok(settings);
        }

        tracing::warn!("握手第{}次尝试超时", attempt);
        timer::set_timeout(500, "").await;
    }

    tracing::warn!("所有握手尝试失败，按旧版兼容模式继续");
    Ok(None)
}

/// 强制重置握手状态（用于出错后重试）
pub fn reset_session(device_addr: &str) {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    guard.sessions.remove(device_addr);
    tracing::info!("握手会话已重置: {}", device_addr);
}
