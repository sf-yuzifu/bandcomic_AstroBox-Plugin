use wit_bindgen::FutureReader;

use crate::exports::astrobox::psys_plugin::{
    event,
    event_v3,
    lifecycle,
};

pub mod logger;
pub mod ui;
pub mod network;

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

impl event::Guest for MyPlugin {
    #[allow(async_fn_in_trait)]
    fn on_event(event_type: event::EventType, event_payload: _rt::String) -> FutureReader<String> {
        let (writer, reader) = wit_future::new::<String>(|| "".to_string());

        match event_type {
            event::EventType::Timer => {
                if event_payload == ui::state::HIDE_STATUS_EVENT {
                    ui::hide_status();
                } else if event_payload == ui::state::HIDE_APP_DATA_STATUS_EVENT {
                    ui::hide_app_data_status();
                }
            }
            _ => {}
        };

        wit_bindgen::spawn(async move {
            let _ = writer.write("".to_string()).await;
        });

        reader
    }

    fn on_ui_event(
        _event_id: _rt::String,
        _event_type: crate::astrobox::psys_host::ui::Event,
        _event_payload: _rt::String,
    ) -> wit_bindgen::rt::async_support::FutureReader<_rt::String> {
        let (writer, reader) = wit_future::new::<String>(|| "".to_string());

        wit_bindgen::spawn(async move {
            let _ = writer.write("".to_string()).await;
        });

        reader
    }

    fn on_ui_render(_element_id: _rt::String) -> wit_bindgen::rt::async_support::FutureReader<()> {
        let (writer, reader) = wit_future::new::<()>(|| ());

        wit_bindgen::spawn(async move {
            let _ = writer.write(()).await;
        });

        reader
    }

    fn on_card_render(card_id: _rt::String) -> wit_bindgen::rt::async_support::FutureReader<()> {
        let (writer, reader) = wit_future::new::<()>(|| ());

        tracing::info!("on_card_render(legacy) called: {}", card_id);
        ui::render_card(&card_id);

        wit_bindgen::spawn(async move {
            let _ = writer.write(()).await;
        });

        reader
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
                if event_payload == ui::state::HIDE_STATUS_EVENT {
                    ui::hide_status();
                } else if event_payload == ui::state::HIDE_APP_DATA_STATUS_EVENT {
                    ui::hide_app_data_status();
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