//! Host-owned local comic HTTP service and device-verified address binding.
use crate::astrobox::psys_host_v4::{http_server as host, interconnect, os};
use crate::http_probe::{self, ProbeResponse, ServiceIdentity};
use crate::ui::state::WATCH_APP_PKG_NAME;
use serde_json::{json, Value};
use std::net::Ipv4Addr;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const START_EVENT: &str = "http_probe_start";
pub const STOP_EVENT: &str = "http_probe_stop";
pub const IP_INPUT_EVENT: &str = "http_probe_ip_input";
pub const IP_SAVE_EVENT: &str = "http_fallback_ip_save";
pub const BIND_EVENT: &str = "http_probe_bind_device";
pub const SETTINGS_EVENT: &str = "http_connection_settings";
const LOOPBACK_IP: &str = "127.0.0.1";

pub async fn expire_binding(session: &str) -> bool {
    if state().lock().unwrap_or_else(|p| p.into_inner()).bind_session.as_deref() != Some(session) {
        return false;
    }
    fail_binding("设备连接超时，请检查 AstroBox 连接或设置备用 IPv4 后重试", true).await
}

#[derive(Default)]
struct ServerState {
    info: Option<host::ServerInfo>,
    identity: Option<ServiceIdentity>,
    busy: bool,
    error: Option<String>,
    requests: u64,
    advertised_ip: String,
    fallback_ip: String,
    fallback_input: String,
    remaining_ip: Option<String>,
    device_addr: Option<String>,
    bind_session: Option<String>,
    bind_timer_id: Option<u64>,
    bind_status: Option<String>,
    bound: bool,
    settings_expanded: bool,
}

static STATE: OnceLock<Mutex<ServerState>> = OnceLock::new();

fn state() -> &'static Mutex<ServerState> {
    STATE.get_or_init(|| {
        // 旧版保存的局域网地址迁移为备用地址，不覆盖回环优先策略。
        let fallback_ip = normalize_fallback_ip(&std::fs::read_to_string("http-address.txt").unwrap_or_default())
            .unwrap_or_default();
        Mutex::new(ServerState {
            advertised_ip: LOOPBACK_IP.into(),
            fallback_input: fallback_ip.clone(),
            fallback_ip,
            ..ServerState::default()
        })
    })
}

#[derive(Clone)]
pub struct Status {
    pub endpoint: Option<String>,
    pub port: Option<u16>,
    pub busy: bool,
    pub error: Option<String>,
    pub fallback_input: String,
    pub bind_status: Option<String>,
    pub bound: bool,
    pub settings_expanded: bool,
}

pub fn status() -> Status {
    let state = state().lock().unwrap_or_else(|p| p.into_inner());
    Status {
        endpoint: state.info.as_ref().map(|info| format!("http://{}:{}", state.advertised_ip, info.port)),
        port: state.info.as_ref().map(|info| info.port),
        busy: state.busy || state.bind_session.is_some(),
        error: state.error.clone(),
        fallback_input: state.fallback_input.clone(),
        bind_status: state.bind_status.clone(),
        bound: state.bound,
        settings_expanded: state.settings_expanded,
    }
}

/// 数据回传仅需服务候选地址，不改变漫画导入的绑定 endpoint 或版本门槛。
pub fn data_sync_config() -> Option<Value> {
    let state = state().lock().unwrap_or_else(|p| p.into_inner());
    let info = state.info.as_ref()?;
    let identity = state.identity.as_ref()?;
    let mut endpoints = vec![format!("http://{}:{}", LOOPBACK_IP, info.port)];
    if !state.fallback_ip.is_empty() { endpoints.push(format!("http://{}:{}", state.fallback_ip, info.port)); }
    Some(json!({"protocol": 1, "endpoints": endpoints, "instanceId": identity.instance_id,
        "chunkBytes": crate::http_data_sync::CHUNK_BYTES, "maxCoverBytes": crate::http_data_sync::MAX_COVER_BYTES}))
}

fn render() {
    crate::ui::build::rerender_main_ui();
}

