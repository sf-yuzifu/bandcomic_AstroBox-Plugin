//! Host-owned HTTP listener. Only small, fixed HTTP-1 probes are exposed.
use crate::astrobox::psys_host_v4::{http_server as host, interconnect, os};
use crate::http_probe::{self, ProbeResponse, ServiceIdentity};
use crate::ui::state::WATCH_APP_PKG_NAME;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const START_EVENT: &str = "http_probe_start";
pub const STOP_EVENT: &str = "http_probe_stop";
pub const IP_INPUT_EVENT: &str = "http_probe_ip_input";
pub const BIND_EVENT: &str = "http_probe_bind_device";

#[derive(Default)]
struct ServerState {
    info: Option<host::ServerInfo>,
    identity: Option<ServiceIdentity>,
    busy: bool,
    error: Option<String>,
    requests: u64,
    advertised_ip: String,
    bind_session: Option<String>,
    bind_status: Option<String>,
    bound: bool,
}

static STATE: OnceLock<Mutex<ServerState>> = OnceLock::new();

fn state() -> &'static Mutex<ServerState> {
    STATE.get_or_init(|| Mutex::new(ServerState::default()))
}

#[derive(Clone)]
pub struct Status {
    pub url: Option<String>,
    pub port: Option<u16>,
    pub busy: bool,
    pub error: Option<String>,
    pub requests: u64,
    pub advertised_ip: String,
    pub bind_status: Option<String>,
    pub bound: bool,
}

pub fn status() -> Status {
    let state = state().lock().unwrap_or_else(|p| p.into_inner());
    Status {
        url: state.info.as_ref().map(|info| info.url.clone()),
        port: state.info.as_ref().map(|info| info.port),
        busy: state.busy,
        error: state.error.clone(),
        requests: state.requests,
        advertised_ip: state.advertised_ip.clone(),
        bind_status: state.bind_status.clone(),
        bound: state.bound,
    }
}

fn render() {
    crate::ui::build::rerender_main_ui();
}

pub fn update_advertised_ip(ip: String) {
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    state.advertised_ip = ip.trim().to_string();
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
        if state.busy { return; }
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

pub async fn bind_device() {
    let (ip, port, instance_id) = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        if state.busy { return; }
        let port = match &state.info {
            Some(info) => info.port,
            None => {
                state.bind_status = Some("请先启动 HTTP 服务！".to_string());
                drop(state);
                render();
                return;
            }
        };
        let ip = state.advertised_ip.trim().to_string();
        if ip.is_empty() {
            state.bind_status = Some("请先输入 Windows 实际 IPv4 地址 (如 192.168.1.100)！".to_string());
            drop(state);
            render();
            return;
        }
        let instance_id = state.identity.as_ref().map(|id| id.instance_id.clone()).unwrap_or_default();
        state.busy = true;
        state.bind_status = Some("正在连接快应用并发送绑定请求...".to_string());
        (ip, port, instance_id)
    };
    render();

    let progress = |msg: String| {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.bind_status = Some(msg);
        drop(state);
        render();
    };

    let device_addr = match crate::ui::handshake::prepare_launch(0, &progress).await {
        Ok(addr) => addr,
        Err(err) => {
            let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
            state.busy = false;
            state.bind_status = Some(format!("启动快应用失败：{}", err));
            drop(state);
            render();
            return;
        }
    };

    // 必须向 AstroBox 注册接收该设备的互联消息，否则宿主不会将快应用的应答派发给插件
    tracing::info!("向宿主注册互联接收: addr={}, pkg={}", device_addr, WATCH_APP_PKG_NAME);
    let reg_result = crate::astrobox::psys_host_v4::register::register_interconnect_recv(
        device_addr.clone(),
        WATCH_APP_PKG_NAME.into(),
    )
    .await;
    match reg_result {
        Ok(()) => tracing::info!("向宿主注册互联接收成功"),
        Err(err) => tracing::warn!("向宿主注册互联接收失败: {}", err),
    }

    let session = format!(
        "bind-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
    );

    {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.bind_session = Some(session.clone());
    }

    let bind_msg = json!({
        "type": "gateway_bind",
        "session": session,
        "endpoint": format!("http://{}:{}", ip, port),
        "service": "bandcomic-local-http",
        "protocolVersion": 1,
        "instanceId": instance_id,
        "port": port
    }).to_string();

    match interconnect::send_qaic_message(device_addr, WATCH_APP_PKG_NAME.into(), bind_msg).await {
        Ok(()) => {
            let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
            state.busy = false;
            state.bind_status = Some("绑定消息已发送，等待手环核对健康检查与图片探针...".to_string());
        }
        Err(err) => {
            let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
            state.busy = false;
            state.bind_status = Some(format!("发送绑定请求失败：{}", err));
        }
    }
    render();
}

pub fn handle_bind_result(parsed: &Value) {
    let session = parsed.get("session").and_then(Value::as_str).unwrap_or("");
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    if state.bind_session.as_deref() != Some(session) {
        tracing::warn!("忽略已过期的 gateway_bind_result: session={}", session);
        return;
    }

    let success = parsed.get("success").and_then(Value::as_bool).unwrap_or(false);
    let native_fetch = parsed.get("nativeFetch").and_then(Value::as_bool).unwrap_or(true);
    let error = parsed.get("error").and_then(Value::as_str).unwrap_or("");

    if success {
        state.bound = true;
        let probe_len = parsed.get("probeLength").and_then(Value::as_u64).unwrap_or(0);
        state.bind_status = Some(format!(
            "🎉 绑定成功！9 Pro 原生 fetch 探针通过 (下载验证 {} 字节)",
            probe_len
        ));
        tracing::info!("手环端绑定确认成功: session={}, probe_len={}", session, probe_len);
    } else {
        state.bound = false;
        let hint = if !native_fetch {
            "（该设备不支持原生 fetch）"
        } else {
            ""
        };
        state.bind_status = Some(format!("绑定失败{}：{}", hint, error));
        tracing::warn!("手环端绑定失败: session={}, error={}", session, error);
    }
    drop(state);
    render();
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
        if state.busy {
            (None, String::new(), 0)
        } else {
            let identity = state.identity.as_ref().filter(|identity| identity.server_id == server_id).cloned();
            if identity.is_some() { state.requests = state.requests.saturating_add(1); }
            let port = state.info.as_ref().map(|i| i.port).unwrap_or(0);
            (identity, state.advertised_ip.clone(), port)
        }
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
