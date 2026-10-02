//! Host-owned HTTP listener. Only small, fixed HTTP-1 probes are exposed.
use crate::astrobox::psys_host_v4::{http_server as host, os};
use crate::http_probe::{self, ProbeResponse, ServiceIdentity};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const START_EVENT: &str = "http_probe_start";
pub const STOP_EVENT: &str = "http_probe_stop";

#[derive(Default)]
struct ServerState {
    info: Option<host::ServerInfo>,
    identity: Option<ServiceIdentity>,
    busy: bool,
    error: Option<String>,
    requests: u64,
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
}

pub fn status() -> Status {
    let state = state().lock().unwrap_or_else(|p| p.into_inner());
    Status {
        url: state.info.as_ref().map(|info| info.url.clone()),
        port: state.info.as_ref().map(|info| info.port),
        busy: state.busy,
        error: state.error.clone(),
        requests: state.requests,
    }
}

fn render() {
    crate::ui::build::rerender_main_ui();
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

fn to_host(result: ProbeResponse) -> host::Response {
    host::Response {
        status: result.status,
        headers: result.headers.into_iter().map(|(name, value)| host::Header { name, value }).collect(),
        body: result.body,
    }
}

pub fn handle(server_id: u32, request: host::Request) -> host::Response {
    let identity = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        if state.busy { None } else {
            let identity = state.identity.as_ref().filter(|identity| identity.server_id == server_id).cloned();
            if identity.is_some() { state.requests = state.requests.saturating_add(1); }
            identity
        }
    };
    let Some(identity) = identity else {
        return to_host(http_probe::error(503, "HTTP probe server is not active"));
    };
    let response = http_probe::route(&request.method, &request.path, &identity);
    tracing::info!("HTTP handler: server={} {} {} -> {} ({} bytes)", server_id,
        request.method, request.path, response.status, response.body.len());
    to_host(response)
}
