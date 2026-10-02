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
    linker.instance("astrobox:psys-host-v4/register")?.func_new_concurrent("register-interconnect-recv", |_, _, _, results| Box::pin(async move {
        results[0] = Val::Result(Ok(None));
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
    linker.instance("astrobox:psys-host-v4/thirdpartyapp")?.func_new_concurrent("get-thirdparty-app-list", |_, _, _, results| Box::pin(async move {
        let app = Val::Record(vec![
            ("package-name".into(), Val::String("moe.yzf.comic".into())),
            ("fingerprint".into(), Val::List(vec![Val::U32(1)])),
            ("version-code".into(), Val::U32(100)),
            ("can-remove".into(), Val::Bool(true)),
            ("app-name".into(), Val::String("腕上漫画".into())),
        ]);
        results[0] = Val::Result(Ok(Some(Box::new(Val::List(vec![app])))));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/thirdpartyapp")?.func_new_concurrent("launch-qa", |_, _, _, results| Box::pin(async move {
        results[0] = Val::Result(Ok(None));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/interconnect")?.func_wrap_concurrent("send-qaic-message", |accessor, (_addr, _pkg, data): (String, String, String)| {
        accessor.with(|mut access| {
            access.get().sent_interconnect.lock().unwrap().push(data);
        });
        Box::pin(async { Ok((Ok::<(), String>(()),)) })
    })?;

    let mut timers = linker.instance("astrobox:psys-host-v4/timer")?;
    timers.func_wrap("set-timeout", |_, (_delay, _payload): (u64, String)| Ok((1u64,)))?;
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
    });
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let world = bindings::PsysWorldV4Http::new(&mut store, &instance)?;
    let (url, requests, fixture_task) = fixture();
    store.run_concurrent(async |accessor| -> wasmtime::Result<()> {
        use bindings::astrobox::psys_host_v4::{http_server::Request, ui::Event};
        use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
        world.astrobox_psys_plugin_v4_lifecycle().call_on_load(accessor).await?;
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

        // Test HTTP-2 IPv4 input and gateway_bind message dispatch
        events.call_on_ui_event(accessor, "http_probe_ip_input".into(), Event::Change,
            serde_json::json!({"value": "192.168.1.100"}).to_string()).await?;
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;

        let sent = sent_interconnect.lock().unwrap().clone();
        assert!(!sent.is_empty(), "gateway_bind should be sent via interconnect");
        let bind_msg: serde_json::Value = serde_json::from_str(sent.last().unwrap()).unwrap();
        assert_eq!(bind_msg["type"], "gateway_bind");
        assert_eq!(bind_msg["endpoint"], format!("http://192.168.1.100:{}", identity["port"]));
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
        assert_eq!(cfg_val["LocalUpload"]["apiUrl"], format!("http://192.168.1.100:{}", identity["port"]));

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
    println!("PASS: Wasmtime 48.0.2 loads the actual V4 component; lifecycle, handler, JPEG/PNG/LVGL, outgoing p2 HTTP, gateway_bind, start/stop, stale id and concurrent event wakeup checked.");
    Ok(())
}
