use crate::astrobox::psys_host::ui;
use super::state::*;
use super::message::get_status_text;

/// 渲染主 UI 到指定元素
pub fn render_main_ui(element_id: &str) {
    // 保存 root_element_id
    {
        let mut state = ui_state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.root_element_id = Some(element_id.to_string());
    }
    
    let ui_element = build_main_ui();
    ui::render(element_id, ui_element);
}

/// 构建主 UI（从状态读取数据）
pub fn build_main_ui() -> ui::Element {
    let state = ui_state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // 主容器 - 使用 flex 布局
    let container = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .padding(16)
        .width_full();

    // 域名标签
    let domain_label = ui::Element::new(
        ui::ElementType::P,
        Some("漫画源域名（例如：https://youapi.domain）"),
    )
    .size(14)
    .margin_bottom(4)
    .width_full();

    // 域名输入框
    let domain_input = ui::Element::new(ui::ElementType::Input, Some(&state.config.domain))
        .on(ui::Event::Change, DOMAIN_INPUT_CHANGE_EVENT)
        .on(ui::Event::Blur, DOMAIN_INPUT_BLUR_EVENT)
        .radius(4)
        .bg("#2A2A2A")
        .width_full()
        .margin_bottom(12)
        .padding(8);

    // 漫画源名称标签
    let source_name_label = ui::Element::new(
        ui::ElementType::P,
        Some("漫画源名称（自动获取）"),
    )
    .size(14)
    .margin_bottom(4)
    .width_full();

    // 漫画源名称输入框（只读）
    let source_name_value = state.fetched_source_name.as_deref()
        .or_else(|| if state.config.source_name.is_empty() { None } else { Some(&state.config.source_name) })
        .unwrap_or("");
    
    let source_name_input = ui::Element::new(ui::ElementType::Input, Some(source_name_value))
        .radius(4)
        .bg("#1A1A1A")
        .width_full()
        .margin_bottom(12)
        .padding(8)
        .disabled();

    // Cookie 标签
    let cookie_label = ui::Element::new(
        ui::ElementType::P,
        Some("Cookie（从浏览器开发者工具获取）"),
    )
    .size(14)
    .margin_bottom(4)
    .width_full();

    // Cookie 输入框
    let cookie_input = ui::Element::new(ui::ElementType::Input, Some(&state.config.cookie))
        .on(ui::Event::Change, COOKIE_INPUT_EVENT)
        .radius(4)
        .bg("#2A2A2A")
        .width_full()
        .margin_bottom(12)
        .padding(8);

    // 状态消息 - 使用纯文本
    let (status_text, text_color) = get_status_text(&state.current_status);
    let status_message = ui::Element::new(ui::ElementType::P, Some(&status_text))
        .without_default_styles()
        .text_color(&text_color)
        .size(13)
        .align_center()
        .width_full()
        .margin_bottom(12);

    // 同步按钮
    let sync_button = ui::Element::new(ui::ElementType::Button, Some("同步到手表"))
        .on(ui::Event::Click, SYNC_BUTTON_EVENT)
        .radius(4)
        .bg("#1890ff")
        .text_color("#ffffff")
        .width_full()
        .padding(12)
        .size(14);

    // 组装 UI
    container
        .child(domain_label)
        .child(domain_input)
        .child(source_name_label)
        .child(source_name_input)
        .child(cookie_label)
        .child(cookie_input)
        .child(status_message)
        .child(sync_button)
}
