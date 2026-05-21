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
        TabPage::Upload => build_upload_ui(&state),
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

    let upload_trigger = build_tab_trigger(
        "上传本地漫画",
        icon_upload_svg(),
        state.current_tab == TabPage::Upload,
        TAB_UPLOAD_EVENT,
    );

    tabs_root.child(tabs_list.child(sync_trigger).child(data_trigger).child(upload_trigger))
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

fn build_upload_ui(state: &UiState) -> ui::Element {
    let is_single = state.upload_mode == UploadMode::Single;

    let comic_name_title = build_section_title("漫画名称");

    let name_input = ui::Element::new(ui::ElementType::Input, Some(&state.upload_comic_name_input))
        .on(ui::Event::Change, UPLOAD_NAME_INPUT_EVENT)
        .radius(18)
        .bg("#2A2A2A")
        .height(INPUT_HEIGHT)
        .width_full()
        .padding_left(12)
        .padding_right(12)
        .margin_bottom(8);

    let mode_title = build_section_title("上传模式");

    let single_btn = build_mode_button(
        "单篇上传",
        icon_file_svg(),
        is_single,
        UPLOAD_MODE_SINGLE_EVENT,
    );

    let multi_btn = build_mode_button(
        "多章节上传",
        icon_folder_svg(),
        !is_single,
        UPLOAD_MODE_MULTI_EVENT,
    );

    let mode_row = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .gap(8)
        .margin_bottom(8)
        .child(single_btn)
        .child(multi_btn);

    let mut root = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .gap(8)
        .child(comic_name_title)
        .child(name_input)
        .child(mode_title)
        .child(mode_row);

    if is_single {
        root = root.child(build_single_mode_ui(state));
    } else {
        root = root.child(build_multi_mode_ui(state));
    }

    root
}

fn build_mode_button(label: &str, icon_svg: String, is_active: bool, event_id: &str) -> ui::Element {
    let icon = ui::Element::new(ui::ElementType::Svg, Some(&icon_svg))
        .width(18)
        .height(18);

    let text = ui::Element::new(ui::ElementType::Span, Some(label)).size(13);

    ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .on(ui::Event::Click, event_id)
        .radius(999)
        .padding_top(10)
        .padding_bottom(10)
        .padding_left(16)
        .padding_right(16)
        .bg(if is_active { "#0090FF26" } else { "#2A2A2A" })
        .text_color(if is_active { "#0090FF" } else { "#BBBBBB" })
        .flex()
        .align_center()
        .gap(5)
        .child(icon)
        .child(text)
}

fn build_single_mode_ui(state: &UiState) -> ui::Element {
    let mut root = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(6);

    let pick_files_button = build_icon_text_button_full(
        "选择漫画图片",
        icon_file_plus_svg(),
        UPLOAD_PICK_FILES_EVENT,
    )
    .bg("#2A2A2A")
    .margin_bottom(2);

    root = root.child(pick_files_button);

    if !state.upload_items.is_empty() {
        let total_pages: usize = state.upload_items.iter().map(|i| i.files.len()).sum();
        let _has_cover = state.upload_items.iter().any(|i| i.cover.is_some());

        // Cover area
        let cover_section_title = build_section_title("封面");
        root = root.child(cover_section_title);
        root = root.child(build_cover_edit_area(
            state.upload_items.first().and_then(|i| i.cover.as_ref()),
            UPLOAD_PICK_COVER_EVENT,
        ));

        // Page list
        if total_pages > 0 {
            let files_title = build_section_title(&format!("页面列表（共 {} 张）", total_pages));
            root = root.child(files_title);
            root = root.child(build_single_page_list(state));
        }

        let upload_status_area = build_upload_status_area(state);
        root = root.child(upload_status_area);

        let action_row = ui::Element::new(ui::ElementType::Div, None)
            .flex()
            .flex_direction(ui::FlexDirection::Row)
            .gap(8)
            .margin_top(4);

        let upload_btn = build_icon_text_button_full(
            "上传到手表",
            icon_send_svg(),
            UPLOAD_START_EVENT,
        )
        .bg("#0090FF26")
        .text_color("#0090FF");

        let clear_btn = build_icon_text_button_full(
            "清空列表",
            icon_trash_svg(),
            UPLOAD_CLEAR_EVENT,
        )
        .bg("#3D1515")
        .text_color("#FF5252");

        root = root.child(action_row.child(upload_btn).child(clear_btn));
    } else {
        let hint = ui::Element::new(ui::ElementType::P, Some(
            "选择图片作为漫画页，第一张自动设为封面",
        ))
        .size(12)
        .text_color("#666666")
        .margin_top(4)
        .margin_left(8);

        root = root.child(hint);
    }

    root
}

