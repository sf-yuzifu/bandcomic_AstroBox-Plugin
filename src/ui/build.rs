use crate::astrobox::psys_host;
use crate::astrobox::psys_host::ui_v3 as ui;
use super::state::*;
use super::message::get_status_text;

const INPUT_HEIGHT: u32 = 40;

pub fn render_main_ui(element_id: &str) {
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.root_element_id = Some(element_id.to_string());
    }
    let ui_tree = build_main_ui();
    psys_host::ui_v3::render(element_id, ui_tree);
}

pub fn build_main_ui() -> ui::Element {
    let state = ui_state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let container = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .padding(20);

    let tabs = build_tabs(&state);
    let content = match state.current_tab {
        TabPage::Sync => build_sync_ui(&state),
        TabPage::Data => build_data_ui(&state),
    };

    container.child(tabs).child(content)
}

pub fn rerender_main_ui() {
    let element_id = {
        let state = ui_state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.root_element_id.clone()
    };

    if let Some(element_id) = element_id {
        let ui_tree = build_main_ui();
        psys_host::ui_v3::render(&element_id, ui_tree);
    }
}

fn build_tabs(state: &UiState) -> ui::Element {
    let tabs_root = ui::Element::new(ui::ElementType::TabsRoot, None)
        .flex()
        .justify_center()
        .margin_bottom(20);

    let tabs_list = ui::Element::new(ui::ElementType::TabsList, None)
        .flex()
        .bg("#1E1E1F")
        .radius(999)
        .padding(4)
        .gap(4);

    let sync_trigger = build_tab_trigger(
        "漫画源同步",
        icon_sync_svg(),
        state.current_tab == TabPage::Sync,
        TAB_SYNC_EVENT,
    );

    let data_trigger = build_tab_trigger(
        "数据浏览",
        icon_data_svg(),
        state.current_tab == TabPage::Data,
        TAB_DATA_EVENT,
    );

    tabs_root.child(tabs_list.child(sync_trigger).child(data_trigger))
}

fn build_tab_trigger(label: &str, icon_svg: String, is_active: bool, event_id: &str) -> ui::Element {
    let icon = ui::Element::new(ui::ElementType::Svg, Some(&icon_svg))
        .width(22)
        .height(22);

    let text = ui::Element::new(ui::ElementType::Span, Some(label)).size(14);

    ui::Element::new(ui::ElementType::TabsTrigger, None)
        .without_default_styles()
        .on(ui::Event::Click, event_id)
        .radius(999)
        .padding_top(10)
        .padding_bottom(10)
        .padding_left(14)
        .padding_right(14)
        .bg(if is_active { "#2A2A2A" } else { "#1E1E1F" })
        .text_color(if is_active { "#FFFFFF" } else { "#BBBBBB" })
        .flex()
        .align_center()
        .gap(5)
        .child(icon)
        .child(text)
}

fn build_section_title(text: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::P, Some(text))
        .size(13)
        .text_color("#888888")
        .margin_left(12)
}

fn build_settings_card(
    icon_svg: String,
    title: &str,
    desc: Option<&str>,
    right: Option<ui::Element>,
    click_event: Option<&str>,
) -> ui::Element {
    let icon = ui::Element::new(ui::ElementType::Svg, Some(&icon_svg))
        .width(22)
        .height(22)
        .text_color("#FFFFFF");

    let icon_wrap = ui::Element::new(ui::ElementType::Div, None)
        .width(22)
        .height(22)
        .flex()
        .align_center()
        .justify_center()
        .child(icon);

    let mut text_col = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full();

    let title_el = ui::Element::new(ui::ElementType::P, Some(title)).size(15);
    text_col = text_col.child(title_el);

    if let Some(desc_text) = desc {
        let desc_el = ui::Element::new(ui::ElementType::P, Some(desc_text))
            .size(13)
            .text_color("#888888");
        text_col = text_col.child(desc_el);
    }

    let mut row = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .width_full()
        .bg("#1E1E1F")
        .radius(18)
        .padding_left(12)
        .padding_right(12)
        .padding_top(10)
        .padding_bottom(10)
        .gap(10)
        .child(icon_wrap)
        .child(text_col);

    if let Some(right_el) = right {
        let right_wrap = ui::Element::new(ui::ElementType::Div, None)
            .flex()
            .align_center()
            .justify_end()
            .child(right_el);
        row = row.child(right_wrap);
    }

    if let Some(event_id) = click_event {
        row = row.on(ui::Event::Click, event_id);
    }

    row
}

fn build_icon_text_button_full(label: &str, icon_svg: String, event_id: &str) -> ui::Element {
    let icon = ui::Element::new(ui::ElementType::Svg, Some(&icon_svg))
        .width(22)
        .height(22);

    let text = ui::Element::new(ui::ElementType::Span, Some(label)).size(14);

    ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .on(ui::Event::Click, event_id)
        .radius(18)
        .padding(14)
        .bg("#2A2A2A")
        .width_full()
        .flex()
        .align_center()
        .gap(8)
        .child(icon)
        .child(text)
}

