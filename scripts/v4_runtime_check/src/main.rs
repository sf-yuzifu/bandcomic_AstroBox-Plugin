//! ABI/WASI smoke check with deliberately small test doubles for AstroBox APIs.
//! This exercises the built plugin, but does not replace installed AstroBox tests.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use wasmtime::{Config, Engine, Store};
use wasmtime::component::{Component, Linker, ResourceTable, Val};
use wasmtime::component::{types::ComponentItem, ResourceType};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, FsPerms};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "psys-world-v4-http",
        imports: { default: trappable },
        exports: { default: async | store },
    });
}

struct Context {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    listener: Option<TcpListener>,
    server_id: u32,
    starts: u32,
    stops: u32,
    sent_interconnect: Arc<Mutex<Vec<String>>>,
    picked_images: usize,
    app_version: u32,
    launches: usize,
    launched_at: Option<std::time::Instant>,
    registrations: usize,
    timers: Vec<(u64, String)>,
}

// Five distinct 2x2 BMPs: a cover followed by four pages. No image dependency is
// needed in the host; decoding/encoding and disk caching run in the real WASM.
fn picked_image(index: usize) -> Vec<u8> {
    let colors = [[0, 0, 255], [0, 255, 0], [255, 0, 0], [0, 255, 255], [255, 0, 255]];
    let color = colors[index % colors.len()]; // BGR
    let mut bmp = vec![0u8; 70];
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
    bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    bmp[34..38].copy_from_slice(&16u32.to_le_bytes());
    for row in 0..2 {
        for col in 0..2 {
            let offset = 54 + row * 8 + col * 3;
            bmp[offset..offset + 3].copy_from_slice(&color);
        }
    }
    bmp
}

impl WasiView for Context {
    fn ctx(&mut self) -> WasiCtxView<'_> { WasiCtxView { ctx: &mut self.wasi, table: &mut self.table } }
}
impl WasiHttpView for Context {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView { ctx: &mut self.http, table: &mut self.table, hooks: wasmtime_wasi_http::default_hooks() }
    }
}

fn fixture() -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let task = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut buffer = [0u8; 1024];
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
            }
            assert!(request.starts_with(b"GET /config HTTP/1.1\r\n"));
            let body = br#"{"Probe":{"name":"Probe","apiUrl":"http://127.0.0.1","detailPath":"/album/<id>","photoPath":"/photo/<id>/<chapter>","searchPath":"/search/<text>/<page>"}}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
            calls.fetch_add(1, Ordering::SeqCst);
        }
    });
    (url, count, task)
}