fn build_cover_edit_area(cover: Option<&UploadFile>, pick_event: &str) -> ui::Element {
    let mut area = ui::Element::new(ui::ElementType::Div, None)
        .bg("#1E1E1F")
        .radius(16)
        .border(1, "#2A2A2A")
        .padding(12)
        .width_full()
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .gap(12)
        .on(ui::Event::Click, pick_event);

    if let Some(cover_file) = cover {
        let mime = detect_mime(&cover_file.name);
        let data_url = format!("data:{};base64,{}", mime, base64_encode_str(&cover_file.thumbnail));

        let thumb = ui::Element::new(ui::ElementType::Image, Some(&data_url))
            .width(52)
            .height(72)
            .radius(8);

        area = area.child(thumb);

        let info = ui::Element::new(ui::ElementType::Div, None)
            .flex()
            .flex_direction(ui::FlexDirection::Column)
            .gap(2)
            .flex()
            .child(
                ui::Element::new(ui::ElementType::P, Some(&cover_file.name))
                    .size(13)
                    .text_color("#DDDDDD")
            )
            .child(
                ui::Element::new(ui::ElementType::P, Some(&format!("原图 {} · 压缩 {}", format_size(cover_file.original_size), format_size(cover_file.size))))
                    .size(10)
                    .text_color("#666666")
            );

        area = area.child(info);
    } else {
        let placeholder = ui::Element::new(ui::ElementType::Div, None)
            .width(52)
            .height(72)
            .radius(8)
            .bg("#2A2A2A")
            .flex()
            .align_center()
            .justify_center()
            .child(
                ui::Element::new(ui::ElementType::Svg, Some(&icon_plus_svg()))
                    .width(20)
                    .height(20)
                    .text_color("#555555")
            );

        area = area.child(placeholder)
            .child(
                ui::Element::new(ui::ElementType::P, Some("点击添加封面"))
                    .size(13)
                    .text_color("#666666")
            );
    }

    area
}

fn build_multi_mode_ui(state: &UiState) -> ui::Element {
    let mut root = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(8);

    let add_chapter_btn = build_icon_text_button_full(
        "添加章节",
        icon_plus_svg(),
        UPLOAD_ADD_CHAPTER_EVENT,
    )
    .bg("#2A2A2A");

    root = root.child(add_chapter_btn);

    if !state.upload_chapters.is_empty() {
        // Book-level cover area
        let cover_title = build_section_title("作品封面");
        root = root.child(cover_title);
        root = root.child(build_cover_edit_area(
            state.multi_cover.as_ref(),
            UPLOAD_PICK_MULTI_COVER_EVENT,
        ));

        let total_count = state.upload_chapters.len();
        let chapter_title = build_section_title(&format!("章节列表（共 {} 章）", total_count));
        root = root.child(chapter_title);

        for (ci, chapter) in state.upload_chapters.iter().enumerate() {
            root = root.child(build_chapter_card(state, chapter, ci));
        }

        let upload_status_area = build_upload_status_area(state);
        root = root.child(upload_status_area);

        let action_row = ui::Element::new(ui::ElementType::Div, None)
            .flex()
            .flex_direction(ui::FlexDirection::Row)
            .gap(8)
            .margin_top(4);

        let upload_btn = build_icon_text_button_full(
            "全部上传",
            icon_send_svg(),
            UPLOAD_START_EVENT,
        )
        .bg("#0090FF26")
        .text_color("#0090FF");

        let clear_btn = build_icon_text_button_full(
            "清空全部",
            icon_trash_svg(),
            UPLOAD_CLEAR_EVENT,
        )
        .bg("#3D1515")
        .text_color("#FF5252");

        root = root.child(action_row.child(upload_btn).child(clear_btn));
    }

    root
}