fn build_comic_card(comic: &ComicInfo, index: usize) -> ui::Element {
    let has_cover = !comic.cover_base64.is_empty();

    let cover = if has_cover {
        ui::Element::new(ui::ElementType::Image, Some(&comic.cover_base64))
            .absolute()
            .left(10)
            .width(60)
            .height(88)
            .radius(8)
    } else {
        ui::Element::new(ui::ElementType::Div, None)
            .absolute()
            .left(10)
            .width(60)
            .height(88)
            .radius(8)
            .bg("#2A2A2A")
            .flex()
            .align_center()
            .justify_center()
            .child(
                ui::Element::new(ui::ElementType::Svg, Some(&icon_book_svg()))
                    .width(24)
                    .height(24)
                    .text_color("#555555")
            )
    };

    let detail = if comic.chapters > 0 {
        format!("{}话", comic.chapters)
    } else {
        format!("{}页", comic.page_count)
    };

    let name = ui::Element::new(ui::ElementType::P, Some(&comic.name))
        .size(15)
        .text_color("#DDDDDD");

    let meta = ui::Element::new(ui::ElementType::P, Some(&detail))
        .size(13)
        .text_color("#888888")
        .margin_top(4);

    let text_col = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .margin_left(80)
        .margin_right(48)
        .flex()
        .child(name)
        .child(meta);

    let delete_event_id = format!("{}{}", DELETE_COMIC_PREFIX, index);
    let delete_btn = ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .on(ui::Event::Click, &delete_event_id)
        .absolute()
        .right(10)
        .width(36)
        .height(36)
        .radius(18)
        .bg("#3D1515")
        .flex()
        .align_center()
        .justify_center()
        .child(
            ui::Element::new(ui::ElementType::Svg, Some(&icon_trash_svg()))
                .width(18)
                .height(18)
                .text_color("#FF5252")
        );

    ui::Element::new(ui::ElementType::Div, None)
        .relative()
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .bg("#1E1E1F")
        .radius(18)
        .padding(10)
        .gap(12)
        .min_height(108)
        .width_full()
        .child(text_col)
        .child(delete_btn)
        .child(cover)
}

fn build_source_card(source: &SourceInfo, display_url: &str, index: usize) -> ui::Element {
    let icon = ui::Element::new(ui::ElementType::Svg, Some(&icon_link_svg()))
        .width(22)
        .height(22)
        .text_color("#FFFFFF");

    let icon_wrap = ui::Element::new(ui::ElementType::Div, None)
        .width(22)
        .height(22)
        .flex()
        .align_center()
        .justify_center()
        .child(icon);

    let title_el = ui::Element::new(ui::ElementType::P, Some(&source.name)).size(15);
    let desc_el = ui::Element::new(ui::ElementType::P, Some(display_url))
        .size(13)
        .text_color("#888888");

    let text_col = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .child(title_el)
        .child(desc_el);

    let left_row = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .gap(10)
        .margin_right(48)
        .flex()
        .child(icon_wrap)
        .child(text_col);

    let delete_event_id = format!("{}{}", DELETE_SOURCE_PREFIX, index);
    let delete_btn = ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .on(ui::Event::Click, &delete_event_id)
        .absolute()
        .right(12)
        .width(36)
        .height(36)
        .radius(18)
        .bg("#3D1515")
        .flex()
        .align_center()
        .justify_center()
        .child(
            ui::Element::new(ui::ElementType::Svg, Some(&icon_trash_svg()))
                .width(18)
                .height(18)
                .text_color("#FF5252")
        );

    ui::Element::new(ui::ElementType::Div, None)
        .relative()
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .width_full()
        .bg("#1E1E1F")
        .radius(18)
        .padding_left(12)
        .padding_right(12)
        .padding_top(10)
        .padding_bottom(10)
        .child(left_row)
        .child(delete_btn)
}

