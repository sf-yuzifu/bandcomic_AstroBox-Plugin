wit_bindgen::generate!({
    path: "../../wit",
    world: "psys-world-v4-http",
    generate_all,
});

use astrobox::psys_host_v4::{http_server, os};
use exports::astrobox::psys_plugin_v4::{event, http, lifecycle};

struct Probe;

impl lifecycle::Guest for Probe {
    async fn on_load() {
        println!("V4 probe: platform={}", os::platform().await);
        println!("V4 probe: host={:?}", os::device_id().await);
        println!("V4 probe: server={:?}", http_server::start(http_server::ServerOptions {
            port: 0,
            bind_all_interfaces: true,
        }).await);
        // Also exercise the existing outgoing WASI p2 HTTP client. The runtime
        // check supplies a deterministic HTTP fixture at this address.
        let response = waki::Client::new().get("http://127.0.0.1:18761/config").send().unwrap();
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.body().unwrap(), b"{\"probe\":true}");
    }
}

impl event::Guest for Probe {
    async fn on_event(_: event::EventType, _: String) -> String { String::new() }
    async fn on_ui_event(_: String, _: astrobox::psys_host_v4::ui::Event, _: String) -> String { String::new() }
    async fn on_ui_render(_: String) {}
    async fn on_card_render(_: String) {}
}

impl http::Guest for Probe {
    async fn handle(_: u32, _: http_server::Request) -> http_server::Response {
        http_server::Response {
            status: 200,
            headers: vec![],
            body: b"{\"probe\":true}".to_vec(),
        }
    }
}

export!(Probe);
