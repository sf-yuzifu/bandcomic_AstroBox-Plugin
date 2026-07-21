pub mod state;
pub mod message;
pub mod build;
pub mod event_handler;
pub mod handshake;

use tracing;

pub use build::render_main_ui;
pub use build::render_comic_data_card;
pub use event_handler::ui_event_processor;
pub use event_handler::handle_interconnect_message;
pub use event_handler::hide_app_data_status;
pub use event_handler::hide_upload_status;
pub use message::hide_status;

pub const COMIC_DATA_CARD_ID: &str = "band-comic-data";
pub const COMIC_DATA_CARD_NAME: &str = "腕上漫画 · 数据";

pub fn render_card(card_id: &str) {
    if card_id == COMIC_DATA_CARD_ID {
        build::render_comic_data_card(card_id);
    } else {
        tracing::info!("未知卡片ID: {}", card_id);
    }
}