fn build_chapter_card(_state: &UiState, chapter: &ChapterItem, chapter_index: usize) -> ui::Element {
    let chapter_name = if chapter.name.is_empty() {
        "未命名章节".to_string()
    } else {
        chapter.name.clone()
    };

    let name_input = ui::Element::new(ui::ElementType::Input, Some(&chapter.name))
        .on(ui::Event::Change, &format!("{}{}", CHAPTER_NAME_INPUT_PREFIX, chapter_index))
        .radius(10)
        .bg("#2A2A2A")
        .height(34)
        .width_full()
        .padding_left(10);

    // Page file list
    let mut page_list_el = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(4);

    for (fi, file) in chapter.files.iter().enumerate() {
        let file_card = build_chapter_page_card(file, chapter_index, fi);
        page_list_el = page_list_el.child(file_card);
    }

    let file_count = chapter.files.len();
    let count_text = if file_count == 0 {
        "暂无图片".to_string()
    } else {
        let page_comp: usize = chapter.files.iter().fold(0, |acc, f| acc + f.size);
        format!("{} 张 · {}", file_count, format_size(page_comp))
    };

    let count_label = ui::Element::new(ui::ElementType::P, Some(&count_text))
        .size(11)
        .text_color("#777777")
        .margin_bottom(4);

    // Per-chapter button row
    let mut btn_row = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .gap(4);

    let pick_btn = build_small_btn("添加图片", &format!("{}{}", CHAPTER_PICK_FILES_PREFIX, chapter_index), "#666666");
    
    let upload_btn = build_small_btn("上传本章", &format!("{}{}", CHAPTER_UPLOAD_PREFIX, chapter_index), "#0090FF");

    let clear_btn = build_small_btn("清空", &format!("{}{}", CHAPTER_CLEAR_PREFIX, chapter_index), "#FF5252");

    let del_chap_btn = build_small_btn("删除章节", &format!("{}{}", CHAPTER_DELETE_PREFIX, chapter_index), "#FF0000");

    btn_row = btn_row.child(pick_btn).child(upload_btn).child(clear_btn).child(del_chap_btn);

    let mut card = ui::Element::new(ui::ElementType::Div, None)
        .bg("#1E1E1F")
        .radius(16)
        .border(1, "#2A2A2A")
        .padding(10)
        .width_full()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(6);

    // Chapter header
    let header = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .gap(6)
        .child(
            ui::Element::new(ui::ElementType::P, Some(&chapter_name))
                .size(14)
                .text_color("#FFFFFF")
        )
        .child(
            ui::Element::new(ui::ElementType::Span, Some(&format!("第{}章", chapter_index + 1)))
                .size(10)
                .text_color("#777777")
                .bg("#2A2A2A")
                .padding_left(6)
                .padding_right(6)
                .padding_top(2)
                .padding_bottom(2)
                .radius(4)
        );

    card = card.child(header)
        .child(name_input)
        .child(count_label);

    if !chapter.files.is_empty() {
        let page_label = build_section_title(&format!("页面（{} 张）", file_count));
        card = card.child(page_label).child(page_list_el);
    }

    card.child(btn_row)
}

fn build_chapter_page_card(
    file: &UploadFile,
    chapter_index: usize,
    file_index: usize,
) -> ui::Element {
    let mime = detect_mime(&file.name);
    let data_url = format!("data:{};base64,{}", mime, base64_encode_str(&file.thumbnail));

    let thumbnail = ui::Element::new(ui::ElementType::Image, Some(&data_url))
        .width(44)
        .height(60)
        .radius(6);

    let name_el = ui::Element::new(ui::ElementType::P, Some(&file.name))
        .size(13)
        .text_color("#DDDDDD");

    let size_text = format!("原图 {} · 压缩 {}", format_size(file.original_size), format_size(file.size));
    let meta_el = ui::Element::new(ui::ElementType::P, Some(&size_text))
        .size(10)
        .text_color("#666666");

    let info_col = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(2)
        .flex()
        .child(name_el)
        .child(meta_el);

    let up_enabled = file_index > 0;
    let down_enabled = true; // Simplified - allow moving

    let up_btn = card_icon_button("▲", &format!("{}{}_{}", CHAPTER_MOVE_UP_PREFIX, chapter_index, file_index), up_enabled);
    let down_btn = card_icon_button("▼", &format!("{}{}_{}", CHAPTER_MOVE_DOWN_PREFIX, chapter_index, file_index), down_enabled);

    let del_btn = card_text_button("删除", &format!("{}{}_{}", CHAPTER_DEL_FILE_PREFIX, chapter_index, file_index), "#FF5252", "#2A1111");

    let actions = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .gap(3)
        .align_center()
        .child(up_btn)
        .child(down_btn)
        .child(del_btn);

    ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .gap(8)
        .bg("#262627")
        .radius(10)
        .padding(8)
        .child(thumbnail)
        .child(info_col)
        .child(actions)
}

fn build_small_btn(label: &str, event_id: &str, color: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .on(ui::Event::Click, event_id)
        .padding_top(5)
        .padding_bottom(5)
        .padding_left(10)
        .padding_right(10)
        .radius(8)
        .bg("#2A2A2A")
        .child(
            ui::Element::new(ui::ElementType::Span, Some(label))
                .size(11)
                .text_color(color)
        )
}