fn add_test_hosts(linker: &mut Linker<Context>) -> wasmtime::Result<()> {
    linker.instance("astrobox:psys-host-v4/os")?.func_wrap_concurrent("device-id", |_, (): ()| Box::pin(async {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok((Ok::<_, String>("runtime-check-host".to_string()),))
    }))?;
    linker.instance("astrobox:psys-host-v4/register")?.func_new_concurrent("register-card", |_, _, _, results| Box::pin(async move {
        results[0] = Val::Result(Ok(None));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/register")?.func_new_concurrent("register-interconnect-recv", |accessor, _, _, results| Box::pin(async move {
        accessor.with(|mut access| {
            let ctx = access.get();
            assert!(ctx.launched_at.expect("registration must follow launch").elapsed() >= std::time::Duration::from_secs(3),
                "interconnect registration happened before the 3-second startup delay");
            ctx.registrations += 1;
        });
        results[0] = Val::Result(Ok(None));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/dialog")?.func_new_concurrent("pick-file", |accessor, _, _, results| Box::pin(async move {
        let index = accessor.with(|mut access| {
            let ctx = access.get();
            let index = ctx.picked_images;
            ctx.picked_images += 1;
            index
        });
        let pick_res = Val::Record(vec![
            ("name".into(), Val::String(format!("test_image_{index}.bmp"))),
            ("data".into(), Val::List(picked_image(index).into_iter().map(Val::U8).collect())),
        ]);
        results[0] = Val::Result(Ok(Some(Box::new(pick_res))));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/device")?.func_new_concurrent("get-connected-device-list", |_, _, _, results| Box::pin(async move {
        let dev = Val::Record(vec![
            ("name".into(), Val::String("Xiaomi Smart Band 9 Pro".into())),
            ("addr".into(), Val::String("11:22:33:44:55:66".into())),
        ]);
        results[0] = Val::List(vec![dev]);
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/thirdpartyapp")?.func_new_concurrent("get-thirdparty-app-list", |accessor, _, _, results| Box::pin(async move {
        let version = accessor.with(|mut access| access.get().app_version);
        let app = Val::Record(vec![
            ("package-name".into(), Val::String("moe.yzf.comic".into())),
            ("fingerprint".into(), Val::List(vec![Val::U32(1)])),
            ("version-code".into(), Val::U32(version)),
            ("can-remove".into(), Val::Bool(true)),
            ("app-name".into(), Val::String("腕上漫画".into())),
        ]);
        results[0] = Val::Result(Ok(Some(Box::new(Val::List(vec![app])))));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/thirdpartyapp")?.func_new_concurrent("launch-qa", |accessor, _, _, results| Box::pin(async move {
        accessor.with(|mut access| {
            let ctx = access.get();
            ctx.launches += 1;
            ctx.launched_at = Some(std::time::Instant::now());
        });
        results[0] = Val::Result(Ok(None));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/interconnect")?.func_wrap_concurrent("send-qaic-message", |accessor, (_addr, _pkg, data): (String, String, String)| {
        accessor.with(|mut access| {
            let ctx = access.get();
            assert!(ctx.launched_at.expect("send must follow launch").elapsed() >= std::time::Duration::from_secs(3),
                "interconnect message sent before the 3-second startup delay");
            ctx.sent_interconnect.lock().unwrap().push(data);
        });
        Box::pin(async { Ok((Ok::<(), String>(()),)) })
    })?;

    let mut timers = linker.instance("astrobox:psys-host-v4/timer")?;
    timers.func_wrap("set-timeout", |mut store, (delay, payload): (u64, String)| {
        let ctx = store.data_mut();
        ctx.timers.push((delay, payload));
        Ok((ctx.timers.len() as u64,))
    })?;
    timers.func_wrap("clear-timer", |_, (_id,): (u64,)| Ok(()))?;

    let mut servers = linker.instance("astrobox:psys-host-v4/http-server")?;
    servers.func_new_concurrent("start", |accessor, _, params, results| Box::pin(async move {
        if let Val::Record(options) = &params[0] {
            assert!(options.contains(&("port".into(), Val::U16(0))));
            assert!(options.contains(&("bind-all-interfaces".into(), Val::Bool(true))));
        } else { panic!("expected server-options"); }
        let (id, port) = accessor.with(|mut access| {
            let ctx = access.get();
            assert!(ctx.listener.is_none(), "duplicate listener");
            let listener = TcpListener::bind("0.0.0.0:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            ctx.listener = Some(listener);
            ctx.server_id += 1;
            ctx.starts += 1;
            (ctx.server_id, port)
        });
        results[0] = Val::Result(Ok(Some(Box::new(Val::Record(vec![
            ("id".into(), Val::U32(id)),
            ("host".into(), Val::String("0.0.0.0".into())),
            ("port".into(), Val::U16(port)),
            ("url".into(), Val::String(format!("http://127.0.0.1:{port}"))),
        ])))));
        Ok(())
    }))?;
    servers.func_wrap_concurrent("stop", |accessor, (id,): (u32,)| Box::pin(async move {
        accessor.with(|mut access| {
            let ctx = access.get();
            assert_eq!(id, ctx.server_id);
            assert!(ctx.listener.take().is_some());
            ctx.stops += 1;
        });
        Ok((Ok::<(), String>(()),))
    }))?;
    Ok(())
}

fn stub_unused_astrobox_imports(linker: &mut Linker<Context>, engine: &Engine, component: &Component) -> wasmtime::Result<()> {
    for (name, import) in component.component_type().imports(engine) {
        if !name.starts_with("astrobox:") { continue; }
        let ComponentItem::ComponentInstance(instance) = import.ty else { continue; };
        let mut interface = linker.instance(name)?;
        for (item_name, export) in instance.exports(engine) {
            let message = format!("unimplemented smoke-test host: {name}#{item_name}");
            match export.ty {
                ComponentItem::ComponentFunc(function) if function.async_() => {
                    interface.func_new_concurrent(item_name, move |_, _, _, _| {
                        let message = message.clone();
                        Box::pin(async move { Err(wasmtime::Error::msg(message)) })
                    })?;
                }
                ComponentItem::ComponentFunc(_) => {
                    interface.func_new(item_name, move |_, _, _, _| Err(wasmtime::Error::msg(message.clone())))?;
                }
                ComponentItem::Resource(_) => {
                    interface.resource(item_name, ResourceType::host::<()>(), |_, _| Ok(()))?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// Exercise the actual guest deadline, including early delivery and stale timers.
async fn complete_startup(
    world: &bindings::PsysWorldV4Http,
    accessor: &wasmtime::component::Accessor<Context>,
) -> wasmtime::Result<()> {
    use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
    let events = world.astrobox_psys_plugin_v4_event();
    let (delay, payload, registrations, sent, launched_at) = accessor.with(|mut access| {
        let ctx = access.get();
        let (delay, payload) = ctx.timers.iter().rev().find(|(_, p)| p.starts_with("hs_register_retry:")).unwrap().clone();
        (delay, payload, ctx.registrations, ctx.sent_interconnect.lock().unwrap().len(), ctx.launched_at.unwrap())
    });
    assert!(delay <= 3001 && delay > 0);
    for early in [payload.clone(), "hs_register_retry:old-session".into(), "hs_ping_poll:old-session".into()] {
        events.call_on_event(accessor, EventType::Timer, serde_json::json!({"payload": early}).to_string()).await?;
    }
    accessor.with(|mut access| {
        let ctx = access.get();
        assert_eq!(ctx.registrations, registrations, "early/old timers must not register");
        assert_eq!(ctx.sent_interconnect.lock().unwrap().len(), sent, "startup must stay silent");
    });
    tokio::time::sleep(std::time::Duration::from_millis(3050).saturating_sub(launched_at.elapsed())).await;
    events.call_on_event(accessor, EventType::Timer, serde_json::json!({"payload": payload}).to_string()).await?;
    let ping: serde_json::Value = accessor.with(|mut access| {
        let ctx = access.get();
        assert_eq!(ctx.registrations, registrations + 1);
        serde_json::from_str(ctx.sent_interconnect.lock().unwrap().last().unwrap()).unwrap()
    });
    assert_eq!(ping["type"], "hs_ping");
    events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
        "type": "hs_pong", "session": ping["session"],
        "settings": { "imageSize": 480, "imageQuality": 50, "imageUsePng": false, "imagePreTranscode": false },
        "caps": { "httpImport": true }
    }).to_string()).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> wasmtime::Result<()> {
    let wasm = std::env::args().nth(1).expect("usage: v4-runtime-check <plugin.wasm>");
    let mut config = Config::new();
    config.wasm_component_model(true).wasm_component_model_async(true).wasm_memory64(false);
    let engine = Engine::new(&config)?;
    let component = Component::from_file(&engine, wasm)?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    stub_unused_astrobox_imports(&mut linker, &engine, &component)?;
    linker.allow_shadowing(true);
    add_test_hosts(&mut linker)?;
    let directory = tempfile::tempdir_in(std::env::temp_dir().join("opencode")).unwrap();
    let mut wasi = WasiCtxBuilder::new();
    wasi.inherit_stdout().inherit_stderr().preopened_dir(directory.path(), ".", FsPerms::ReadWrite).unwrap();
    let sent_interconnect = Arc::new(Mutex::new(Vec::new()));
    let mut store = Store::new(&engine, Context {
        wasi: wasi.build(),
        http: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        listener: None,
        server_id: 0,
        starts: 0,
        stops: 0,
        sent_interconnect: sent_interconnect.clone(),
        picked_images: 0,
        app_version: 382,
        launches: 0,
        launched_at: None,
        registrations: 0,
        timers: Vec::new(),
    });
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let world = bindings::PsysWorldV4Http::new(&mut store, &instance)?;
    let (url, requests, fixture_task) = fixture();
    store.run_concurrent(async |accessor| -> wasmtime::Result<()> {
        use bindings::astrobox::psys_host_v4::{http_server::Request, ui::Event};
        use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
        world.astrobox_psys_plugin_v4_lifecycle().call_on_load(accessor).await?;
        accessor.with(|mut access| {
            let ctx = access.get();
            assert_eq!(ctx.registrations, 0, "plugin load must not register against a closed app");
            assert!(ctx.sent_interconnect.lock().unwrap().is_empty());
        });
        let http = world.astrobox_psys_plugin_v4_http();
        let events = world.astrobox_psys_plugin_v4_event();
        let request = |path: &str| Request { method: "GET".into(), path: path.into(), query: "".into(), headers: vec![], body: vec![] };
        let health = http.call_handle(accessor, 1, request("/control/health")).await?;
        assert_eq!(health.status, 200);
        let identity: serde_json::Value = serde_json::from_slice(&health.body).unwrap();
        assert_eq!(identity["hostId"], "runtime-check-host");
        for sample in identity["samples"].as_array().unwrap() {
            let response = http.call_handle(accessor, 1, request(sample["path"].as_str().unwrap())).await?;
            assert_eq!(response.status, 200);
            assert_eq!(response.body.len() as u64, sample["length"].as_u64().unwrap());
            assert!(response.headers.iter().any(|h| h.name.eq_ignore_ascii_case("Content-Length") && h.value == response.body.len().to_string()));
        }

        // Test outgoing WASI p2 fetch for sources
        events.call_on_ui_event(accessor, "domain_input_blur".into(), Event::Blur,
            serde_json::json!({"value": url}).to_string()).await?;
        assert_eq!(requests.load(Ordering::SeqCst), 2, "outgoing p2 HTTP config fetches");

        // Both management tabs reject 317 before launch, registration or send.
        accessor.with(|mut access| access.get().app_version = 317);
        for event in ["sync_button", "fetch_app_data"] {
            events.call_on_ui_event(accessor, event.into(), Event::Click, "{}".into()).await?;
        }
        accessor.with(|mut access| {
            let ctx = access.get();
            assert_eq!(ctx.launches, 0);
            assert_eq!(ctx.registrations, 0);
            assert!(ctx.sent_interconnect.lock().unwrap().is_empty());
        });
        // 318 is accepted by sync/data, each with a fresh startup wait.
        accessor.with(|mut access| access.get().app_version = 318);
        events.call_on_ui_event(accessor, "sync_button".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let last: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(last["type"], "source_config");
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let last: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(last["type"], "request_data");

        // Upload-tab connection testing also requires 382.
        accessor.with(|mut access| access.get().app_version = 381);
        let sent_count = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;
        accessor.with(|mut access| assert_eq!(access.get().launches, 2));
        assert_eq!(sent_interconnect.lock().unwrap().len(), sent_count);
        accessor.with(|mut access| access.get().app_version = 382);

        // Fresh installs bind without entering an IP address.
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;

        let sent = sent_interconnect.lock().unwrap().clone();
        assert!(!sent.is_empty(), "gateway_bind should be sent via interconnect");
        let bind_msg: serde_json::Value = serde_json::from_str(sent.last().unwrap()).unwrap();
        assert_eq!(bind_msg["type"], "gateway_bind");
        assert_eq!(bind_msg["endpoint"], format!("http://127.0.0.1:{}", identity["port"]));
        assert_eq!(bind_msg["service"], "bandcomic-local-http");
        assert_eq!(bind_msg["instanceId"], identity["instanceId"]);

        // Simulate device replying with successful gateway_bind_result
        let bind_session = bind_msg["session"].as_str().unwrap();
        let reply = serde_json::json!({
            "type": "gateway_bind_result",
            "session": bind_session,
            "success": true,
            "nativeFetch": true,
            "instanceId": identity["instanceId"],
            "probeLength": 689
        });
        events.call_on_event(accessor, EventType::InterconnectMessage, reply.to_string()).await?;

        // Test HTTP-3 LocalUpload source routes: /config, /local/search, /local/album, /local/photo
        let req_query = |path: &str, query: &str| Request {
            method: "GET".into(),
            path: path.into(),
            query: query.into(),
            headers: vec![],
            body: vec![],
        };
        let cfg = http.call_handle(accessor, 1, req_query("/config", "")).await?;
        assert_eq!(cfg.status, 200);
        let cfg_val: serde_json::Value = serde_json::from_slice(&cfg.body).unwrap();
        assert_eq!(cfg_val["LocalUpload"]["type"], "local");
        assert_eq!(cfg_val["LocalUpload"]["apiUrl"], format!("http://127.0.0.1:{}", identity["port"]));

        // Editing a fallback must not change an already-bound source address.
        events.call_on_ui_event(accessor, "http_probe_ip_input".into(), Event::Change,
            serde_json::json!({"value": "192."}).to_string()).await?;
        events.call_on_ui_event(accessor, "http_fallback_ip_save".into(), Event::Blur,
            serde_json::json!({"value": "192.168.1.100"}).to_string()).await?;
        let cfg = http.call_handle(accessor, 1, req_query("/config", "")).await?;
        let cfg_val: serde_json::Value = serde_json::from_slice(&cfg.body).unwrap();
        assert_eq!(cfg_val["LocalUpload"]["apiUrl"], bind_msg["endpoint"]);

        // Device rejection advances once to the saved fallback with a new session.
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let first: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(first["endpoint"], bind_msg["endpoint"], "saved LAN IP must not override loopback priority");
        events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
            "type": "gateway_bind_result", "session": first["session"],
            "success": false, "nativeFetch": true, "error": "loopback unavailable"
        }).to_string()).await?;
        let fallback: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(fallback["endpoint"], format!("http://192.168.1.100:{}", identity["port"]));
        assert_ne!(fallback["session"], first["session"]);
        let count = sent_interconnect.lock().unwrap().len();
        events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
            "type": "gateway_bind_result", "session": first["session"], "success": true
        }).to_string()).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(), count, "late loopback result must be ignored");
        events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
            "type": "gateway_bind_result", "session": fallback["session"], "success": true, "probeLength": 689
        }).to_string()).await?;
        let cfg = http.call_handle(accessor, 1, req_query("/config", "")).await?;
        let cfg_val: serde_json::Value = serde_json::from_slice(&cfg.body).unwrap();
        assert_eq!(cfg_val["LocalUpload"]["apiUrl"], fallback["endpoint"]);

        // Timeout also advances; an old timeout cannot expire the fallback attempt.
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let first: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        let timeout = serde_json::json!({"payload": format!("http_bind_timeout:{}", first["session"].as_str().unwrap())}).to_string();
        events.call_on_event(accessor, EventType::Timer, timeout.clone()).await?;
        let fallback: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(fallback["endpoint"], format!("http://192.168.1.100:{}", identity["port"]));
        let count = sent_interconnect.lock().unwrap().len();
        events.call_on_event(accessor, EventType::Timer, timeout).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(), count);
        events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
            "type": "gateway_bind_result", "session": fallback["session"],
            "success": false, "nativeFetch": true, "error": "fallback unavailable"
        }).to_string()).await?;
        events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
            "type": "gateway_bind_result", "session": fallback["session"], "success": true
        }).to_string()).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(), count, "finished attempts must reject late results");

        // Missing native fetch is not an address error and must not trigger fallback.
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let first: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        let count = sent_interconnect.lock().unwrap().len();
        events.call_on_event(accessor, EventType::InterconnectMessage, serde_json::json!({
            "type": "gateway_bind_result", "session": first["session"],
            "success": false, "nativeFetch": false, "error": "no native fetch"
        }).to_string()).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(), count);

        // Search returns catalog
        let search = http.call_handle(accessor, 1, req_query("/local/search/all/1", "")).await?;
        assert_eq!(search.status, 200);
        let s_val: serde_json::Value = serde_json::from_slice(&search.body).unwrap();
        assert_eq!(s_val["page"], 1);
        let results = s_val["results"].as_array().unwrap();
        assert!(!results.is_empty());
        let item_id = results[0]["comic_id"].as_str().unwrap();

        // Album detail
        let album = http.call_handle(accessor, 1, req_query(&format!("/local/album/{}", item_id), "")).await?;
        assert_eq!(album.status, 200);
        let a_val: serde_json::Value = serde_json::from_slice(&album.body).unwrap();
        assert_eq!(a_val["item_id"], item_id);
        assert!(a_val["page_count"].as_u64().unwrap() > 0);

        // Photo list
        let photo = http.call_handle(accessor, 1, req_query(&format!("/local/photo/{}/chapter/1", item_id), "")).await?;
        assert_eq!(photo.status, 200);
        let p_val: serde_json::Value = serde_json::from_slice(&photo.body).unwrap();
        let images = p_val["images"].as_array().unwrap();
        assert!(!images.is_empty());

        // Photo page JPEG
        let page_jpg = http.call_handle(accessor, 1, req_query(&format!("/local/photo/{}/chapter/1/1.jpg", item_id), "width=80&quality=50")).await?;
        assert_eq!(page_jpg.status, 200);
        assert!(page_jpg.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-type") && h.value == "image/jpeg"));

        // Photo page LVGL binary
        let page_lvgl = http.call_handle(accessor, 1, req_query(&format!("/local/photo/{}/chapter/1/1.jpg", item_id), "width=32&ifLVGL=1")).await?;
        assert_eq!(page_lvgl.status, 200);
        assert!(page_lvgl.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-type") && h.value == "application/octet-stream"));
        assert_eq!(page_lvgl.body.len(), 4 + 256 * 4 + 32 * 32);

        // Test HTTP-6: pick file, process, and trigger HTTP task creation via upload_start
        for _ in 0..5 {
            events.call_on_ui_event(accessor, "upload_pick_files".into(), Event::Click, "{}".into()).await?;
            events.call_on_event(accessor, EventType::Timer, serde_json::json!({"payload": "pick_process"}).to_string()).await?;
        }
        let launches = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.app_version = 381;
            ctx.launches
        });
        let sent_count = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, "upload_start".into(), Event::Click, "{}".into()).await?;
        accessor.with(|mut access| assert_eq!(access.get().launches, launches));
        assert_eq!(sent_interconnect.lock().unwrap().len(), sent_count);
        accessor.with(|mut access| access.get().app_version = 382);
        events.call_on_ui_event(accessor, "upload_start".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        accessor.with(|mut access| assert_eq!(access.get().launches, launches + 1,
            "HTTP binding must reuse the upload handshake instead of launching twice"));

        // Upload now automatically revalidates the HTTP binding before dispatch.
        let bind_msg_str = sent_interconnect.lock().unwrap().last().cloned().unwrap();
        let bind_msg: serde_json::Value = serde_json::from_str(&bind_msg_str).unwrap();
        assert_eq!(bind_msg["type"], "gateway_bind");
        assert_eq!(bind_msg["endpoint"], format!("http://127.0.0.1:{}", identity["port"]));
        let reply = serde_json::json!({ "type": "gateway_bind_result", "session": bind_msg["session"],
            "success": true, "nativeFetch": true, "probeLength": 689 });
        events.call_on_event(accessor, EventType::InterconnectMessage, reply.to_string()).await?;

        // Now import_http_task has been dispatched!
        let task_msg_str = sent_interconnect.lock().unwrap().last().cloned().expect("import_http_task sent");
        let task_msg: serde_json::Value = serde_json::from_str(&task_msg_str).unwrap();
        assert_eq!(task_msg["type"], "import_http_task");
        assert_eq!(task_msg["endpoint"], bind_msg["endpoint"]);
        let task_id = task_msg["taskId"].as_str().unwrap();

        // 1. Fetch task details
        let task_resp = http.call_handle(accessor, 1, req_query(&format!("/control/tasks/{}", task_id), "")).await?;
        assert_eq!(task_resp.status, 200);
        let task_detail: serde_json::Value = serde_json::from_slice(&task_resp.body).unwrap();
        assert_eq!(task_detail["taskId"], task_id);
        let book_id = task_detail["comicId"].as_str().unwrap();
        assert!(book_id.starts_with("book_"));
        assert_eq!(task_detail["totalPages"], 4);
        assert_eq!(task_detail["coverUrl"], format!("http://127.0.0.1:{}/local/album/{book_id}/cover", identity["port"]));
        let cfg = http.call_handle(accessor, 1, req_query("/config", "")).await?;
        let cfg_val: serde_json::Value = serde_json::from_slice(&cfg.body).unwrap();
        assert_eq!(cfg_val["LocalUpload"]["apiUrl"], task_msg["endpoint"]);
        let album = http.call_handle(accessor, 1, req_query(&format!("/local/album/{book_id}"), "")).await?;
        let album: serde_json::Value = serde_json::from_slice(&album.body).unwrap();
        assert_eq!(album["cover"], task_detail["coverUrl"]);
        let photo = http.call_handle(accessor, 1, req_query(&format!("/local/photo/{book_id}/chapter/1"), "")).await?;
        let photo: serde_json::Value = serde_json::from_slice(&photo.body).unwrap();
        for image in photo["images"].as_array().unwrap() {
            assert!(image["url"].as_str().unwrap().starts_with(&format!("{}/", task_msg["endpoint"].as_str().unwrap())));
        }
        let cover_path = format!("/local/album/{book_id}/cover");
        let cover_before = http.call_handle(accessor, 1, req_query(&cover_path, "width=80&ifPNG=1")).await?;
        assert_eq!(cover_before.status, 200);
        let mut image_bodies = vec![cover_before.body.clone()];
        for page in 1..=4 {
            let image = http.call_handle(accessor, 1,
                req_query(&format!("/local/photo/{book_id}/chapter/1/{page}.jpg"), "width=80&ifPNG=1")).await?;
            assert_eq!(image.status, 200);
            assert!(!image_bodies.contains(&image.body), "cover/page {page} content must remain distinct");
            image_bodies.push(image.body);
        }
        let cover_after = http.call_handle(accessor, 1, req_query(&cover_path, "width=80&ifPNG=1")).await?;
        assert_eq!(cover_after.body, cover_before.body, "reading the last page must not replace the cover");

        // 2. Report progress
        let prog_req = Request {
            method: "POST".into(),
            path: format!("/control/tasks/{}/progress", task_id),
            query: "".into(),
            headers: vec![],
            body: serde_json::json!({ "page": 1, "total": 4 }).to_string().into_bytes(),
        };
        let prog_resp = http.call_handle(accessor, 1, prog_req).await?;
        assert_eq!(prog_resp.status, 200);

        // 3. Report final result
        let res_req = Request {
            method: "POST".into(),
            path: format!("/control/tasks/{}/result", task_id),
            query: "".into(),
            headers: vec![],
            body: serde_json::json!({ "success": true, "savedPages": 4, "totalPages": 4 }).to_string().into_bytes(),
        };
        let res_resp = http.call_handle(accessor, 1, res_req).await?;
        assert_eq!(res_resp.status, 200);

        // Test stop / start / port release
        events.call_on_ui_event(accessor, "http_probe_stop".into(), Event::Click, "{}".into()).await?;
        let port = identity["port"].as_u64().unwrap() as u16;
        drop(TcpListener::bind(("0.0.0.0", port)).expect("stop must release the listener"));
        assert_eq!(http.call_handle(accessor, 1, request("/control/health")).await?.status, 503);
        let (start, event) = tokio::join!(
            events.call_on_ui_event(accessor, "http_probe_start".into(), Event::Click, "{}".into()),
            events.call_on_event(accessor, EventType::PluginMessage, "concurrent-event".into()),
        );
        start?; event?;
        accessor.with(|mut access| { let ctx = access.get(); assert_eq!((ctx.starts, ctx.stops), (2, 1)); });
        let health = http.call_handle(accessor, 2, request("/control/health")).await?;
        assert_eq!(health.status, 200);
        let new_identity: serde_json::Value = serde_json::from_slice(&health.body).unwrap();
        assert_ne!(new_identity["instanceId"], identity["instanceId"]);
        assert_eq!(http.call_handle(accessor, 1, request("/control/health")).await?.status, 503);
        events.call_on_ui_event(accessor, "http_probe_stop".into(), Event::Click, "{}".into()).await?;
        Ok(())
    }).await??;
    fixture_task.join().unwrap();
    let masters: Vec<_> = std::fs::read_dir(directory.path().join("cache/masters"))?.collect::<Result<_, _>>()?;
    assert_eq!(masters.len(), 5, "WASI disk masters must not overwrite one another");
    for index in 0..5 {
        assert!(masters.iter().any(|entry| std::fs::read(entry.path()).unwrap() == picked_image(index)));
    }
    assert_eq!(std::fs::read_to_string(directory.path().join("http-address.txt"))?, "192.168.1.100");
    println!("PASS: version gates 317/318 and 381/382, 3-second startup silence, stale/early handshake timers, single launch per HTTP import, loopback/fallback binding, source/task/image endpoints, JPEG/PNG/LVGL, disk masters and start/stop checked on the actual release WASM.");
    Ok(())
}
