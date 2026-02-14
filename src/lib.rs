use wit_bindgen::FutureReader;

use crate::exports::astrobox::psys_plugin::{
    event::{self, EventType},
    lifecycle,
};

pub mod logger;
pub mod ui;
pub mod network;

wit_bindgen::generate!({
    path: "wit",
    world: "psys-world",
    generate_all,
});

struct MyPlugin;

impl lifecycle::Guest for MyPlugin {
    fn on_load() {
        logger::init();
        tracing::info!("bandcomic Helper 插件已加载...");
        tracing::info!("UI 已初始渲染");
    }
}

impl event::Guest for MyPlugin {
    #[allow(async_fn_in_trait)]
    fn on_event(event_type: EventType, event_payload: _rt::String) -> FutureReader<String> {
        let (writer, reader) = wit_future::new::<String>(|| "".to_string());

        match event_type {
            EventType::PluginMessage => {
                tracing::info!("收到插件消息: {}", event_payload);
            }
            EventType::InterconnectMessage => {
                ui::handle_interconnect_message(&event_payload);
            }
            EventType::DeviceAction => {}
            EventType::ProviderAction => {}
            EventType::DeeplinkAction => {}
            EventType::TransportPacket => {}
            EventType::Timer => {
                // 处理定时器事件
                if event_payload == ui::state::HIDE_STATUS_EVENT {
                    ui::hide_status();
                }
            }
        };

        wit_bindgen::spawn(async move {
            let _ = writer.write("".to_string()).await;
        });

        reader
    }

    fn on_ui_event(
        event_id: _rt::String,
        event_type: event::Event,
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

    fn on_card_render(_card_id: _rt::String) -> wit_bindgen::rt::async_support::FutureReader<()> {
        let (writer, reader) = wit_future::new::<()>(|| ());

        // 腕上漫画插件不需要卡片渲染
        tracing::info!("卡片渲染请求被忽略");

        wit_bindgen::spawn(async move {
            let _ = writer.write(()).await;
        });

        reader
    }
}

export!(MyPlugin);