fn build_single_page_list(state: &UiState) -> ui::Element {
    let mut list = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(6);

    let mut global_index: usize = 0;
    for (_item_idx, item) in state.upload_items.iter().enumerate() {
        for (_file_idx, file) in item.files.iter().enumerate() {
            let idx = global_index;
            let card = build_single_page_card(file, idx, item.files.len());
            list = list.child(card);
            global_index += 1;
        }
    }

    list
}

fn build_single_page_card(
    file: &UploadFile,
    index: usize,
    _total_in_item: usize,
) -> ui::Element {
    let mime = detect_mime(&file.name);
    let data_url = format!("data:{};base64,{}", mime, base64_encode_str(&file.thumbnail));

    let thumbnail = ui::Element::new(ui::ElementType::Image, Some(&data_url))
        .width(44)
        .height(60)
        .radius(6);

    let name_el = ui::Element::new(ui::ElementType::P, Some(&file.name))
        .size(13)
        .text_color("#DDDDDD");

    let size_text = format!("原图 {} · 压缩 {}", format_size(file.original_size), format_size(file.size));
    let meta_el = ui::Element::new(ui::ElementType::P, Some(&size_text))
        .size(10)
        .text_color("#666666");

    let info_col = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(2)
        .flex()
        .child(name_el)
        .child(meta_el);

    let up_btn = card_icon_button("▲", &format!("{}{}", UPLOAD_MOVE_UP_PREFIX, index), index > 0);
    let del_btn = card_text_button("删除", &format!("{}{}", UPLOAD_DELETE_PREFIX, index), "#FF5252", "#2A1111");

    let actions = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .gap(4)
        .align_center()
        .child(up_btn)
        .child(del_btn);

    ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .align_center()
        .gap(8)
        .bg("#262627")
        .radius(10)
        .padding(8)
        .child(thumbnail)
        .child(info_col)
        .child(actions)
}

fn card_icon_button(label: &str, event_id: &str, enabled: bool) -> ui::Element {
    let mut btn = ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .width(24)
        .height(24)
        .radius(6)
        .flex()
        .align_center()
        .justify_center()
        .child(
            ui::Element::new(ui::ElementType::Span, Some(label))
                .size(10)
                .text_color(if enabled { "#999999" } else { "#444444" })
        );

    if enabled {
        btn = btn.on(ui::Event::Click, event_id).bg("#2A2A2A");
    } else {
        btn = btn.bg("#1A1A1B");
    }

    btn
}

fn card_text_button(label: &str, event_id: &str, text_color: &str, bg_color: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Button, None)
        .without_default_styles()
        .on(ui::Event::Click, event_id)
        .padding_top(3)
        .padding_bottom(3)
        .padding_left(8)
        .padding_right(8)
        .radius(6)
        .bg(bg_color)
        .flex()
        .align_center()
        .justify_center()
        .child(
            ui::Element::new(ui::ElementType::Span, Some(label))
                .size(10)
                .text_color(text_color)
        )
}

