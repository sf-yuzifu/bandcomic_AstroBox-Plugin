//! 握手协议实现
//! 参考 FetchBridge v3 握手协议规范，解决安卓端消息乱序和握手错位问题

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tracing;

use crate::astrobox::psys_host::{device, interconnect, thirdpartyapp, timer};
use crate::ui::state::{
    WatchSettings, WATCH_APP_PKG_NAME, HS_PING_EVENT, HS_REGISTER_RETRY_EVENT,
};

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
    touch_session(device_addr, Some(true), Some(settings.clone()));

    // pong 到达即连接就绪：完成挂起的握手会话，推进业务续体
    let is_pending = {
        pending()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|ph| ph.phase == HandshakePhase::Pinging)
            .unwrap_or(false)
    };
    if is_pending {
        tracing::info!("握手成功");
        complete(Ok(Some(settings)));
    }
}

/// 注册重试间隔（毫秒）
const REGISTER_RETRY_INTERVAL_MS: u64 = 300;
/// 注册重试总超时（毫秒）
const REGISTER_RETRY_TIMEOUT_MS: u64 = 5000;
/// 握手 ping 轮询间隔（毫秒）
const HANDSHAKE_POLL_INTERVAL_MS: u64 = 500;
/// 握手 ping 轮询整体超时（毫秒）
const HANDSHAKE_POLL_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum HandshakePhase {
    /// 等待快应用启动完成（注册重试阶段）
    Registering,
    /// 等待快应用应答握手（ping 轮询阶段）
    Pinging,
}

/// 挂起的握手会话。
/// 宿主的 set_timeout 只是注册定时器（立即返回 timer id），没有 async sleep，
/// 所以所有等待都必须由定时器事件驱动：注册重试和 ping 轮询各自挂在
/// HS_REGISTER_RETRY_EVENT / HS_PING_EVENT 上逐步推进，pong 由消息事件完成。
struct PendingHandshake {
    device_addr: String,
    ping_str: String,
    phase: HandshakePhase,
    register_waited_ms: u64,
    ping_waited_ms: u64,
    attempt: u32,
    progress: Box<dyn Fn(String) + Send>,
    on_done: Option<Box<dyn FnOnce(Result<Option<WatchSettings>, String>) + Send>>,
}

static PENDING: OnceLock<Mutex<Option<PendingHandshake>>> = OnceLock::new();

fn pending() -> &'static Mutex<Option<PendingHandshake>> {
    PENDING.get_or_init(|| Mutex::new(None))
}

/// 完成挂起的握手会话并调用业务回调。
/// 调用方必须保证不在 block_on 内部调用（回调里会再开 block_on 跑业务续体）。
fn complete(result: Result<Option<WatchSettings>, String>) {
    let ph = {
        pending()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    };
    if let Some(mut ph) = ph {
        if let Some(cb) = ph.on_done.take() {
            cb(result);
        }
    }
}

/// 第一阶段：检查并启动快应用。只含无等待的 FFI 调用，
/// 可在调用方的 async 上下文中直接 await。
pub async fn prepare_launch(
    min_version: u32,
    progress: &dyn Fn(String),
) -> Result<String, String> {
    let devices = device::get_connected_device_list().await;
    if devices.is_empty() {
        return Err("没有已连接的设备，请检查手表连接。".to_string());
    }
    let device_addr = devices[0].addr.clone();

    progress("正在检查快应用...".to_string());
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
    progress("正在启动快应用...".to_string());
    Ok(device_addr)
}

/// 第二阶段：注册挂起的握手会话并启动事件驱动状态机。
/// 立即返回（首个注册尝试由 10ms 后的定时器事件触发）；
/// 握手完成/失败/超时后通过 on_done 回调业务续体。
pub async fn begin_wait(
    device_addr: String,
    progress: impl Fn(String) + Send + 'static,
    on_done: impl FnOnce(Result<Option<WatchSettings>, String>) + Send + 'static,
) {
    let session = format!(
        "hs{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let ping_str = json!({
        "type": "hs_ping",
        "session": session,
    })
    .to_string();

    {
        let mut guard = pending().lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_some() {
            // 直接丢弃旧会话回调（新操作会刷新 UI）
            tracing::warn!("取消上一个未完成的握手会话");
        }
        *guard = Some(PendingHandshake {
            device_addr: device_addr.clone(),
            ping_str,
            phase: HandshakePhase::Registering,
            register_waited_ms: 0,
            ping_waited_ms: 0,
            attempt: 0,
            progress: Box::new(progress),
            on_done: Some(Box::new(on_done)),
        });
    }

    // 重置会话状态
    touch_session(&device_addr, Some(false), None);

    // 10ms 后由定时器事件驱动第一步注册尝试
    // 探针：确认 set_timeout 的 future 何时 resolve
    tracing::info!("begin_wait: 注册 10ms 启动定时器...");
    let timer_id = timer::set_timeout(10, HS_REGISTER_RETRY_EVENT).await;
    tracing::info!("begin_wait: 定时器注册完成 id={}", timer_id);
}