fn normalize_fallback_ip(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() { return Ok(String::new()); }
    let ip: Ipv4Addr = value.parse().map_err(|_| "请输入有效的 IPv4 地址（无需端口）".to_string())?;
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err("该地址不能作为设备访问地址".into());
    }
    Ok(if ip.is_loopback() { String::new() } else { ip.to_string() })
}

pub fn edit_fallback_ip(ip: String) {
    // 输入中保留草稿，失焦才校验/保存，避免重渲染打断逐字输入 IPv4。
    state().lock().unwrap_or_else(|p| p.into_inner()).fallback_input = ip;
}

pub fn update_fallback_ip(ip: String) {
    let result = normalize_fallback_ip(&ip);
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    state.fallback_input = ip;
    match result {
        Ok(ip) => match std::fs::write("http-address.txt", &ip) {
            Ok(()) => {
                state.fallback_input = ip.clone();
                state.fallback_ip = ip;
            }
            Err(error) => state.bind_status = Some(format!("保存备用地址失败：{}", error)),
        },
        Err(error) => state.bind_status = Some(error),
    }
    // 编辑备用配置不改变已绑定会话及正在下载的漫画 URL。
    drop(state);
    render();
}

pub fn toggle_settings() {
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    state.settings_expanded = !state.settings_expanded;
    drop(state);
    render();
}

pub async fn start() {
    {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        if state.busy || state.info.is_some() { return; }
        state.busy = true;
        state.error = None;
    }
    render();
    if let Err(reason) = http_probe::samples() {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.busy = false;
        state.error = Some(format!("生成探针图片失败：{}", reason));
        drop(state);
        render();
        return;
    }
    let host_id = match os::device_id().await {
        Ok(id) => Some(id),
        Err(reason) => {
            tracing::warn!("读取宿主身份失败: {}", reason);
            None
        }
    };
    let result = host::start(host::ServerOptions { port: 0, bind_all_interfaces: true }).await;
    {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.busy = false;
        match result {
            Ok(info) => {
                let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
                state.identity = Some(ServiceIdentity {
                    host_id,
                    instance_id: format!("probe-{timestamp}-{}", info.id),
                    server_id: info.id,
                    port: info.port,
                });
                tracing::info!("HTTP 探针服务监听 {}:{}；本机测试 {}", info.host, info.port, info.url);
                state.info = Some(info);
                state.advertised_ip = LOOPBACK_IP.into();
                state.bound = false;
                state.bind_status = None;
                state.requests = 0;
            }
            Err(reason) => {
                tracing::error!("启动 HTTP 探针服务失败: {}", reason);
                state.error = Some(reason);
            }
        }
    }
    render();
}

pub async fn stop() {
    let id = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        if state.busy || state.bind_session.is_some() { return; }
        let Some(info) = &state.info else { return; };
        let id = info.id;
        state.busy = true;
        state.error = None;
        id
    };
    render();
    let result = host::stop(id).await;
    {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.busy = false;
        match result {
            Ok(()) => {
                state.info = None;
                state.identity = None;
                state.bound = false;
                tracing::info!("HTTP 探针服务 {} 已停止", id);
            }
            Err(reason) => {
                tracing::error!("停止 HTTP 探针服务失败: {}", reason);
                state.error = Some(reason);
            }
        }
    }
    render();
}

fn begin_binding() -> Result<(), String> {
    {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        if state.busy || state.bind_session.is_some() { return Err("正在连接设备，请稍候".into()); }
        if state.info.is_none() {
            let reason = state.error.clone().unwrap_or_else(|| "本地 HTTP 服务尚未启动".into());
            state.bind_status = Some(reason.clone());
            drop(state);
            render();
            return Err(reason);
        }
        state.advertised_ip = LOOPBACK_IP.into();
        state.remaining_ip = if state.fallback_ip.is_empty() { None } else { Some(state.fallback_ip.clone()) };
        state.device_addr = None;
        state.bound = false;
        state.busy = true;
        state.bind_status = Some("正在自动连接快应用...".into());
    }
    render();
    Ok(())
}

