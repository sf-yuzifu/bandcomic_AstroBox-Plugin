//! ABI/WASI smoke check with deliberately small test doubles for AstroBox APIs.
//! This exercises the built plugin, but does not replace installed AstroBox tests.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use wasmtime::{Config, Engine, Store};
use wasmtime::component::{Component, Linker, ResourceTable, Val};
use wasmtime::component::{types::ComponentItem, ResourceType, Resource, ResourceAny};
use std::collections::HashMap;
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
    next_pick: Option<(String, Vec<u8>)>,
    app_version: u32,
    launches: usize,
    launched_at: Option<std::time::Instant>,
    registrations: usize,
    timers: Vec<(u64, String)>,
    device_addr: String,
    device_name: String,
    connected: bool,
    ui_nodes: HashMap<u32, Vec<String>>,
    ui_next: u32,
    rendered: HashMap<String, Vec<String>>,
    root_renders: HashMap<String, usize>,
    fail_send_type: Option<String>,
    switch_after_send_type: Option<String>,
    sent_targets: Vec<String>,
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

// BITMAPV4HEADER with explicit RGBA masks: transparent/half-transparent test
// pixels and a tall image exercise the guest decoder, never a host image library.
fn rgba_bmp(width: u32, height: u32, transparent: bool) -> Vec<u8> {
    let size = 122 + width as usize * height as usize * 4;
    let mut bmp = vec![0u8; size];
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&(size as u32).to_le_bytes());
    bmp[10..14].copy_from_slice(&122u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&108u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&width.to_le_bytes());
    bmp[22..26].copy_from_slice(&height.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&32u16.to_le_bytes());
    bmp[30..34].copy_from_slice(&3u32.to_le_bytes());
    for (offset, mask) in [(54, 0x00ff0000u32), (58, 0x0000ff00), (62, 0x000000ff), (66, 0xff000000)] {
        bmp[offset..offset + 4].copy_from_slice(&mask.to_le_bytes());
    }
    for y in 0..height { for x in 0..width {
        let offset = 122 + (y * width + x) as usize * 4;
        let pixel = if transparent { [(y * 7) as u8, (x * 11) as u8, (x * 3 + y * 5) as u8,
            if x < width / 3 { 0 } else if x < width * 2 / 3 { 128 } else { 255 }] }
            else { [80, 120, 200, 255] };
        bmp[offset..offset + 4].copy_from_slice(&pixel);
    } }
    bmp
}

fn image_dimensions(bytes: &[u8], format: &str) -> (u32, u32) {
    match format {
        "image/png" => {
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            (u32::from_be_bytes(bytes[16..20].try_into().unwrap()), u32::from_be_bytes(bytes[20..24].try_into().unwrap()))
        }
        "application/octet-stream" => {
            let header = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            assert_eq!(header & 1023, 10);
            let dims = ((header >> 10) & 2047, header >> 21);
            assert_eq!(bytes.len(), 1028 + (dims.0 * dims.1) as usize);
            dims
        }
        "image/jpeg" => {
            assert_eq!(&bytes[..2], &[255, 216]);
            let mut position = 2;
            loop {
                assert_eq!(bytes[position], 255);
                let marker = bytes[position + 1];
                let length = u16::from_be_bytes(bytes[position + 2..position + 4].try_into().unwrap()) as usize;
                if marker == 0xc0 {
                    return (u16::from_be_bytes(bytes[position + 7..position + 9].try_into().unwrap()) as u32,
                        u16::from_be_bytes(bytes[position + 5..position + 7].try_into().unwrap()) as u32);
                }
                position += 2 + length;
                assert!(position < bytes.len(), "JPEG dimensions missing");
            }
        }
        _ => panic!("unexpected image format"),
    }
}