fn detect_mime(filename: &str) -> &str {
    let ext = filename.rsplit('.').next().unwrap_or("jpg").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

fn base64_encode_str(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    let len = data.len();

    for i in (0..len).step_by(3) {
        let b1 = data[i];
        let b2 = if i + 1 < len { data[i + 1] } else { 0 };
        let b3 = if i + 2 < len { data[i + 2] } else { 0 };

        result.push(CHARS[(b1 >> 2) as usize] as char);
        result.push(CHARS[((b1 & 3) << 4 | b2 >> 4) as usize] as char);
        result.push(if i + 1 < len { CHARS[((b2 & 15) << 2 | b3 >> 6) as usize] as char } else { '=' });
        result.push(if i + 2 < len { CHARS[(b3 & 63) as usize] as char } else { '=' });
    }

    result
}

fn build_upload_status_area(state: &UiState) -> ui::Element {
    if matches!(&state.upload_status, StatusState::Default) {
        return ui::Element::new(ui::ElementType::Div, None);
    }

    if matches!(&state.upload_status, StatusState::Processing(_)) {
        let (status_text, text_color) = get_status_text(&state.upload_status);
        let current_file = if state.upload_current_file.is_empty() {
            String::new()
        } else {
            format!("\n当前文件：{}", state.upload_current_file)
        };

        let progress_pct = (state.upload_progress * 100.0) as u32;

        let progress_bar = ui::Element::new(ui::ElementType::Progress, None)
            .prop("value", &progress_pct.to_string())
            .width_full()
            .height(6)
            .radius(3)
            .margin_top(6)
            .margin_bottom(4);

        let progress_text = ui::Element::new(ui::ElementType::P, Some(&format!(
            "{}%{}{}",
            progress_pct,
            if progress_pct < 100 { " · 上传中..." } else { "" },
            current_file
        )))
        .size(12)
        .text_color(&text_color)
        .margin_bottom(4);

        return ui::Element::new(ui::ElementType::Div, None)
            .bg("#0D2137")
            .radius(14)
            .padding(12)
            .width_full()
            .child(progress_text)
            .child(progress_bar)
            .child(
                ui::Element::new(ui::ElementType::P, Some(&status_text))
                    .size(11)
                    .text_color("#888888")
            );
    }

    let (status_text, text_color) = get_status_text(&state.upload_status);
    let status_bg = match &state.upload_status {
        StatusState::Default => "#1E1E1F",
        StatusState::Processing(_) => "#0D2137",
        StatusState::Success(_) => "#0D2818",
        StatusState::Error(_) => "#2D1111",
    };

    ui::Element::new(ui::ElementType::Div, None)
        .bg(status_bg)
        .radius(14)
        .padding(12)
        .width_full()
        .child(
            ui::Element::new(ui::ElementType::P, Some(&status_text))
                .text_color(&text_color)
                .size(13)
                .align_center()
                .width_full()
        )
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
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

fn icon_upload_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>"#.to_string()
}

fn icon_file_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><polyline points="13 2 13 9 20 9"/></svg>"#.to_string()
}

fn icon_folder_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>"#.to_string()
}

fn icon_file_plus_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="12" y1="18" x2="12" y2="12"/><line x1="9" y1="15" x2="15" y2="15"/></svg>"#.to_string()
}

fn icon_plus_svg() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>"#.to_string()
}

fn icon_logo_png_base64() -> String {
    "data:image/png;base64,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".to_string()
}

fn build_comic_data_card_ui() -> ui::Element {
    let state = ui_state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let comic_count = state.app_comic_count.map(|c| c.to_string()).unwrap_or_else(|| "--".to_string());
    let source_count = state.app_source_count.map(|c| c.to_string()).unwrap_or_else(|| "--".to_string());

    let title_text = ui::Element::new(ui::ElementType::P, Some("腕上漫画数据"))
        .size(14)
        .absolute()
        .top(12)
        .left(12)
        .text_color("rgba(255, 255, 255, 0.5)");

    let icon = ui::Element::new(ui::ElementType::Image, Some(&icon_logo_png_base64()))
        .width(34)
        .radius(999)
        .height(34);

    let icon_btn = ui::Element::new(ui::ElementType::Div, None)
        .width(34)
        .height(34)
        .radius(999)
        .absolute()
        .top(12)
        .right(12)
        .bg("rgba(71, 71, 75, 0.3)")
        .flex()
        .align_center()
        .justify_center()
        .child(icon);

    let title_text_wrap = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .child(title_text);

    let title_row = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .justify_center()
        .align_center()
        .width_full()
        .child(title_text_wrap)
        .child(icon_btn);

    let stat_col = |number: &str, label: &str| -> ui::Element {
        let num = ui::Element::new(ui::ElementType::Span, Some(number))
            .size(35)
            .text_color("#FFFFFF");
        let lbl = ui::Element::new(ui::ElementType::Span, Some(label))
            .size(14)
            .text_color("rgba(255, 255, 255, 0.5)");
        ui::Element::new(ui::ElementType::Div, None)
            .flex()
            .flex_direction(ui::FlexDirection::Column)
            .justify_center()
            .width_half()
            .align_center()
            .flex()
            .child(num)
            .child(lbl)
    };

    let comic_stat = stat_col(&comic_count, "漫画数量");
    let source_stat = stat_col(&source_count, "漫画源数量");

    let stats_row = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Row)
        .justify_center()
        .align_center()
        .width_full()
        .absolute()
        .left(0)
        .bottom(12)
        .child(comic_stat)
        .child(source_stat);

    ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .align_start()
        .width_full()
        .child(title_row)
        .child(stats_row)
}

pub fn render_comic_data_card(card_id: &str) {
    tracing::info!("render_comic_data_card: card_id={}", card_id);
    let ui_tree = build_comic_data_card_ui();
    psys_host::ui_v3::render(card_id, ui_tree);
}