/// 上传入口已经检查版本、完成启动等待与握手，直接复用同一设备连接。
pub async fn bind_connected_device(device_addr: String) -> Result<(), String> {
    begin_binding()?;
    state().lock().unwrap_or_else(|p| p.into_inner()).device_addr = Some(device_addr);
    send_binding().await
}

pub async fn bind_device() -> Result<(), String> {
    begin_binding()?;

    let progress = |msg: String| {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.bind_status = Some(msg);
        drop(state);
        render();
    };

    let device_addr = match crate::ui::handshake::prepare_launch(
        crate::ui::handshake::MIN_UPLOAD_VERSION, &progress,
    ).await {
        Ok(addr) => addr,
        Err(err) => {
            let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
            state.busy = false;
            state.remaining_ip = None;
            state.bind_status = Some(format!("启动快应用失败：{}", err));
            drop(state);
            render();
            return Err(err);
        }
    };

    // 手动测试也走统一启动等待；HTTP 绑定超时从真正发出 gateway_bind 开始计时。
    crate::ui::handshake::begin_wait(device_addr.clone(), progress, move |result| async move {
        match result {
            Ok(_) => {
                state().lock().unwrap_or_else(|p| p.into_inner()).device_addr = Some(device_addr);
                let _ = send_binding().await;
            }
            Err(error) => clear_failed_binding(&error),
        }
    });
    Ok(())
}

async fn send_binding() -> Result<(), String> {
    let session = format!(
        "bind-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
    );

    let (device_addr, ip, port, instance_id) = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        let port = match &state.info {
            Some(info) => info.port,
            None => {
                drop(state);
                clear_failed_binding("本地 HTTP 服务已停止");
                return Err("本地 HTTP 服务已停止".into());
            }
        };
        let device_addr = match state.device_addr.clone() {
            Some(addr) => addr,
            None => {
                drop(state);
                clear_failed_binding("设备连接已失效");
                return Err("设备连接已失效".into());
            }
        };
        let instance_id = state.identity.as_ref().map(|id| id.instance_id.clone()).unwrap_or_default();
        state.bind_session = Some(session.clone());
        state.bound = false;
        state.busy = true;
        state.bind_status = Some(if state.advertised_ip == LOOPBACK_IP {
            "正在验证本机回环连接...".into()
        } else {
            "正在验证备用 IPv4 连接...".into()
        });
        (device_addr, state.advertised_ip.clone(), port, instance_id)
    };

    let bind_msg = json!({
        "type": "gateway_bind",
        "session": session,
        "endpoint": format!("http://{}:{}", ip, port),
        "service": "bandcomic-local-http",
        "protocolVersion": 1,
        "instanceId": instance_id,
        "port": port
    }).to_string();
    let timer_id = crate::astrobox::psys_host_v4::timer::set_timeout(30_000, &format!("http_bind_timeout:{}", session));
    state().lock().unwrap_or_else(|p| p.into_inner()).bind_timer_id = Some(timer_id);

    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), bind_msg).await {
        Ok(()) => {
            let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
            state.busy = false;
            drop(state);
            render();
            Ok(())
        }
        Err(err) => {
            // 互联发送失败与 HTTP 地址无关，直接结束并释放待上传状态。
            clear_failed_binding(&format!("发送绑定请求失败：{}", err));
            Err(err)
        }
    }
}

fn clear_failed_binding(reason: &str) {
    let timer_id = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.bind_session = None;
        state.remaining_ip = None;
        state.device_addr = None;
        state.busy = false;
        state.bound = false;
        state.bind_status = Some(reason.into());
        state.bind_timer_id.take()
    };
    if let Some(id) = timer_id { crate::astrobox::psys_host_v4::timer::clear_timer(id); }
    render();
}