fn decode_base64(encoded: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    for chunk in encoded.as_bytes().chunks(4) {
        assert_eq!(chunk.len(), 4);
        let value = |byte| alphabet.iter().position(|&b| b == byte).unwrap() as u8;
        let (a, b) = (value(chunk[0]), value(chunk[1]));
        bytes.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = value(chunk[2]); bytes.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' { bytes.push((c << 6) | value(chunk[3])); }
        }
    }
    bytes
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
        for _ in 0..1 {
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

struct SourceFixture {
    root: String,
    count: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SourceFixture {
    fn drop(&mut self) {
        self.stopped.store(true,Ordering::SeqCst);
        if let Some(task) = self.task.take() { task.join().unwrap(); }
    }
}

fn source_fixture() -> SourceFixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let root = format!("http://{}",listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let count = Arc::new(AtomicUsize::new(0)); let calls = count.clone();
    let stopped = Arc::new(AtomicBool::new(false)); let stop = stopped.clone();
    let task = std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let (mut stream,_) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => { std::thread::sleep(std::time::Duration::from_millis(5)); continue; }
                Err(error) => panic!("source fixture accept: {error}"),
            };
            // Windows accepted sockets can inherit the nonblocking listener mode.
            // Read the test request with a real timeout rather than racing WouldBlock.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|b|b == b"\r\n\r\n") {
                let mut buffer = [0u8;1024]; let size = stream.read(&mut buffer).unwrap();
                assert!(size>0); bytes.extend_from_slice(&buffer[..size]);
            }
            let request = String::from_utf8(bytes).unwrap();
            let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
            calls.fetch_add(1,Ordering::SeqCst);
            let config = |api: &str| serde_json::json!({"name":"同名源","apiUrl":api,"detailPath":"/album/<id>",
                "photoPath":"/photo/<id>","searchPath":"/search/<text>/<page>","future":{"preserved":true}});
            let (status,body) = match path {
                "/multi/config" => (200,serde_json::json!({"A":config("https://a.example"),"B":config("https://b.example"),
                    "Broken":{"name":"无效源","apiUrl":"https://bad.example","detailPath":"/album"},"C":config("https://c.example")}).to_string()),
                "/other/config" => (200,serde_json::json!({"A":config("https://other.example")}).to_string()),
                "/paged/config" => {
                    let map: serde_json::Map<_,_> = (0..10).map(|i|(format!("K{i:02}"),config(&format!("https://s{i}.example")))).collect();
                    (200,serde_json::Value::Object(map).to_string())
                }
                "/http-error/config" => (503,"{}".into()),
                "/json-error/config" => (200,"{broken".into()),
                "/model-error/config" => (200,"[]".into()),
                "/all-bad/config" => (200,serde_json::json!({"using":config("https://a.example")}).to_string()),
                "/network-error/config" => continue, // close without a response
                _ => panic!("unexpected source fixture route: {path}"),
            };
            write!(stream,"HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                if status == 200 { "OK" } else { "Service Unavailable" },body.len()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
        }
    });
    SourceFixture { root,count,stopped,task:Some(task) }
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
        let (name, data) = accessor.with(|mut access| {
            let ctx = access.get();
            if let Some(pick) = ctx.next_pick.take() { return pick; }
            let index = ctx.picked_images;
            ctx.picked_images += 1;
            (format!("test_image_{index}.bmp"), picked_image(index))
        });
        let pick_res = Val::Record(vec![
            ("name".into(), Val::String(name)),
            ("data".into(), Val::List(data.into_iter().map(Val::U8).collect())),
        ]);
        results[0] = Val::Result(Ok(Some(Box::new(pick_res))));
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/dialog")?.func_new_concurrent("show-dialog", |_, _, _, results| Box::pin(async move {
        results[0] = Val::Record(vec![("clicked-btn-id".into(), Val::String("confirm".into())),
            ("input-result".into(), Val::String(String::new()))]);
        Ok(())
    }))?;
    linker.instance("astrobox:psys-host-v4/device")?.func_new_concurrent("get-connected-device-list", |accessor, _, _, results| Box::pin(async move {
        let (connected, name, addr) = accessor.with(|mut access| {
            let ctx = access.get(); (ctx.connected, ctx.device_name.clone(), ctx.device_addr.clone())
        });
        let dev = Val::Record(vec![
            ("name".into(), Val::String(name)),
            ("addr".into(), Val::String(addr)),
        ]);
        results[0] = Val::List(if connected { vec![dev] } else { vec![] });
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
    linker.instance("astrobox:psys-host-v4/interconnect")?.func_wrap_concurrent("send-qaic-message", |accessor, (addr, _pkg, data): (String, String, String)| {
        let failed = accessor.with(|mut access| {
            let ctx = access.get();
            assert!(ctx.launched_at.expect("send must follow launch").elapsed() >= std::time::Duration::from_secs(3),
                "interconnect message sent before the 3-second startup delay");
            let message_type = serde_json::from_str::<serde_json::Value>(&data).unwrap()["type"].as_str().unwrap().to_string();
            ctx.sent_targets.push(addr);
            ctx.sent_interconnect.lock().unwrap().push(data);
            let failed = ctx.fail_send_type.as_deref() == Some(&message_type);
            if failed { ctx.fail_send_type = None; }
            if ctx.switch_after_send_type.as_deref() == Some(&message_type) {
                ctx.switch_after_send_type = None; ctx.device_addr = "CC:DD:EE:00:11:22".into(); ctx.device_name = "Switched device".into();
            }
            failed
        });
        Box::pin(async move { Ok((if failed { Err("injected send failure".to_string()) } else { Ok(()) },)) })
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

// Small UI resource host: retain text/event IDs, not a renderer or layout engine.
// This lets the actual guest build/filter its UI and expose the resulting rows.
fn add_ui_test_host(linker: &mut Linker<Context>, engine: &Engine, component: &Component) -> wasmtime::Result<()> {
    let component_type = component.component_type();
    let import = component_type.imports(engine)
        .find(|(name, _)| *name == "astrobox:psys-host-v4/ui").unwrap().1;
    let ComponentItem::ComponentInstance(instance) = import.ty else { unreachable!() };
    let mut interface = linker.instance("astrobox:psys-host-v4/ui")?;
    interface.resource("element", ResourceType::host::<()>(), |mut store, rep| {
        store.data_mut().ui_nodes.remove(&rep); Ok(())
    })?;
    for (name, export) in instance.exports(engine) {
        let ComponentItem::ComponentFunc(function) = export.ty else { continue; };
        if function.async_() { continue; }
        let method = name.to_string();
        interface.func_new(name, move |mut store, _, params, results| {
            let rep = |value: &Val, store: &mut wasmtime::StoreContextMut<'_, Context>| -> wasmtime::Result<u32> {
                let Val::Resource(resource) = value else { panic!("expected UI element") };
                Ok(resource.try_into_resource::<()>(store)?.rep())
            };
            if method == "render" {
                let Val::String(id) = &params[0] else { unreachable!() };
                let key = rep(&params[1], &mut store)?;
                let text = store.data().ui_nodes[&key].clone();
                let ctx = store.data_mut();
                ctx.rendered.insert(id.clone(), text);
                *ctx.root_renders.entry(id.clone()).or_default() += 1;
                return Ok(());
            }
            let mut text = if method.starts_with("[constructor]") {
                match &params[1] { Val::Option(Some(value)) => match &**value {
                    Val::String(content) => vec![content.clone()], _ => vec![],
                }, _ => vec![] }
            } else {
                let key = rep(&params[0], &mut store)?;
                store.data().ui_nodes[&key].clone()
            };
            if method.ends_with(".child") {
                let key = rep(&params[1], &mut store)?;
                text.extend(store.data().ui_nodes[&key].clone());
            } else if method.ends_with(".on") && let Val::String(id) = &params[2] {
                text.push(id.clone());
            }
            let ctx = store.data_mut();
            ctx.ui_next += 1;
            let key = ctx.ui_next;
            ctx.ui_nodes.insert(key, text);
            results[0] = Val::Resource(ResourceAny::try_from_resource(Resource::<()>::new_own(key), &mut store)?);
            Ok(())
        })?;
    }
    Ok(())
}

fn rendered_text(accessor: &wasmtime::component::Accessor<Context>) -> String {
    accessor.with(|mut access| access.get().rendered.get("library-test").unwrap().join("\n"))
}

fn root_render_count(accessor: &wasmtime::component::Accessor<Context>) -> usize {
    accessor.with(|mut access|access.get().root_renders.get("library-test").copied().unwrap_or(0))
}

fn rendered_delete(accessor: &wasmtime::component::Accessor<Context>, prefix: &str) -> Vec<String> {
    accessor.with(|mut access| access.get().rendered.get("library-test").unwrap().iter()
        .filter(|text| text.starts_with(prefix)).cloned().collect())
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
async fn complete_startup_with_caps(
    world: &bindings::PsysWorldV4Http,
    accessor: &wasmtime::component::Accessor<Context>,
    caps: serde_json::Value,
) -> wasmtime::Result<()> {
    complete_startup_with_profile(world, accessor, caps,
        serde_json::json!({ "imageSize": 480, "imageQuality": 50, "imageUsePng": false, "imagePreTranscode": false })).await
}

async fn complete_startup_with_profile(
    world: &bindings::PsysWorldV4Http,
    accessor: &wasmtime::component::Accessor<Context>,
    caps: serde_json::Value,
    settings: serde_json::Value,
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
        "settings": settings,
        "caps": caps
    }).to_string()).await?;
    Ok(())
}

async fn complete_startup(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>) -> wasmtime::Result<()> {
    complete_startup_with_caps(world, accessor, serde_json::json!({"httpImport": true})).await
}

async fn sync_delete_fixture(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::{ui::Event, http_server::Request};
    let events = world.astrobox_psys_plugin_v4_event();
    events.call_on_ui_event(accessor, "comic_search_clear".into(), Event::Click, "{}".into()).await?;
    events.call_on_ui_event(accessor, "source_search_clear".into(), Event::Click, "{}".into()).await?;
    events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
    complete_startup_with_caps(world, accessor, serde_json::json!({"httpDataSync":1,"syncSession":true,"deleteProtocol":1})).await?;
    let message: serde_json::Value = accessor.with(|mut access| serde_json::from_str(access.get().sent_interconnect.lock().unwrap().last().unwrap()).unwrap());
    assert_eq!(message["type"], "request_data");
    let root = format!("/control/sync/{}", message["session"].as_str().unwrap());
    let mut bodies = vec![serde_json::json!({"kind":"header","comicCount":3,"sourceCount":2}),
        serde_json::json!({"kind":"comics","offset":0,"items":[
            {"id":"same_one","name":"同名漫画","page_count":1,"chapters":0},
            {"id":"same_two","name":"同名漫画","page_count":2,"chapters":0},
            {"id":"other","name":"Other","page_count":3,"chapters":0}]}),
        serde_json::json!({"kind":"sources","offset":0,"items":[
            {"key":"source_a","name":"同名源","apiUrl":"http://a"},
            {"key":"source_b","name":"同名源","apiUrl":"http://b"}]}),
        serde_json::json!({"kind":"done"})];
    for body in bodies.drain(..) {
        let response = world.astrobox_psys_plugin_v4_http().call_handle(accessor, 1, Request { method:"POST".into(),
            path:format!("{root}/metadata"),query:String::new(),headers:vec![],body:body.to_string().into_bytes() }).await?;
        assert_eq!(response.status, 200);
    }
    for id in ["same_one","same_two","other"] {
        assert_eq!(world.astrobox_psys_plugin_v4_http().call_handle(accessor, 1, Request { method:"POST".into(),
            path:format!("{root}/skip"),query:String::new(),headers:vec![],body:serde_json::json!({"id":id}).to_string().into_bytes() }).await?.status, 200);
    }
    assert_eq!(world.astrobox_psys_plugin_v4_http().call_handle(accessor, 1, Request { method:"POST".into(),
        path:format!("{root}/complete"),query:String::new(),headers:vec![],body:b"{}".to_vec() }).await?.status, 200);
    // The real HTTP guest must retain these keys before a delete target is captured.
    assert!(rendered_text(accessor).contains("Key：source_b"));
    Ok(())
}

fn delete_result(request: &serde_json::Value, status: &str) -> serde_json::Value {
    serde_json::json!({"type":"delete_result","protocol":1,"requestId":request["requestId"],"session":request["session"],
        "kind":request["kind"],"comicId":request["comicId"],"sourceKey":request["sourceKey"],"status":status,
        "filesState":if request["kind"] == "source" { "not_applicable" } else { "removed" },
        "indexState":if status == "success" { "removed" } else { "retained" },"message":format!("device {status}")})
}

async fn read_sources(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>, endpoint: &str) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::ui::Event;
    let events = world.astrobox_psys_plugin_v4_event();
    events.call_on_ui_event(accessor,"domain_input_change".into(),Event::Change,serde_json::json!({"value":endpoint}).to_string()).await?;
    events.call_on_ui_event(accessor,"source_fetch".into(),Event::Click,"{}".into()).await?;
    Ok(())
}

async fn begin_sources(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::ui::Event;
    let event = rendered_delete(accessor,"source_sync_")[0].clone();
    world.astrobox_psys_plugin_v4_event().call_on_ui_event(accessor,event,Event::Click,"{}".into()).await?;
    Ok(())
}

async fn sync_sources(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>) -> wasmtime::Result<()> {
    begin_sources(world,accessor).await?;
    complete_startup_with_caps(world,accessor,serde_json::json!({})).await
}

async fn check_sources(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::ui::Event;
    use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
    let fixture = source_fixture();
    let events = world.astrobox_psys_plugin_v4_event();
    let (original_addr,original_name,original_version) = accessor.with(|mut access| {
        let ctx = access.get(); let value = (ctx.device_addr.clone(),ctx.device_name.clone(),ctx.app_version); ctx.app_version=318; value
    });
    let commands = |start: usize| accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap()[start..].iter()
        .map(|s|serde_json::from_str::<serde_json::Value>(s).unwrap())
        .filter(|m|matches!(m["type"].as_str(),Some("source_config"|"cookie"))).collect::<Vec<_>>());
    let sent_len = || accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_render(accessor,"library-test".into()).await?;
    events.call_on_ui_event(accessor,"tab_sync".into(),Event::Click,"{}".into()).await?;
    let endpoint = format!("{}/multi",fixture.root);
    let old_sync = rendered_delete(accessor,"source_sync_").into_iter().next();
    let renders = root_render_count(accessor);
    // The real guest must keep the mounted input across typing, paste, delete and
    // the blur emitted by hosts when an unrelated node is replaced. Only read clicks fetch.
    for value in ["".to_string(), "h".into(), "ht".into(), "https://".into(), "https://api.ex".into(),
        endpoint.clone(), endpoint[..endpoint.len()-1].into(), endpoint.clone()] {
        events.call_on_ui_event(accessor,"domain_input_change".into(),Event::Change,serde_json::json!({"value":value}).to_string()).await?;
        events.call_on_ui_event(accessor,"domain_input_blur".into(),Event::Blur,serde_json::json!({"value":value}).to_string()).await?;
        assert_eq!(root_render_count(accessor),renders,"text edits/blur must not replace the focused input");
        assert_eq!(fixture.count.load(Ordering::SeqCst),0,"typing/blur must not fetch unfinished addresses");
    }
    if let Some(old_sync) = old_sync {
        let before = sent_len();
        events.call_on_ui_event(accessor,old_sync,Event::Click,"{}".into()).await?;
        assert_eq!(sent_len(),before,"draft changes invalidate the old snapshot before any redraw");
    }
    events.call_on_ui_event(accessor,"source_fetch".into(),Event::Click,"{}".into()).await?;
    assert_eq!(fixture.count.load(Ordering::SeqCst),1);
    let text = rendered_text(accessor);
    assert!(text.contains("已读取 4 个源，3 个可同步") && text.contains("已选择 0 个"));
    assert!(text.contains("Key：A") && text.contains("Key：B") && text.contains("detailPath"));
    assert!(rendered_delete(accessor,"source_sync_").is_empty(),"multi-source needs explicit selection");
    let select = rendered_delete(accessor,"source_select_"); assert_eq!(select.len(),3,"invalid row has no selector");
    let update = rendered_delete(accessor,"source_cookie_update_");
    let clear = rendered_delete(accessor,"source_cookie_clear_");
    let keep = rendered_delete(accessor,"source_cookie_keep_");
    for id in &select[..2] { events.call_on_ui_event(accessor,id.clone(),Event::Change,r#"{"checked":true}"#.into()).await?; }
    for (index,cookie) in [(0,"cookie-a"),(1,"cookie-b"),(2,"unselected-cookie-c")] {
        events.call_on_ui_event(accessor,update[index].clone(),Event::Click,"{}".into()).await?;
        let input = rendered_delete(accessor,"source_cookie_input_")[index].clone();
        let renders = root_render_count(accessor);
        for end in 1..=cookie.len() {
            events.call_on_ui_event(accessor,input.clone(),Event::Change,serde_json::json!({"value":&cookie[..end]}).to_string()).await?;
            assert_eq!(root_render_count(accessor),renders,"Cookie typing must not replace the focused input");
        }
    }
    let old_input_a = rendered_delete(accessor,"source_cookie_input_")[0].clone();
    events.call_on_ui_event(accessor,clear[1].clone(),Event::Click,"{}".into()).await?;
    let old_sync_event = rendered_delete(accessor,"source_sync_")[0].clone();
    let before = sent_len();
    begin_sources(world,accessor).await?;
    // Even injected stale UI events while waiting cannot edit the captured payload.
    events.call_on_ui_event(accessor,old_input_a.clone(),Event::Change,r#"{"value":"must-not-replace-a"}"#.into()).await?;
    events.call_on_ui_event(accessor,select[1].clone(),Event::Change,r#"{"checked":false}"#.into()).await?;
    complete_startup_with_caps(world,accessor,serde_json::json!({})).await?;
    let sent = commands(before); assert_eq!(sent.len(),2);
    assert_eq!(sent[0]["type"],"source_config"); assert_eq!(sent[0]["configs"].as_array().unwrap().len(),2);
    assert!(sent[0]["configs"][0].get("A").is_some() && sent[0]["configs"][1].get("B").is_some());
    assert_eq!(sent[0]["configs"][0]["A"]["future"]["preserved"],true);
    assert_eq!(sent[1],serde_json::json!({"type":"cookie","A":"cookie-a","B":""}));
    assert_eq!(fixture.count.load(Ordering::SeqCst),1,"sync must not re-fetch /config");
    assert!(rendered_text(accessor).contains("命令已发送，待设备核实"));
    assert!(!rendered_text(accessor).contains("同步成功"));

    // Explicit refresh restores this endpoint's drafts, but old generation events stay stale.
    events.call_on_ui_event(accessor,"source_fetch".into(),Event::Click,"{}".into()).await?;
    assert_eq!(fixture.count.load(Ordering::SeqCst),2);
    let before = sent_len();
    events.call_on_ui_event(accessor,old_sync_event.clone(),Event::Click,"{}".into()).await?;
    assert_eq!(sent_len(),before,"old sync button cannot send the refreshed snapshot");
    events.call_on_ui_event(accessor,old_input_a.clone(),Event::Change,r#"{"value":"wrong-old-generation"}"#.into()).await?;
    assert!(rendered_text(accessor).contains("cookie-a") && !rendered_text(accessor).contains("wrong-old-generation"));
    events.call_on_ui_event(accessor,keep[0].clone(),Event::Click,"{}".into()).await?; // stale keep is ignored too
    let current_keep = rendered_delete(accessor,"source_cookie_keep_");
    events.call_on_ui_event(accessor,current_keep[0].clone(),Event::Click,"{}".into()).await?;
    let before = sent_len(); sync_sources(world,accessor).await?;
    assert_eq!(commands(before)[1],serde_json::json!({"type":"cookie","B":""}),"keep is omission, not clear");
    assert_eq!(fixture.count.load(Ordering::SeqCst),2);

    // The real guest reports failed/partial transport separately from device persistence.
    accessor.with(|mut access|access.get().fail_send_type=Some("source_config".into()));
    let before = sent_len(); sync_sources(world,accessor).await?;
    assert_eq!(commands(before).len(),1,"config send failure cannot send Cookie");
    assert!(rendered_text(accessor).contains("配置命令发送异常") && rendered_text(accessor).contains("Cookie 命令未发送"));
    accessor.with(|mut access|access.get().fail_send_type=Some("cookie".into()));
    let before = sent_len(); sync_sources(world,accessor).await?;
    assert_eq!(commands(before).len(),2);
    assert!(rendered_text(accessor).contains("部分命令已发送") && rendered_text(accessor).contains("Cookie 命令发送异常"));
    accessor.with(|mut access|access.get().switch_after_send_type=Some("source_config".into()));
    let before = sent_len(); sync_sources(world,accessor).await?;
    assert_eq!(commands(before).len(),1,"changed device cannot receive Cookie from the original plan");
    assert!(rendered_text(accessor).contains("连接已变化"));
    accessor.with(|mut access| {
        let ctx = access.get(); assert!(ctx.sent_targets[before..].iter().all(|a|a == &original_addr));
        ctx.device_addr=original_addr.clone(); ctx.device_name=original_name.clone();
    });
    events.call_on_event(accessor,EventType::DeviceAction,"{}".into()).await?;

    // Address mutation during the timer-driven handshake stops the old plan and old blur.
    let before = sent_len();
    begin_sources(world,accessor).await?;
    let other = format!("{}/other",fixture.root);
    events.call_on_ui_event(accessor,"domain_input_change".into(),Event::Change,serde_json::json!({"value":other}).to_string()).await?;
    complete_startup_with_caps(world,accessor,serde_json::json!({})).await?;
    assert!(commands(before).is_empty());
    assert!(rendered_text(accessor).contains("配置入口已变化") && !rendered_text(accessor).contains("Key：A"));
    let count = fixture.count.load(Ordering::SeqCst);
    events.call_on_ui_event(accessor,"domain_input_blur".into(),Event::Blur,serde_json::json!({"value":endpoint}).to_string()).await?;
    assert_eq!(fixture.count.load(Ordering::SeqCst),count,"stale blur cannot restore the previous endpoint");
    events.call_on_ui_event(accessor,"domain_input_blur".into(),Event::Blur,serde_json::json!({"value":other}).to_string()).await?;
    assert_eq!(fixture.count.load(Ordering::SeqCst),count,"current blur also waits for an explicit read click");
    events.call_on_ui_event(accessor,"source_fetch".into(),Event::Click,"{}".into()).await?;
    assert_eq!(fixture.count.load(Ordering::SeqCst),count+1);
    assert!(rendered_text(accessor).contains("已选择 1 个"),"single valid source defaults selected");
    let invalid_sync_event = rendered_delete(accessor,"source_sync_")[0].clone();
    let update_a = rendered_delete(accessor,"source_cookie_update_")[0].clone();
    events.call_on_ui_event(accessor,update_a,Event::Click,"{}".into()).await?;
    events.call_on_ui_event(accessor,old_input_a,Event::Change,r#"{"value":"credential-from-old-endpoint"}"#.into()).await?;
    assert!(!rendered_text(accessor).contains("cookie-a") && !rendered_text(accessor).contains("credential-from-old-endpoint"));
    assert_eq!(rendered_delete(accessor,"source_sync_").len(),1,"sync validates the latest Cookie draft when clicked");
    let before = sent_len(); events.call_on_ui_event(accessor,invalid_sync_event,Event::Click,"{}".into()).await?;
    assert_eq!(sent_len(),before,"invalid Cookie is blocked before handshake or any command");
    let keep_a = rendered_delete(accessor,"source_cookie_keep_")[0].clone();
    events.call_on_ui_event(accessor,keep_a,Event::Click,"{}".into()).await?;
    begin_sources(world,accessor).await?;
    accessor.with(|mut access| { access.get().device_addr="FF:00:11:22:33:44".into(); });
    events.call_on_event(accessor,EventType::DeviceAction,"{}".into()).await?;
    let before = sent_len(); complete_startup_with_caps(world,accessor,serde_json::json!({})).await?;
    assert!(commands(before).is_empty() && rendered_text(accessor).contains("原目标设备已断开或切换"));
    accessor.with(|mut access| { let ctx=access.get(); ctx.device_addr=original_addr.clone(); ctx.device_name=original_name.clone(); });
    events.call_on_event(accessor,EventType::DeviceAction,"{}".into()).await?;

    // Error state persists after transient status timers and cannot reuse the old snapshot.
    for (path,reason) in [("http-error","HTTP 503"),("json-error","配置 JSON 错误"),("model-error","字段 config"),
        ("all-bad","可同步；1 个配置无效"),("network-error","网络错误")] {
        let uri = format!("{}/{path}",fixture.root);
        let count = fixture.count.load(Ordering::SeqCst); read_sources(world,accessor,&uri).await?;
        assert_eq!(fixture.count.load(Ordering::SeqCst),count+1);
        assert!(rendered_text(accessor).contains(reason),"{path}: {}",rendered_text(accessor));
        events.call_on_event(accessor,EventType::Timer,r#"{"payload":"hide_status"}"#.into()).await?;
        assert!(rendered_text(accessor).contains(reason));
        assert!(rendered_delete(accessor,"source_sync_").is_empty());
        let before=sent_len(); events.call_on_ui_event(accessor,old_sync_event.clone(),Event::Click,"{}".into()).await?;
        assert_eq!(sent_len(),before); assert_eq!(fixture.count.load(Ordering::SeqCst),count+1);
        assert!(rendered_text(accessor).contains(reason),"stale sync button cannot erase the specific read error");
    }
    let before = sent_len();
    events.call_on_ui_event(accessor,"domain_input_change".into(),Event::Change,r#"{"value":"ftp://example.com"}"#.into()).await?;
    let count = fixture.count.load(Ordering::SeqCst);
    events.call_on_ui_event(accessor,"source_fetch".into(),Event::Click,"{}".into()).await?;
    assert!(rendered_text(accessor).contains("地址错误")); assert_eq!(fixture.count.load(Ordering::SeqCst),count);
    assert_eq!(sent_len(),before);

    // Larger catalogs render eight rows; original slots on the second page build one filtered payload.
    read_sources(world,accessor,&format!("{}/paged",fixture.root)).await?;
    assert_eq!(rendered_delete(accessor,"source_select_").len(),8);
    assert!(rendered_text(accessor).contains("Key：K00") && !rendered_text(accessor).contains("Key：K09"));
    events.call_on_ui_event(accessor,"source_catalog_next".into(),Event::Click,"{}".into()).await?;
    assert_eq!(rendered_delete(accessor,"source_select_").len(),2);
    assert!(rendered_text(accessor).contains("Key：K09"));
    let all = rendered_delete(accessor,"source_all_")[0].clone();
    events.call_on_ui_event(accessor,all,Event::Click,"{}".into()).await?;
    let update_last = rendered_delete(accessor,"source_cookie_update_")[1].clone();
    events.call_on_ui_event(accessor,update_last,Event::Click,"{}".into()).await?;
    let input_last = rendered_delete(accessor,"source_cookie_input_")[0].clone();
    events.call_on_ui_event(accessor,input_last,Event::Change,r#"{"value":"last-page-cookie"}"#.into()).await?;
    let before=sent_len(); let count=fixture.count.load(Ordering::SeqCst);
    sync_sources(world,accessor).await?;
    let sent=commands(before);
    assert_eq!(sent[0]["configs"].as_array().unwrap().len(),10);
    assert_eq!(sent[1],serde_json::json!({"type":"cookie","K09":"last-page-cookie"}));
    assert_eq!(fixture.count.load(Ordering::SeqCst),count);
    accessor.with(|mut access|access.get().app_version=original_version);
    Ok(())
}

async fn pick_test_image(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>, event: &str, name: &str, data: Vec<u8>) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::ui::Event;
    use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
    accessor.with(|mut access| access.get().next_pick = Some((name.into(), data)));
    world.astrobox_psys_plugin_v4_event().call_on_ui_event(accessor,event.into(),Event::Click,"{}".into()).await?;
    world.astrobox_psys_plugin_v4_event().call_on_event(accessor,EventType::Timer,serde_json::json!({"payload":"pick_process"}).to_string()).await?;
    Ok(())
}

async fn receive_legacy_import(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>, header: &serde_json::Value) -> wasmtime::Result<HashMap<String, Vec<u8>>> {
    use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
    let events = world.astrobox_psys_plugin_v4_event();
    let name = header["name"].as_str().unwrap();
    let mut cursor = accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_event(accessor,EventType::InterconnectMessage,serde_json::json!({"type":"import_header_ack","name":name}).to_string()).await?;
    let mut files: HashMap<String, (usize, std::collections::BTreeMap<usize, String>)> = HashMap::new();
    for _ in 0..10000 {
        let messages: Vec<serde_json::Value> = accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap()[cursor..]
            .iter().map(|s|serde_json::from_str(s).unwrap()).collect());
        assert!(!messages.is_empty(), "legacy sender stalled");
        cursor += messages.len();
        let mut last = None;
        let mut done = false;
        for message in messages {
            if message["type"] == "import_comic_done" { done = true; continue; }
            assert_eq!(message["type"], "import_comic_chunk");
            let file = message["file"].as_str().unwrap().to_string();
            let total = message["total"].as_u64().unwrap() as usize;
            let entry = files.entry(file).or_insert_with(|| (total, Default::default()));
            assert_eq!(entry.0, total);
            entry.1.insert(message["index"].as_u64().unwrap() as usize, message["data"].as_str().unwrap().into());
            last = Some(message);
        }
        if done {
            return Ok(files.into_iter().map(|(file,(total,chunks))| {
                assert_eq!(chunks.len(),total);
                assert!(chunks.keys().copied().eq(0..total));
                (file,decode_base64(&chunks.into_values().collect::<String>()))
            }).collect());
        }
        let last = last.unwrap();
        let ack = if let Some(gseq) = last["gseq"].as_u64() {
            serde_json::json!({"type":"import_chunk_ack","name":name,"ack":gseq+1})
        } else { serde_json::json!({"type":"import_chunk_ack","name":name,"file":last["file"],"index":last["index"]}) };
        events.call_on_event(accessor,EventType::InterconnectMessage,ack.to_string()).await?;
    }
    panic!("legacy import did not finish");
}

async fn compare_image_import(
    world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>, directory: &std::path::Path,
    event: &str, outputs: &[(&str, &str)], png: bool, lvgl: bool, window: bool,
) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::{ui::Event, http_server::Request};
    let http = world.astrobox_psys_plugin_v4_http();
    let events = world.astrobox_psys_plugin_v4_event();
    let query = format!("width=96&quality=65&ifPNG={}&ifLVGL={}",u8::from(png),u8::from(lvgl));
    let request = |path: &str| Request { method:"GET".into(),path:path.into(),query:query.clone(),headers:vec![],body:vec![] };
    let mut expected = HashMap::new();
    for &(file,path) in outputs {
        let response = http.call_handle(accessor,1,request(path)).await?;
        assert_eq!(response.status,200);
        let mime = if file == "cover" { if png { "image/png" } else { "image/jpeg" } }
            else if lvgl { "application/octet-stream" } else if png { "image/png" } else { "image/jpeg" };
        assert!(response.headers.iter().any(|h|h.name.eq_ignore_ascii_case("content-type") && h.value == mime));
        let (width,height) = image_dimensions(&response.body,mime);
        if file == "cover" { assert_eq!(width,80); }
        if lvgl && file != "cover" { assert!(width <= 2047 && height <= 2047); }
        expected.insert(file.to_string(),response.body);
    }
    // Independently encode on the legacy path, then exercise real cache hits and corruption.
    std::fs::remove_dir_all(directory.join("cache/rendered"))?;
    let before = accessor.with(|mut access|access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor,event.into(),Event::Click,"{}".into()).await?;
    complete_startup_with_profile(world,accessor,if window { serde_json::json!({"importWindow":4}) } else { serde_json::json!({}) },
        serde_json::json!({"imageSize":96,"imageQuality":65,"imageUsePng":png,"imagePreTranscode":lvgl})).await?;
    let header: serde_json::Value = accessor.with(|mut access|access.get().sent_interconnect.lock().unwrap()[before..].iter()
        .map(|s|serde_json::from_str::<serde_json::Value>(s).unwrap()).find(|v|v["type"] == "import_comic_header").expect("valid images must produce a header"));
    assert_eq!(header.get("wchunks").is_some(),window);
    let actual = receive_legacy_import(world,accessor,&header).await?;
    assert_eq!(actual,expected,"real HTTP body and reassembled legacy payload differ");
    if lvgl && outputs.iter().any(|(_,path)|path.contains("/2.jpg")) {
        assert!(rendered_text(accessor).contains("2047") && rendered_text(accessor).contains("整体缩小"));
    }
    for &(file,path) in outputs {
        assert_eq!(http.call_handle(accessor,1,request(path)).await?.body,expected[file],"cache hit changed the product");
    }
    for entry in std::fs::read_dir(directory.join("cache/rendered"))? {
        std::fs::write(entry?.path(),b"truncated-cache")?;
    }
    for &(file,path) in outputs {
        assert_eq!(http.call_handle(accessor,1,request(path)).await?.body,expected[file],"corrupt cache changed the product");
    }
    Ok(())
}

async fn check_images(world: &bindings::PsysWorldV4Http, accessor: &wasmtime::component::Accessor<Context>, directory: &std::path::Path) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::{ui::Event, http_server::Request};
    let events = world.astrobox_psys_plugin_v4_event();
    events.call_on_ui_render(accessor,"library-test".into()).await?;
    events.call_on_ui_event(accessor,"tab_upload".into(),Event::Click,"{}".into()).await?;
    events.call_on_ui_event(accessor,"upload_clear".into(),Event::Click,"{}".into()).await?;
    events.call_on_ui_event(accessor,"upload_mode_single".into(),Event::Click,"{}".into()).await?;
    pick_test_image(world,accessor,"upload_pick_files","transparent.bmp",rgba_bmp(240,480,true)).await?;
    pick_test_image(world,accessor,"upload_pick_files","long.bmp",rgba_bmp(64,6000,false)).await?;
    assert!(rendered_text(accessor).contains("已整理 2 页正文"));
    for (png,lvgl,window) in [(false,false,false),(true,false,true),(false,true,true),(true,true,true)] {
        let first = if lvgl { "1.bin" } else { "1" };
        let second = if lvgl { "2.bin" } else { "2" };
        compare_image_import(world,accessor,directory,"upload_start",&[
            ("cover","/local/album/single_1/cover"),(first,"/local/photo/single_1/chapter/1/1.jpg"),
            (second,"/local/photo/single_1/chapter/1/2.jpg")],png,lvgl,window).await?;
    }
    events.call_on_ui_event(accessor,"upload_clear".into(),Event::Click,"{}".into()).await?;
    events.call_on_ui_event(accessor,"upload_mode_multi".into(),Event::Click,"{}".into()).await?;
    events.call_on_ui_event(accessor,"upload_add_chapter".into(),Event::Click,"{}".into()).await?;
    pick_test_image(world,accessor,"chapter_pick_files_0","transparent.bmp",rgba_bmp(240,480,true)).await?;
    pick_test_image(world,accessor,"chapter_pick_files_0","long.bmp",rgba_bmp(64,6000,false)).await?;
    compare_image_import(world,accessor,directory,"upload_start",&[
        ("cover","/local/album/multi_1/cover"),("1　第1章/1","/local/photo/multi_1/chapter/1/1.jpg"),
        ("1　第1章/2","/local/photo/multi_1/chapter/1/2.jpg")],true,false,true).await?;
    compare_image_import(world,accessor,directory,"chapter_upload_0",&[
        ("1.bin","/local/photo/multi_1/chapter/1/1.jpg"),("2.bin","/local/photo/multi_1/chapter/1/2.jpg")],false,true,false).await?;

    // Picker rejects bad data before it becomes a valid body page.
    events.call_on_ui_event(accessor,"upload_clear".into(),Event::Click,"{}".into()).await?;
    events.call_on_ui_event(accessor,"upload_mode_single".into(),Event::Click,"{}".into()).await?;
    pick_test_image(world,accessor,"upload_pick_files","bad.png",b"<html>bad image</html>".to_vec()).await?;
    assert!(rendered_text(accessor).contains("bad.png") && rendered_text(accessor).contains("解码失败"));
    assert!(rendered_text(accessor).contains("已整理 0 页正文"));
    let mut oversized = rgba_bmp(1,1,false);
    oversized[18..22].copy_from_slice(&4096u32.to_le_bytes()); oversized[22..26].copy_from_slice(&9000u32.to_le_bytes());
    pick_test_image(world,accessor,"upload_pick_files","oversized.bmp",oversized).await?;
    assert!(rendered_text(accessor).contains("预算失败") && rendered_text(accessor).contains("已整理 0 页正文"),"{}",rendered_text(accessor));

    // Missing/corrupt disk data must fail on HTTP and before any legacy header.
    pick_test_image(world,accessor,"upload_pick_files","missing.bmp",rgba_bmp(240,480,true)).await?;
    for entry in std::fs::read_dir(directory.join("cache/masters"))? { std::fs::remove_file(entry?.path())?; }
    let request = || Request { method:"GET".into(),path:"/local/photo/single_1/chapter/1/1.jpg".into(),query:"ifLVGL=1".into(),headers:vec![],body:vec![] };
    let response = world.astrobox_psys_plugin_v4_http().call_handle(accessor,1,request()).await?;
    assert_eq!(response.status,500);
    assert!(serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()["message"].as_str().unwrap().contains("读取"));
    let before = accessor.with(|mut access|access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor,"upload_start".into(),Event::Click,"{}".into()).await?;
    complete_startup_with_profile(world,accessor,serde_json::json!({}),serde_json::json!({"imagePreTranscode":true})).await?;
    assert!(rendered_text(accessor).contains("missing.bmp") && rendered_text(accessor).contains("读取失败"));
    accessor.with(|mut access| assert!(!access.get().sent_interconnect.lock().unwrap()[before..].iter().any(|s|s.contains("import_comic_header"))));
    events.call_on_ui_event(accessor,"upload_clear".into(),Event::Click,"{}".into()).await?;
    pick_test_image(world,accessor,"upload_pick_files","corrupt.bmp",rgba_bmp(240,480,true)).await?;
    for entry in std::fs::read_dir(directory.join("cache/masters"))? { std::fs::write(entry?.path(),b"corrupt-master")?; }
    assert_eq!(world.astrobox_psys_plugin_v4_http().call_handle(accessor,1,request()).await?.status,422);
    let before = accessor.with(|mut access|access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor,"upload_start".into(),Event::Click,"{}".into()).await?;
    complete_startup_with_profile(world,accessor,serde_json::json!({}),serde_json::json!({"imagePreTranscode":true})).await?;
    assert!(rendered_text(accessor).contains("corrupt.bmp") && rendered_text(accessor).contains("解码失败"));
    accessor.with(|mut access| assert!(!access.get().sent_interconnect.lock().unwrap()[before..].iter().any(|s|s.contains("import_comic_header"))));
    // Re-selecting valid input repairs the disk master without requiring cache cleanup UI.
    events.call_on_ui_event(accessor,"upload_clear".into(),Event::Click,"{}".into()).await?;
    pick_test_image(world,accessor,"upload_pick_files","repaired.bmp",rgba_bmp(240,480,true)).await?;
    assert_eq!(world.astrobox_psys_plugin_v4_http().call_handle(accessor,1,request()).await?.status,200);
    Ok(())
}

async fn check_import_result_protocol(
    world: &bindings::PsysWorldV4Http,
    accessor: &wasmtime::component::Accessor<Context>,
) -> wasmtime::Result<()> {
    use bindings::astrobox::psys_host_v4::ui::Event;
    use bindings::exports::astrobox::psys_plugin_v4::event::EventType;
    let events = world.astrobox_psys_plugin_v4_event();
    events.call_on_ui_render(accessor, "library-test".into()).await?;
    events.call_on_ui_event(accessor, "tab_upload".into(), Event::Click, "{}".into()).await?;
    events.call_on_ui_event(accessor, "upload_clear".into(), Event::Click, "{}".into()).await?;
    events.call_on_ui_event(accessor, "upload_mode_single".into(), Event::Click, "{}".into()).await?;
    pick_test_image(world, accessor, "upload_pick_files", "p1.bmp", rgba_bmp(100, 100, false)).await?;
    pick_test_image(world, accessor, "upload_pick_files", "p2.bmp", rgba_bmp(100, 100, false)).await?;

    // 1. 成功导入并结算
    let before = accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor, "upload_start".into(), Event::Click, "{}".into()).await?;
    complete_startup_with_profile(
        world,
        accessor,
        serde_json::json!({ "importWindow": 4, "importResultProtocol": 1 }),
        serde_json::json!({ "imageSize": 100, "imageQuality": 60, "imageUsePng": false, "imagePreTranscode": false }),
    ).await?;
    let header: serde_json::Value = accessor.with(|mut access| {
        access.get().sent_interconnect.lock().unwrap()[before..]
            .iter()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
            .find(|v| v["type"] == "import_comic_header")
            .unwrap()
    });
    let session_id = header["sessionId"].as_str().unwrap().to_string();
    let name = header["name"].as_str().unwrap().to_string();
    assert!(!session_id.is_empty(), "header must carry sessionId");

    receive_legacy_import(world, accessor, &header).await?;
    let text = rendered_text(accessor);
    assert!(text.contains("正在保存《") && text.contains("并写入索引"), "{}", text);

    // 回复成功的最终结果
    events.call_on_event(
        accessor,
        EventType::InterconnectMessage,
        serde_json::json!({
            "type": "import_comic_result",
            "sessionId": session_id,
            "name": name,
            "success": true,
            "savedPages": 2,
            "totalPages": 2,
            "failedFiles": 0,
            "indexSuccess": true,
            "error": null,
        }).to_string(),
    ).await?;
    let text = rendered_text(accessor);
    assert!(text.contains("🎉 《") && text.contains("已成功保存到手环！共 2 页"), "{}", text);

    // 2. 超时回退（再次上传）
    let before = accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor, "upload_start".into(), Event::Click, "{}".into()).await?;
    complete_startup_with_profile(
        world,
        accessor,
        serde_json::json!({ "importWindow": 4, "importResultProtocol": 1 }),
        serde_json::json!({ "imageSize": 100, "imageQuality": 60, "imageUsePng": false, "imagePreTranscode": false }),
    ).await?;
    let header: serde_json::Value = accessor.with(|mut access| {
        access.get().sent_interconnect.lock().unwrap()[before..]
            .iter()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
            .find(|v| v["type"] == "import_comic_header")
            .unwrap()
    });
    let session_id = header["sessionId"].as_str().unwrap().to_string();
    receive_legacy_import(world, accessor, &header).await?;
    assert!(rendered_text(accessor).contains("正在保存"));
    // 触发超时
    events.call_on_event(
        accessor,
        EventType::Timer,
        serde_json::json!({ "payload": format!("upload_result_timeout:{session_id}") }).to_string(),
    ).await?;
    let text = rendered_text(accessor);
    assert!(text.contains("设备保存确认超时（待核实）"), "{}", text);

    // 3. 部分失败与索引失败反馈
    let before = accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor, "upload_start".into(), Event::Click, "{}".into()).await?;
    complete_startup_with_profile(
        world,
        accessor,
        serde_json::json!({ "importWindow": 4, "importResultProtocol": 1 }),
        serde_json::json!({ "imageSize": 100, "imageQuality": 60, "imageUsePng": false, "imagePreTranscode": false }),
    ).await?;
    let header: serde_json::Value = accessor.with(|mut access| {
        access.get().sent_interconnect.lock().unwrap()[before..]
            .iter()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
            .find(|v| v["type"] == "import_comic_header")
            .unwrap()
    });
    let session_id = header["sessionId"].as_str().unwrap().to_string();
    let name = header["name"].as_str().unwrap().to_string();
    receive_legacy_import(world, accessor, &header).await?;
    events.call_on_event(
        accessor,
        EventType::InterconnectMessage,
        serde_json::json!({
            "type": "import_comic_result",
            "sessionId": session_id,
            "name": name,
            "success": false,
            "savedPages": 1,
            "totalPages": 2,
            "failedFiles": 1,
            "indexSuccess": true,
            "error": "1 个文件保存失败",
        }).to_string(),
    ).await?;
    let text = rendered_text(accessor);
    assert!(text.contains("导入未完全成功") && text.contains("1/2 页"), "{}", text);

    // 4. 未知状态响应
    let before = accessor.with(|mut access| access.get().sent_interconnect.lock().unwrap().len());
    events.call_on_ui_event(accessor, "upload_start".into(), Event::Click, "{}".into()).await?;
    complete_startup_with_profile(
        world,
        accessor,
        serde_json::json!({ "importWindow": 4, "importResultProtocol": 1 }),
        serde_json::json!({ "imageSize": 100, "imageQuality": 60, "imageUsePng": false, "imagePreTranscode": false }),
    ).await?;
    let header: serde_json::Value = accessor.with(|mut access| {
        access.get().sent_interconnect.lock().unwrap()[before..]
            .iter()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
            .find(|v| v["type"] == "import_comic_header")
            .unwrap()
    });
    let session_id = header["sessionId"].as_str().unwrap().to_string();
    receive_legacy_import(world, accessor, &header).await?;
    events.call_on_event(
        accessor,
        EventType::InterconnectMessage,
        serde_json::json!({
            "type": "import_comic_result_status",
            "sessionId": session_id,
            "name": "any",
            "status": "unknown",
        }).to_string(),
    ).await?;
    let text = rendered_text(accessor);
    assert!(text.contains("无该次保存记录（待核实）"), "{}", text);

    Ok(())
}

#[tokio::main]
async fn main() -> wasmtime::Result<()> {
    let wasm = std::env::args().nth(1).expect("usage: v4-runtime-check <plugin.wasm>");
    let sources_only = std::env::args().any(|a|a == "--sources-only");
    let images_only = std::env::args().any(|a|a == "--images-only");
    let import_results_only = std::env::args().any(|a|a == "--import-results-only");
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
    add_ui_test_host(&mut linker, &engine, &component)?;
    let directory = tempfile::tempdir_in(std::env::temp_dir().join("opencode")).unwrap();
    let mut wasi = WasiCtxBuilder::new();
    // Guest debug output is also written to its sandbox log; keep the smoke-check console bounded.
    wasi.inherit_stderr().preopened_dir(directory.path(), ".", FsPerms::ReadWrite).unwrap();
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
        next_pick: None,
        app_version: 382,
        launches: 0,
        launched_at: None,
        registrations: 0,
        timers: Vec::new(),
        device_addr: "11:22:33:44:55:66".into(),
        device_name: "Xiaomi Smart Band 9 Pro".into(),
        connected: true,
        ui_nodes: HashMap::new(),
        ui_next: 0,
        rendered: HashMap::new(),
        root_renders: HashMap::new(),
        fail_send_type: None,
        switch_after_send_type: None,
        sent_targets: Vec::new(),
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
        events.call_on_ui_render(accessor,"library-test".into()).await?;
        events.call_on_ui_event(accessor,"tab_sync".into(),Event::Click,"{}".into()).await?;
        assert_eq!(rendered_delete(accessor,"source_fetch").len(),1,"the read action stays reachable while the address draft is empty");
        let renders = root_render_count(accessor);
        events.call_on_ui_event(accessor,"domain_input_change".into(),Event::Change,serde_json::json!({"value":url}).to_string()).await?;
        assert_eq!(root_render_count(accessor),renders);
        assert_eq!(requests.load(Ordering::SeqCst),0);
        events.call_on_ui_event(accessor,"source_fetch".into(),Event::Click,"{}".into()).await?;
        assert_eq!(requests.load(Ordering::SeqCst), 1, "one outgoing /config fetch produces the complete snapshot");
        if import_results_only { check_import_result_protocol(&world,accessor).await?; return Ok(()); }
        if images_only { check_images(&world,accessor,directory.path()).await?; return Ok(()); }
        if sources_only {
            check_sources(&world,accessor).await?;
            return Ok(());
        }
        events.call_on_ui_render(accessor,"library-test".into()).await?;
        events.call_on_ui_event(accessor,"tab_sync".into(),Event::Click,"{}".into()).await?;
        let source_sync_event = rendered_delete(accessor,"source_sync_")[0].clone();

        // Both management tabs reject 317 before launch, registration or send.
        accessor.with(|mut access| access.get().app_version = 317);
        for event in [source_sync_event.as_str(), "fetch_app_data"] {
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
        events.call_on_ui_event(accessor, source_sync_event, Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let last: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(last["type"], "source_config");
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let last: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(last["type"], "request_data");
        assert!(last.get("http").is_none(), "318 without HTTP capability stays on interconnect");

        // New capability is independent of the 382 upload gate. Validate real HTTP callbacks.
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        complete_startup_with_caps(&world, accessor, serde_json::json!({"httpImport": true, "httpDataSync": 1, "syncSession": true})).await?;
        let sync_request: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(sync_request["type"], "request_data");
        assert_eq!(sync_request["http"]["protocol"], 1);
        assert_eq!(sync_request["http"]["instanceId"], identity["instanceId"]);
        assert_eq!(sync_request["http"]["chunkBytes"], 16384);
        let sync_root = format!("/control/sync/{}", sync_request["session"].as_str().unwrap());
        let sync_http = |method: &str, resource: &str, query: &str, body: Vec<u8>| Request {
            method: method.into(), path: format!("{sync_root}/{resource}"), query: query.into(), headers: vec![], body,
        };
        let probe = vec![0, 1, 127, 128, 255, 0, 42, 13, 10];
        assert_eq!(http.call_handle(accessor, 1, sync_http("POST", "probe", "", probe)).await?.status, 200);
        assert_eq!(http.call_handle(accessor, 1, sync_http("POST", "probe", "", vec![0, 255])).await?.status, 422);
        for value in [
            serde_json::json!({"kind":"header", "comicCount":2, "sourceCount":1}),
            serde_json::json!({"kind":"comics", "offset":0, "items":[
                {"id":"first", "name":"Same", "page_count":2, "chapters":0},
                {"id":"second", "name":"Same", "page_count":3, "chapters":0}]}),
            serde_json::json!({"kind":"sources", "offset":0, "items":[{"name":"Local", "apiUrl":"http://source"}]}),
            serde_json::json!({"kind":"done"}),
        ] {
            let req = sync_http("POST", "metadata", "", value.to_string().into_bytes());
            assert_eq!(http.call_handle(accessor, 1, req.clone()).await?.status, 200);
            assert_eq!(http.call_handle(accessor, 1, req).await?.status, 200, "metadata replay must be idempotent");
        }
        assert_eq!(http.call_handle(accessor, 1, sync_http("POST", "complete", "", b"{}".to_vec())).await?.status, 409);
        let png = http.call_handle(accessor, 1, request("/control/probe.png")).await?.body;
        for (offset, bytes) in [(0, png[..8].to_vec()), (8, png[8..].to_vec())] {
            let req = sync_http("PUT", "covers/1", &format!("offset={offset}&total={}", png.len()), bytes);
            assert_eq!(http.call_handle(accessor, 1, req.clone()).await?.status, 200);
            assert_eq!(http.call_handle(accessor, 1, req).await?.status, 200, "binary chunk replay must be idempotent");
        }
        assert_eq!(http.call_handle(accessor, 1, sync_http("POST", "skip", "", br#"{"id":"first"}"#.to_vec())).await?.status, 200);
        let complete = http.call_handle(accessor, 1, sync_http("POST", "complete", "", b"{}".to_vec())).await?;
        assert_eq!(complete.status, 200);
        let ack: serde_json::Value = serde_json::from_slice(&complete.body).unwrap();
        assert_eq!(ack["receivedComics"], 2);
        assert_eq!(ack["receivedSources"], 1);
        assert_eq!(ack["resolvedCovers"], 2);
        assert_eq!(ack["skippedCovers"], 1);
        assert_eq!(http.call_handle(accessor, 1, sync_http("POST", "complete", "", b"{}".to_vec())).await?.status, 200);
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        assert_eq!(http.call_handle(accessor, 1, sync_http("POST", "complete", "", b"{}".to_vec())).await?.status, 409,
            "old session cannot update a new data request");
        complete_startup(&world, accessor).await?;

        // Upload-tab connection testing also requires 382.
        accessor.with(|mut access| access.get().app_version = 381);
        let sent_count = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, "http_probe_bind_device".into(), Event::Click, "{}".into()).await?;
        accessor.with(|mut access| assert_eq!(access.get().launches, 4));
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
        assert_eq!(task_detail["totalPages"], 5);
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
        for page in 1..=5 {
            let image = http.call_handle(accessor, 1,
                req_query(&format!("/local/photo/{book_id}/chapter/1/{page}.jpg"), "width=80&ifPNG=1")).await?;
            assert_eq!(image.status, 200);
            if page > 1 {
                assert!(!image_bodies.contains(&image.body), "distinct pages must have distinct content");
            }
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
            body: serde_json::json!({ "page": 1, "total": 5 }).to_string().into_bytes(),
        };
        let prog_resp = http.call_handle(accessor, 1, prog_req).await?;
        assert_eq!(prog_resp.status, 200);

        // 3. Report final result
        let res_req = Request {
            method: "POST".into(),
            path: format!("/control/tasks/{}/result", task_id),
            query: "".into(),
            headers: vec![],
            body: serde_json::json!({ "success": true, "savedPages": 5, "totalPages": 5 }).to_string().into_bytes(),
        };
        let res_resp = http.call_handle(accessor, 1, res_req).await?;
        assert_eq!(res_resp.status, 200);

        // P1-42: real guest UI, filtered slots, HTTP/legacy completion and ownership.
        events.call_on_ui_render(accessor, "library-test".into()).await?;
        events.call_on_ui_event(accessor, "tab_data".into(), Event::Click, "{}".into()).await?;
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        complete_startup_with_caps(&world, accessor, serde_json::json!({"httpDataSync":1, "syncSession":true})).await?;
        let message: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        let root = format!("/control/sync/{}", message["session"].as_str().unwrap());
        let callback = |resource: &str, body: serde_json::Value| Request {
            method: "POST".into(), path: format!("{root}/{resource}"), query: String::new(), headers: vec![],
            body: body.to_string().into_bytes(),
        };
        assert_eq!(http.call_handle(accessor, 1, callback("metadata", serde_json::json!({
            "kind":"header", "comicCount":25, "sourceCount":10}))).await?.status, 200);
        for (start, end) in [(0, 16), (16, 25)] {
            let items: Vec<_> = (start..end).map(|i| serde_json::json!({"id":format!("id{i}"),
                "name":format!("{} {i}", if i % 2 == 0 { "Book" } else { "Other" }), "page_count":3, "chapters":0})).collect();
            assert_eq!(http.call_handle(accessor, 1, callback("metadata", serde_json::json!({
                "kind":"comics", "offset":start, "items":items}))).await?.status, 200);
        }
        let sources: Vec<_> = (0..10).map(|i| serde_json::json!({"name":format!("Source {i}"), "apiUrl":format!("http://source{i}")})).collect();
        assert_eq!(http.call_handle(accessor, 1, callback("metadata", serde_json::json!({
            "kind":"sources", "offset":0, "items":sources}))).await?.status, 200);
        assert_eq!(http.call_handle(accessor, 1, callback("metadata", serde_json::json!({"kind":"done"}))).await?.status, 200);
        let sent_before_search = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, "comic_search".into(), Event::Change, r#"{"value":" BOOK "}"#.into()).await?;
        events.call_on_ui_event(accessor, "comic_page_next".into(), Event::Click, "{}".into()).await?;
        events.call_on_ui_event(accessor, "source_search".into(), Event::Change, r#"{"value":"Source 9"}"#.into()).await?;
        let text = rendered_text(accessor);
        assert!(text.contains("匹配 13 本") && text.contains("Book 16") && text.contains("Book 24"));
        assert!(!text.contains("Book 0\n") && !text.contains("Other 17"));
        assert!(text.contains("Source 9") && !text.contains("Source 0\n"));
        assert_eq!(sent_interconnect.lock().unwrap().len(), sent_before_search, "filter/page changes are local");
        assert_eq!(http.call_handle(accessor, 1, Request { method:"PUT".into(), path:format!("{root}/covers/16"),
            query:format!("offset=0&total={}", png.len()), headers:vec![], body:png.clone() }).await?.status, 200);
        assert!(rendered_text(accessor).contains("Book 24"), "cover updates preserve filtered page");
        for i in 0..25 {
            if i != 16 {
                assert_eq!(http.call_handle(accessor, 1, callback("skip", serde_json::json!({"id":format!("id{i}")}))).await?.status, 200);
            }
        }
        assert_eq!(http.call_handle(accessor, 1, callback("complete", serde_json::json!({"skipped":24}))).await?.status, 200);
        assert_eq!(rendered_delete(accessor, "delete_comic_").len(), 5, "only the filtered page is rendered");
        assert!(rendered_text(accessor).contains("24 张封面缺失或跳过"));
        events.call_on_ui_event(accessor, "comic_search".into(), Event::Change, r#"{"value":"absent"}"#.into()).await?;
        assert!(rendered_text(accessor).contains("没有匹配的漫画"));
        assert!(rendered_delete(accessor, "delete_comic_").is_empty());
        events.call_on_ui_event(accessor, "comic_search_clear".into(), Event::Click, "{}".into()).await?;
        assert_eq!(rendered_delete(accessor, "delete_comic_").len(), 8);
        events.call_on_ui_event(accessor, "comic_search".into(), Event::Change, r#"{"value":"Book 24"}"#.into()).await?;
        let delete_event = rendered_delete(accessor, "delete_comic_")[0].clone();
        events.call_on_ui_event(accessor, delete_event.clone(), Event::Click, "{}".into()).await?;
        complete_startup(&world, accessor).await?;
        let deletion: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(deletion["type"], "delete_comic");
        assert_eq!(deletion["name"], "Book 24", "filtered index must retain the original row identity");
        assert!(rendered_text(accessor).contains("Book 24") && rendered_text(accessor).contains("重新读取确认设备结果"));
        let last_time = rendered_text(accessor).lines().find(|s| s.starts_with("最近完整同步：")).unwrap().to_string();
        accessor.with(|mut access| {
            let ctx = access.get(); ctx.device_addr = "AA:BB:CC:DD:EE:FF".into(); ctx.device_name = "Device B".into();
        });
        events.call_on_event(accessor, EventType::DeviceAction, "{}".into()).await?;
        let sent = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, delete_event.clone(), Event::Click, "{}".into()).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(), sent, "another device cannot receive deletion for A's list");
        accessor.with(|mut access| access.get().app_version = 317);
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        assert!(rendered_text(accessor).contains("Book 24") && rendered_text(accessor).contains("仍显示上次列表"));
        assert!(rendered_text(accessor).contains(&last_time), "failed refresh must retain A's full-sync time");
        accessor.with(|mut access| access.get().app_version = 382);
        events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
        complete_startup_with_caps(&world, accessor, serde_json::json!({"httpDataSync":1, "syncSession":true})).await?;
        let message: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        let root_b = format!("/control/sync/{}", message["session"].as_str().unwrap());
        for value in [serde_json::json!({"kind":"header", "comicCount":2, "sourceCount":0}),
            serde_json::json!({"kind":"comics", "offset":0, "items":[{"id":"B1", "name":"New B", "page_count":1, "chapters":0}]})] {
            assert_eq!(http.call_handle(accessor, 1, Request { method:"POST".into(), path:format!("{root_b}/metadata"),
                query:String::new(), headers:vec![], body:value.to_string().into_bytes() }).await?.status, 200);
        }
        assert!(rendered_text(accessor).contains("Device B") && rendered_text(accessor).contains("本设备尚无完整同步记录"));
        accessor.with(|mut access| access.get().connected = false);
        events.call_on_event(accessor, EventType::DeviceAction, "{}".into()).await?;
        assert!(rendered_text(accessor).contains("当前仅有部分数据"));
        assert_eq!(http.call_handle(accessor, 1, Request { method:"POST".into(), path:format!("{root_b}/complete"),
            query:String::new(), headers:vec![], body:b"{}".to_vec() }).await?.status, 409);
        accessor.with(|mut access| access.get().connected = true);
        events.call_on_ui_event(accessor, "comic_search_clear".into(), Event::Click, "{}".into()).await?;
        for complete_lists in [false, true] {
            events.call_on_ui_event(accessor, "fetch_app_data".into(), Event::Click, "{}".into()).await?;
            complete_startup_with_caps(&world, accessor, serde_json::json!({"syncSession":true})).await?;
            let message: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
            assert!(message.get("http").is_none());
            // A metadata row deliberately arrives before the header. Done arrives before the final row.
            let mut frames = vec![serde_json::json!({"type":"app_data_comic", "index":1,
                "comic":{"id":"L2", "name":"Legacy Two", "page_count":2, "chapters":0}}),
                serde_json::json!({"type":"app_data_header", "comic_count":2, "source_count":0}),
                serde_json::json!({"type":"app_data_done"})];
            if complete_lists { frames.push(serde_json::json!({"type":"app_data_comic", "index":0,
                "comic":{"id":"L1", "name":"Legacy One", "page_count":1, "chapters":0}})); }
            frames.push(serde_json::json!({"type":"cover_done"}));
            for (gseq, mut frame) in frames.into_iter().enumerate() {
                frame["session"] = message["session"].clone(); frame["gseq"] = serde_json::json!(gseq);
                events.call_on_event(accessor, EventType::InterconnectMessage, frame.to_string()).await?;
            }
            let text = rendered_text(accessor);
            assert!(text.contains("Legacy Two"));
            if complete_lists { assert!(text.contains("Legacy One") && text.contains("2 张封面未回传"));
                assert!(!text.contains("本设备尚无完整同步记录")); }
            else {
                assert!(text.contains("同步未完整") && text.contains("本设备尚无完整同步记录"));
                assert!(rendered_delete(accessor, "delete_comic_").is_empty(), "partial lists cannot offer name-based deletion");
            }
        }
        let sent = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, delete_event, Event::Click, "{}".into()).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(), sent, "stale button from A cannot act on B's snapshot");

        // P1-43: exercise the release guest's actual dialog/handshake/send/result and timers.
        sync_delete_fixture(&world, accessor).await?;
        let original_time = rendered_text(accessor).lines().find(|s| s.starts_with("最近完整同步：")).unwrap().to_string();
        let second = rendered_delete(accessor, "delete_comic_")[1].clone();
        events.call_on_ui_event(accessor, second.clone(), Event::Click, "{}".into()).await?;
        complete_startup_with_caps(&world, accessor, serde_json::json!({"deleteProtocol":1})).await?;
        let deletion: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(deletion["type"], "delete_item"); assert_eq!(deletion["comicId"], "same_two");
        assert!(rendered_text(accessor).contains("等待设备删除结果") && rendered_text(accessor).contains("ID：same_two"));
        events.call_on_event(accessor, EventType::InterconnectMessage, delete_result(&deletion,"partial").to_string()).await?;
        assert!(rendered_text(accessor).contains("设备部分操作完成") && rendered_text(accessor).contains("ID：same_two"));
        events.call_on_ui_event(accessor, second.clone(), Event::Click, "{}".into()).await?;
        complete_startup_with_caps(&world, accessor, serde_json::json!({"deleteProtocol":1})).await?;
        let retry: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_ne!(retry["requestId"], deletion["requestId"]);
        let result = delete_result(&retry,"success");
        events.call_on_event(accessor, EventType::InterconnectMessage, result.to_string()).await?;
        events.call_on_event(accessor, EventType::InterconnectMessage, result.to_string()).await?;
        assert!(!rendered_text(accessor).contains("ID：same_two") && rendered_text(accessor).contains("ID：same_one"));
        assert!(rendered_text(accessor).contains("漫画 2/2 本") && rendered_text(accessor).contains(&original_time));
        let sent = sent_interconnect.lock().unwrap().len();
        events.call_on_ui_event(accessor, second, Event::Click, "{}".into()).await?;
        assert_eq!(sent_interconnect.lock().unwrap().len(),sent,"success invalidates old index-based buttons");

        let source = rendered_delete(accessor,"delete_source_")[1].clone();
        events.call_on_ui_event(accessor,source,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"deleteProtocol":1})).await?;
        let source_delete: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(source_delete["sourceKey"],"source_b");
        let mut wrong = delete_result(&source_delete,"success"); wrong["sourceKey"] = serde_json::json!("source_a");
        events.call_on_event(accessor,EventType::InterconnectMessage,wrong.to_string()).await?;
        assert!(rendered_text(accessor).contains("Key：source_b"));
        events.call_on_event(accessor,EventType::InterconnectMessage,delete_result(&source_delete,"success").to_string()).await?;
        assert!(!rendered_text(accessor).contains("Key：source_b") && rendered_text(accessor).contains("Key：source_a"));

        let first = rendered_delete(accessor,"delete_comic_")[0].clone();
        events.call_on_ui_event(accessor,first,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"deleteProtocol":1})).await?;
        let lost: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        let timeout = accessor.with(|mut access| access.get().timers.iter().rev().find(|(_,p)|
            p.starts_with(&format!("delete_result_timeout:{}:",lost["requestId"].as_str().unwrap()))).unwrap().1.clone());
        events.call_on_event(accessor,EventType::Timer,serde_json::json!({"payload":timeout}).to_string()).await?;
        assert!(rendered_text(accessor).contains("等待设备删除结果"),"early timer cannot expire the real deadline");
        tokio::time::sleep(std::time::Duration::from_millis(20_050)).await;
        events.call_on_event(accessor,EventType::Timer,serde_json::json!({"payload":timeout}).to_string()).await?;
        assert!(rendered_text(accessor).contains("删除结果待核实") && rendered_text(accessor).contains("ID：same_one"));
        let query = rendered_delete(accessor,"delete_query_")[0].clone();
        events.call_on_ui_event(accessor,query,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"deleteProtocol":1})).await?;
        let query_message: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(query_message["type"],"delete_status"); assert_eq!(query_message["requestSession"],lost["session"]);
        assert_ne!(query_message["session"],lost["session"]);
        events.call_on_event(accessor,EventType::Timer,serde_json::json!({"payload":timeout}).to_string()).await?;
        assert!(rendered_text(accessor).contains("查询原删除结果"),"stale timeout cannot terminate new query");
        events.call_on_event(accessor,EventType::InterconnectMessage,serde_json::json!({"deviceAddr":"wrong-device",
            "payloadText":delete_result(&lost,"success")}).to_string()).await?;
        assert!(rendered_text(accessor).contains("ID：same_one"));
        events.call_on_event(accessor,EventType::InterconnectMessage,delete_result(&lost,"success").to_string()).await?;
        assert!(!rendered_text(accessor).contains("ID：same_one"));

        let other = rendered_delete(accessor,"delete_comic_")[0].clone();
        events.call_on_ui_event(accessor,other,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"deleteProtocol":1})).await?;
        let delayed: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        sync_delete_fixture(&world,accessor).await?; // refresh while waiting, same ID in a new snapshot
        events.call_on_event(accessor,EventType::InterconnectMessage,delete_result(&delayed,"success").to_string()).await?;
        assert!(rendered_text(accessor).contains("ID：other") && rendered_text(accessor).contains("当前列表未直接调整"));

        let first = rendered_delete(accessor,"delete_comic_")[0].clone();
        events.call_on_ui_event(accessor,first,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"deleteProtocol":1})).await?;
        accessor.with(|mut access| access.get().connected=false);
        events.call_on_event(accessor,EventType::DeviceAction,"{}".into()).await?;
        assert!(rendered_text(accessor).contains("删除结果待核实") && rendered_text(accessor).contains("ID：same_one"));
        assert!(!rendered_text(accessor).contains("设备已确认删除"));
        accessor.with(|mut access| access.get().connected=true);
        sync_delete_fixture(&world,accessor).await?;
        assert!(rendered_text(accessor).contains("重新读取确认条目仍在"));
        let first = rendered_delete(accessor,"delete_comic_")[0].clone();
        events.call_on_ui_event(accessor,first,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({})).await?;
        assert!(rendered_text(accessor).contains("旧端删除协议按名称定位"));
        let last: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(last["type"],"hs_ping","legacy same-name deletion must not send a delete command");

        // No native HTTP capability (10 Pro-style path): keys must survive the QAIC list as well.
        events.call_on_ui_event(accessor,"fetch_app_data".into(),Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"syncSession":true,"deleteProtocol":1})).await?;
        let qaic_request: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert!(qaic_request.get("http").is_none());
        for (gseq, mut frame) in [serde_json::json!({"type":"app_data_header","comic_count":0,"source_count":2}),
            serde_json::json!({"type":"app_data_source","index":0,"source":{"key":"qaic_a","name":"同名源","apiUrl":"http://a"}}),
            serde_json::json!({"type":"app_data_source","index":1,"source":{"key":"qaic_b","name":"同名源","apiUrl":"http://b"}}),
            serde_json::json!({"type":"app_data_done"}),serde_json::json!({"type":"cover_done"})].into_iter().enumerate() {
            frame["session"] = qaic_request["session"].clone(); frame["gseq"] = serde_json::json!(gseq);
            events.call_on_event(accessor,EventType::InterconnectMessage,frame.to_string()).await?;
        }
        assert!(rendered_text(accessor).contains("Key：qaic_b"));
        let second = rendered_delete(accessor,"delete_source_")[1].clone();
        events.call_on_ui_event(accessor,second,Event::Click,"{}".into()).await?;
        complete_startup_with_caps(&world,accessor,serde_json::json!({"deleteProtocol":1})).await?;
        let deletion: serde_json::Value = serde_json::from_str(sent_interconnect.lock().unwrap().last().unwrap()).unwrap();
        assert_eq!(deletion["sourceKey"],"qaic_b");
        events.call_on_event(accessor,EventType::InterconnectMessage,delete_result(&deletion,"success").to_string()).await?;
        assert!(!rendered_text(accessor).contains("Key：qaic_b") && rendered_text(accessor).contains("Key：qaic_a"));

        check_sources(&world,accessor).await?;

        let masters: Vec<_> = std::fs::read_dir(directory.path().join("cache/masters"))?.collect::<Result<_, _>>()?;
        assert_eq!(masters.len(),5,"WASI disk masters must not overwrite one another");
        for expected in &image_bodies[1..] {
            assert!(masters.iter().any(|entry|std::fs::read(entry.path()).unwrap() == *expected),"canonical PNG masters must match the full-size PNG products");
        }
        check_images(&world,accessor,directory.path()).await?;
        check_import_result_protocol(&world,accessor).await?;

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
    if import_results_only {
        println!("PASS: P1-46 actual release WASM importResultProtocol negotiation, header sessionId, saving waiting state, result settlement, 25s timeout, partial/index error reporting and unknown query.");
        return Ok(());
    }
    if images_only {
        println!("PASS: P1-45 actual release WASM HTTP/legacy byte equality, JPEG/PNG/LVGL priority, transparent/long images, cover width, single/multi/chapter uploads, stop-and-wait/window chunks, cache hits/corruption, bounded picks and missing/corrupt master errors.");
        return Ok(());
    }
    if sources_only {
        println!("PASS: P1-44 actual release WASM input/Cookie typing without root replacement, explicit reads with no change/blur fetch, single-fetch catalogs, selection/Cookie snapshots, stale events, device changes and read/send errors.");
        return Ok(());
    }
    assert_eq!(std::fs::read_to_string(directory.path().join("http-address.txt"))?, "192.168.1.100");
    println!("PASS: P1-46 legacy import result protocol / timeouts / errors; P1-45 HTTP/legacy byte equality, formats/transparency/long images, all upload entrances, caches and image errors; P1-44 source catalogs/Cookies; P1-43 deletion/results/timeouts; plus library completeness, version gates, HTTP import/binding, disk masters and start/stop on the actual release WASM.");
    Ok(())
}
