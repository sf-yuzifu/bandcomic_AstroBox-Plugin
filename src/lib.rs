use wit_bindgen::FutureReader;

use crate::exports::astrobox::psys_plugin::{
    event_v3,
    lifecycle,
};

pub mod logger;
pub mod ui;
pub mod network;
pub mod lvgl;

wit_bindgen::generate!({
    path: "wit",
    world: "psys-world-v3",
    generate_all,
});

struct MyPlugin;

impl lifecycle::Guest for MyPlugin {
    fn on_load() {
        logger::init();
        tracing::info!("bandcomic Helper 插件已加载...");
        tracing::info!("UI 已初始渲染");

        wit_bindgen::block_on(async move {
            let _ = crate::astrobox::psys_host::register::register_card(
                crate::astrobox::psys_host::register::CardType::Element,
                ui::COMIC_DATA_CARD_ID,
                ui::COMIC_DATA_CARD_NAME,
            )
            .await;

            tracing::info!("漫画数据卡片已注册");
        });
    }
}

impl event_v3::Guest for MyPlugin {
    #[allow(async_fn_in_trait)]
    fn on_event(event_type: event_v3::EventType, event_payload: _rt::String) -> FutureReader<String> {
        let (writer, reader) = wit_future::new::<String>(|| "".to_string());

        match event_type {
            event_v3::EventType::PluginMessage => {
                tracing::info!("收到插件消息: {}", event_payload);
            }
            event_v3::EventType::InterconnectMessage => {
                ui::handle_interconnect_message(&event_payload);
            }
            event_v3::EventType::Timer => {
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

                if timer_payload == ui::state::HIDE_STATUS_EVENT {
                    ui::hide_status();
                } else if timer_payload == ui::state::HIDE_APP_DATA_STATUS_EVENT {
                    ui::hide_app_data_status();
                } else if timer_payload == ui::state::HIDE_UPLOAD_STATUS_EVENT {
                    ui::event_handler::hide_upload_status();
                } else if timer_payload == ui::state::UPLOAD_ACK_TIMEOUT_EVENT {
                    ui::event_handler::handle_upload_ack_timeout();
                } else if timer_payload == ui::state::UPLOAD_HEADER_TIMEOUT_EVENT {
                    ui::event_handler::handle_upload_header_timeout();
                } else if timer_payload == ui::state::APP_DATA_RECV_TIMEOUT_EVENT {
                    ui::event_handler::handle_app_data_recv_timeout();
                } else if timer_payload == ui::state::HS_REGISTER_RETRY_EVENT
                    || timer_payload == ui::state::HS_PING_EVENT
                {
                    ui::handshake::on_timer(&timer_payload);
                } else {
                    tracing::warn!("未知 Timer 事件 payload: {}", timer_payload);
                }
            }
            _ => {}
        };

        wit_bindgen::spawn(async move {
            let _ = writer.write("".to_string()).await;
        });

        reader
    }

    fn on_ui_event_v3(
        event_id: _rt::String,
        event_type: event_v3::Event,
        event_payload: _rt::String,
    ) -> wit_bindgen::rt::async_support::FutureReader<_rt::String> {
        let (writer, reader) = wit_future::new::<String>(|| "".to_string());

        ui::ui_event_processor(event_type, &event_id, &event_payload);

        wit_bindgen::spawn(async move {
            let _ = writer.write("".to_string()).await;
        });

        reader
    }

    fn on_ui_render(element_id: _rt::String) -> wit_bindgen::rt::async_support::FutureReader<()> {
        let (writer, reader) = wit_future::new::<()>(|| ());

        ui::render_main_ui(&element_id);

        wit_bindgen::spawn(async move {
            let _ = writer.write(()).await;
        });

        reader
    }

    fn on_card_render(card_id: _rt::String) -> wit_bindgen::rt::async_support::FutureReader<()> {
        let (writer, reader) = wit_future::new::<()>(|| ());

        tracing::info!("on_card_render called: {}", card_id);
        ui::render_card(&card_id);

        wit_bindgen::spawn(async move {
            let _ = writer.write(()).await;
        });

        reader
    }
}

export!(MyPlugin);