use crate::exports::astrobox::psys_plugin_v4::{
    event,
    http,
    lifecycle,
};

pub mod logger;
pub mod ui;
pub mod network;
pub mod lvgl;
pub mod transfer;
pub mod sync_receive;
pub mod http_data_sync;
pub mod http_server;
pub mod http_probe;
pub mod local_source;
pub mod assets;
pub mod jobs;

wit_bindgen::generate!({
    path: "wit",
    world: "psys-world-v4-http",
    generate_all,
});

struct MyPlugin;

// Legacy handshake/import state machines process one business event at a time.
// An async mutex yields to the host; render and HTTP callbacks stay independent
// so dialogs can trigger re-rendering while a UI event awaits their result.
static BUSINESS_EVENTS: std::sync::LazyLock<futures::lock::Mutex<()>> =
    std::sync::LazyLock::new(|| futures::lock::Mutex::new(()));

impl lifecycle::Guest for MyPlugin {
    async fn on_load() {
        logger::init();
        tracing::info!("bandcomic Helper 插件已加载...");
        tracing::info!("UI 已初始渲染");

            let result = crate::astrobox::psys_host_v4::register::register_card(
                crate::astrobox::psys_host_v4::register::CardType::Element,
                 ui::COMIC_DATA_CARD_ID.to_string(),
                 ui::COMIC_DATA_CARD_NAME.to_string(),
            )
            .await;

            match result {
                Ok(()) => tracing::info!("漫画数据卡片已注册"),
                Err(reason) => tracing::warn!("漫画数据卡片注册失败: {}", reason),
            }

        // 互联接收在用户操作启动快应用并等待 3 秒后由握手模块注册。
        http_server::start().await;
    }
}

impl event::Guest for MyPlugin {
    async fn on_event(event_type: event::EventType, event_payload: String) -> String {
        let _guard = BUSINESS_EVENTS.lock().await;

        match event_type {
            event::EventType::PluginMessage => {
                tracing::info!("收到插件消息: {}", event_payload);
            }
            event::EventType::InterconnectMessage => {
                ui::handle_interconnect_message(&event_payload).await;
            }
            event::EventType::Timer => {
                // 宿主把定时器 payload 包在 JSON 信封里送达：
                // {"kind":"timeout","payload":"...","timerId":N}
                // 解开信封拿到注册时的 payload 字符串再分发
                let timer_payload = serde_json::from_str::<serde_json::Value>(&event_payload)
                    .ok()
                    .and_then(|v| {
                        v.get("payload")
                            .and_then(|p| p.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| event_payload.to_string());

                if let Some(session) = timer_payload.strip_prefix("http_bind_timeout:") {
                    if http_server::expire_binding(session).await {
                        ui::event_handler::http_bind_timeout().await;
                    }
                } else if timer_payload == ui::state::HIDE_STATUS_EVENT {
                    ui::hide_status();
                } else if timer_payload == ui::state::HIDE_APP_DATA_STATUS_EVENT {
                    ui::hide_app_data_status();
                } else if timer_payload == ui::state::HIDE_UPLOAD_STATUS_EVENT {
                    ui::event_handler::hide_upload_status();
                } else if timer_payload == ui::state::UPLOAD_ACK_TIMEOUT_EVENT {
                    ui::event_handler::handle_upload_ack_timeout().await;
                } else if timer_payload == ui::state::UPLOAD_HEADER_TIMEOUT_EVENT {
                    ui::event_handler::handle_upload_header_timeout().await;
                } else if let Some(generation) = timer_payload
                    .strip_prefix(ui::state::APP_DATA_RECV_TIMEOUT_EVENT)
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    ui::event_handler::handle_app_data_recv_timeout(generation).await;
                } else if timer_payload.starts_with(&format!("{}:", ui::state::HS_REGISTER_RETRY_EVENT))
                    || timer_payload.starts_with(&format!("{}:", ui::state::HS_PING_EVENT))
                {
                    ui::handshake::on_timer(&timer_payload).await;
                } else if timer_payload == ui::state::PICK_PROCESS_EVENT {
                    ui::event_handler::handle_pick_process();
                } else {
                    tracing::warn!("未知 Timer 事件 payload: {}", timer_payload);
                }
            }
            _ => {}
        };

        String::new()
    }

    async fn on_ui_event(
        event_id: String,
        event_type: crate::astrobox::psys_host_v4::ui::Event,
        event_payload: String,
    ) -> String {
        let _guard = BUSINESS_EVENTS.lock().await;
        ui::ui_event_processor(event_type, &event_id, &event_payload).await;
        String::new()
    }

    async fn on_ui_render(element_id: String) {
        ui::render_main_ui(&element_id);
    }

    async fn on_card_render(card_id: String) {
        tracing::info!("on_card_render called: {}", card_id);
        ui::render_card(&card_id);

    }
}

impl http::Guest for MyPlugin {
    async fn handle(server_id: u32, request: crate::astrobox::psys_host_v4::http_server::Request)
        -> crate::astrobox::psys_host_v4::http_server::Response
    {
        http_server::handle(server_id, request)
    }
}

export!(MyPlugin);