enum HsAction {
    Nothing,
    StartPing,
    Complete(Result<Option<WatchSettings>, String>),
}

/// 注册阶段单步：尝试注册互联，失败则武装下一次重试
fn register_step() -> HsAction {
    let device_addr = {
        let guard = pending().lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_ref() {
            Some(ph) if ph.phase == HandshakePhase::Registering => ph.device_addr.clone(),
            _ => return HsAction::Nothing,
        }
    };

    tracing::info!("握手: 尝试注册互联...");
    let registered = wit_bindgen::block_on(async {
        crate::astrobox::psys_host::register::register_interconnect_recv(
            &device_addr,
            WATCH_APP_PKG_NAME,
        )
        .await
        .is_ok()
    });
    tracing::info!("握手: 注册结果 = {}", registered);

    {
        let mut guard = pending().lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(ph) if ph.phase == HandshakePhase::Registering => {
                ph.attempt += 1;
                if registered {
                    tracing::info!(
                        "互联注册成功 (第{}次尝试，总等待 {}ms)",
                        ph.attempt,
                        ph.register_waited_ms
                    );
                    ph.phase = HandshakePhase::Pinging;
                    return HsAction::StartPing;
                }
                if ph.register_waited_ms >= REGISTER_RETRY_TIMEOUT_MS {
                    return HsAction::Complete(Err(
                        "无法连接快应用，请确保手表快应用已打开后重试。".to_string(),
                    ));
                }
                ph.register_waited_ms += REGISTER_RETRY_INTERVAL_MS;
                (ph.progress)(format!(
                    "等待快应用就绪 ({}s)...",
                    ph.register_waited_ms / 1000 + 1
                ));
            }
            _ => return HsAction::Nothing,
        }
    }

    // 武装下一次重试
    wit_bindgen::block_on(async {
        timer::set_timeout(REGISTER_RETRY_INTERVAL_MS, HS_REGISTER_RETRY_EVENT).await;
    });
    HsAction::Nothing
}

/// ping 阶段单步：发一个 ping 并武装下一次轮询。
/// 快应用 JS 侧回调初始化晚于系统通道就绪，启动窗口内的 ping 会被丢弃，
/// 短间隔持续 ping 让应用一起来就能应答
fn ping_step() -> HsAction {
    let (device_addr, ping_str) = {
        let mut guard = pending().lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(ph) if ph.phase == HandshakePhase::Pinging => {
                if ph.ping_waited_ms >= HANDSHAKE_POLL_TIMEOUT_MS {
                    tracing::warn!("握手轮询超时，按旧版兼容模式继续");
                    return HsAction::Complete(Ok(None));
                }
                ph.attempt += 1;
                (ph.progress)(format!(
                    "正在连接快应用 ({}s)...",
                    ph.ping_waited_ms / 1000 + 1
                ));
                ph.ping_waited_ms += HANDSHAKE_POLL_INTERVAL_MS;
                (ph.device_addr.clone(), ph.ping_str.clone())
            }
            _ => return HsAction::Nothing,
        }
    };

    wit_bindgen::block_on(async {
        if let Err(e) =
            interconnect::send_qaic_message(&device_addr, WATCH_APP_PKG_NAME, &ping_str).await
        {
            tracing::warn!("握手 ping 发送失败: {:?}", e);
        } else {
            tracing::info!("握手 ping 已发送，等待 pong...");
        }
        timer::set_timeout(HANDSHAKE_POLL_INTERVAL_MS, HS_PING_EVENT).await;
    });
    HsAction::Nothing
}

/// 定时器事件入口（lib.rs 分发）
pub fn on_timer(payload: &str) {
    let mut action = if payload == HS_REGISTER_RETRY_EVENT {
        register_step()
    } else if payload == HS_PING_EVENT {
        ping_step()
    } else {
        HsAction::Nothing
    };
    // 链式推进：注册成功立即进入 ping 阶段；complete 在任何 block_on 之外调用
    loop {
        match action {
            HsAction::StartPing => {
                action = ping_step();
            }
            HsAction::Complete(result) => {
                complete(result);
                break;
            }
            HsAction::Nothing => break,
        }
    }
}