// false：仍在尝试下一个地址；true：本次连接结束，可消费待上传任务。
async fn fail_binding(reason: &str, allow_fallback: bool) -> bool {
    let (timer_id, retry) = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        let timer_id = state.bind_timer_id.take();
        state.bind_session = None;
        let next = if allow_fallback { state.remaining_ip.take() } else { None };
        if let Some(ip) = next {
            state.advertised_ip = ip;
            (timer_id, true)
        } else {
            (timer_id, false)
        }
    };
    if let Some(id) = timer_id { crate::astrobox::psys_host_v4::timer::clear_timer(id); }
    if retry {
        tracing::info!("回环连接未通过（{}），尝试已保存的备用 IPv4", reason);
        if send_binding().await.is_ok() { return false; }
    } else {
        clear_failed_binding(reason);
    }
    true
}

pub async fn handle_bind_result(parsed: &Value) -> bool {
    let session = parsed.get("session").and_then(Value::as_str).unwrap_or("");
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    if state.bind_session.as_deref() != Some(session) {
        tracing::warn!("忽略已过期的 gateway_bind_result: session={}", session);
        return false;
    }

    let success = parsed.get("success").and_then(Value::as_bool).unwrap_or(false);
    let native_fetch = parsed.get("nativeFetch").and_then(Value::as_bool).unwrap_or(true);
    let error = parsed.get("error").and_then(Value::as_str).unwrap_or("");

    if success {
        state.bound = true;
        state.bind_session = None;
        state.remaining_ip = None;
        state.device_addr = None;
        state.busy = false;
        let timer_id = state.bind_timer_id.take();
        let probe_len = parsed.get("probeLength").and_then(Value::as_u64).unwrap_or(0);
        state.bind_status = Some(format!(
            "原生 HTTP 已连接（图片探针 {} 字节）",
            probe_len
        ));
        tracing::info!("手环端绑定确认成功: session={}, probe_len={}", session, probe_len);
        drop(state);
        if let Some(id) = timer_id { crate::astrobox::psys_host_v4::timer::clear_timer(id); }
        render();
        true
    } else {
        let hint = if !native_fetch {
            "（该设备不支持原生 fetch）"
        } else {
            ""
        };
        tracing::warn!("手环端绑定失败: session={}, error={}", session, error);
        drop(state);
        fail_binding(&format!("连接失败{}：{}", hint, error), native_fetch).await
    }
}

fn to_host(result: ProbeResponse) -> host::Response {
    host::Response {
        status: result.status,
        headers: result.headers.into_iter().map(|(name, value)| host::Header { name, value }).collect(),
        body: result.body,
    }
}

pub fn handle(server_id: u32, request: host::Request) -> host::Response {
    let (identity, advertised_ip, port) = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        // 发送控制消息期间仍允许设备探针进入，避免把连接中的服务误报为 503。
        let identity = state.identity.as_ref().filter(|identity| identity.server_id == server_id).cloned();
        if identity.is_some() { state.requests = state.requests.saturating_add(1); }
        let port = state.info.as_ref().map(|i| i.port).unwrap_or(0);
        (identity, state.advertised_ip.clone(), port)
    };
    let Some(identity) = identity else {
        return to_host(http_probe::error(503, "HTTP probe server is not active"));
    };

    let host_header = request.headers.iter()
        .find(|h| h.name.eq_ignore_ascii_case("host"))
        .map(|h| h.value.as_str());

    let base_url = if !advertised_ip.is_empty() && port > 0 {
        format!("http://{}:{}", advertised_ip, port)
    } else if let Some(host) = host_header {
        format!("http://{}", host)
    } else {
        format!("http://127.0.0.1:{}", port)
    };

    let response = if let Some(local_resp) = crate::local_source::route_local(
        &request.method,
        &request.path,
        &request.query,
        &base_url,
    ) {
        local_resp
    } else if let Some(sync_resp) = crate::http_data_sync::route(&request.method, &request.path, &request.query, &request.body) {
        sync_resp
    } else if let Some(task_resp) = crate::jobs::route_task(&request.method, &request.path, &request.body) {
        task_resp
    } else if request.path.starts_with("/control/") {
        http_probe::route(&request.method, &request.path, &identity)
    } else {
        http_probe::error(404, "Route not found")
    };

    tracing::info!("HTTP handler: server={} {} {} -> {} ({} bytes)", server_id,
        request.method, request.path, response.status, response.body.len());
    to_host(response)
}