fn build_sync_ui(state: &UiState) -> ui::Element {
    let source_title = build_section_title("漫画源配置");

    let domain_input = ui::Element::new(ui::ElementType::Input, Some(&state.config.domain))
        .on(ui::Event::Change, DOMAIN_INPUT_CHANGE_EVENT)
        .on(ui::Event::Blur, DOMAIN_INPUT_BLUR_EVENT)
        .radius(18)
        .bg("#2A2A2A")
        .height(INPUT_HEIGHT)
        .width_full()
        .padding_left(12)
        .padding_right(12)
        .margin_bottom(8);

    let source_name_value = state.fetched_source_name.as_deref()
        .or_else(|| if state.config.source_name.is_empty() { None } else { Some(&state.config.source_name) })
        .unwrap_or("");

    let source_name_card = build_settings_card(
        icon_globe_svg(),
        if source_name_value.is_empty() { "输入域名自动获取" } else { source_name_value },
        Some("漫画源名称"),
        None,
        None,
    );

    let cookie_title = build_section_title("Cookie 配置").margin_top(8);

    let cookie_input = ui::Element::new(ui::ElementType::Input, Some(&state.config.cookie))
        .on(ui::Event::Change, COOKIE_INPUT_EVENT)
        .radius(18)
        .bg("#2A2A2A")
        .height(INPUT_HEIGHT)
        .width_full()
        .padding_left(12)
        .padding_right(12)
        .margin_bottom(8);

    let (status_text, text_color) = get_status_text(&state.current_status);
    let status_bg = match &state.current_status {
        StatusState::Default => "#1E1E1F",
        StatusState::Processing(_) => "#0D2137",
        StatusState::Success(_) => "#0D2818",
        StatusState::Error(_) => "#2D1111",
    };

    let status_message = ui::Element::new(ui::ElementType::Div, None)
        .bg(status_bg)
        .radius(18)
        .padding(12)
        .margin_bottom(8)
        .width_full()
        .child(
            ui::Element::new(ui::ElementType::P, Some(&status_text))
                .text_color(&text_color)
                .size(13)
                .align_center()
                .width_full()
        );

    let sync_button = build_icon_text_button_full("同步到手表", icon_send_svg(), SYNC_BUTTON_EVENT)
        .bg("#0090FF26")
        .text_color("#0090FF");

    ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .gap(8)
        .child(source_title)
        .child(domain_input)
        .child(source_name_card)
        .child(cookie_title)
        .child(cookie_input)
        .child(status_message)
        .child(sync_button)
}

fn build_data_ui(state: &UiState) -> ui::Element {
    let fetch_button = build_icon_text_button_full("获取快应用数据", icon_download_svg(), FETCH_APP_DATA_EVENT)
        .bg("#0090FF26")
        .text_color("#0090FF")
        .margin_bottom(8);

    let data_status = if matches!(&state.app_data_status, StatusState::Default) {
        None
    } else {
        let (data_status_text, data_text_color) = get_status_text(&state.app_data_status);
        let data_status_bg = match &state.app_data_status {
            StatusState::Default => "#1E1E1F",
            StatusState::Processing(_) => "#0D2137",
            StatusState::Success(_) => "#0D2818",
            StatusState::Error(_) => "#2D1111",
        };

        Some(
            ui::Element::new(ui::ElementType::Div, None)
                .bg(data_status_bg)
                .radius(18)
                .padding(12)
                .margin_bottom(8)
                .width_full()
                .child(
                    ui::Element::new(ui::ElementType::P, Some(&data_status_text))
                        .text_color(&data_text_color)
                        .size(13)
                        .align_center()
                        .width_full()
                ),
        )
    };

    let comic_count = state.app_comic_count.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string());
    let source_count = state.app_source_count.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string());

    let summary_card = build_settings_card(
        icon_stats_svg(),
        &format!("{} 本漫画 / {} 个漫画源", comic_count, source_count),
        Some("快应用数据概览"),
        None,
        None,
    );

    let mut root = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .gap(8);

    root = root.child(fetch_button);
    if let Some(ds) = data_status {
        root = root.child(ds);
    }
    root = root.child(summary_card);

    if !state.app_comics.is_empty() {
        let comic_title = build_section_title(&format!("漫画列表（共 {} 本）", state.app_comics.len()));
        root = root.child(comic_title);

        let _total = state.app_comics.len().min(20);
        for (i, comic) in state.app_comics.iter().take(20).enumerate() {
            root = root.child(build_comic_card(comic, i));
        }

        if state.app_comics.len() > 20 {
            let more = ui::Element::new(ui::ElementType::P, Some(&format!(
                "... 还有 {} 本",
                state.app_comics.len() - 20
            )))
            .size(13)
            .text_color("#888888")
            .margin_left(12);
            root = root.child(more);
        }
    }

    if !state.app_sources.is_empty() {
        let source_title = build_section_title(&format!("漫画源列表（共 {} 个）", state.app_sources.len())).margin_top(8);
        root = root.child(source_title);

        let _total = state.app_sources.len().min(20);
        for (i, source) in state.app_sources.iter().take(20).enumerate() {
            let display_url = if source.api_url.len() > 32 {
                format!("{}...", &source.api_url[..32])
            } else {
                source.api_url.clone()
            };

            let row = build_source_card(source, &display_url, i);
            root = root.child(row);
        }

        if state.app_sources.len() > 20 {
            let more = ui::Element::new(ui::ElementType::P, Some(&format!(
                "... 还有 {} 个",
                state.app_sources.len() - 20
            )))
            .size(13)
            .text_color("#888888")
            .margin_left(12);
            root = root.child(more);
        }
    }

    root
}

fn icon_sync_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2"/></svg>"#.to_string()
}

fn icon_data_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>"#.to_string()
}

fn icon_globe_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>"#.to_string()
}

fn icon_send_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 22 11 13 2 9 22 2"/></svg>"#.to_string()
}

fn icon_download_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>"#.to_string()
}

fn icon_stats_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>"#.to_string()
}

fn icon_book_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></svg>"#.to_string()
}

fn icon_link_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>"#.to_string()
}

fn icon_trash_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>"#.to_string()
}