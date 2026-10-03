//! ClaraLight-inspired tokens mapped to AstroBox's V4 UI tree.
use crate::astrobox::psys_host_v4::ui;

pub const CANVAS: &str = "#191919";
pub const PANEL: &str = "#1E1E1F";
pub const CONTROL: &str = "rgba(255, 255, 255, 0.08)";
pub const CONTROL_HOVER: &str = "rgba(255, 255, 255, 0.12)";
pub const OUTLINE: &str = "rgba(255, 255, 255, 0.08)";
pub const OUTLINE_STRONG: &str = "rgba(255, 255, 255, 0.16)";
pub const TEXT: &str = "#FFFFFF";
pub const SECONDARY: &str = "rgba(255, 255, 255, 0.75)";
pub const MUTED: &str = "rgba(255, 255, 255, 0.45)";
pub const ACCENT: &str = "#0090FF";
pub const ACCENT_BG: &str = "rgba(0, 144, 255, 0.15)";
pub const SUCCESS: &str = "#30D158";
pub const WARNING: &str = "#FFBA18";
pub const DANGER: &str = "#E5484D";
pub const DANGER_BG: &str = "rgba(229, 72, 77, 0.15)";
pub const RADIUS: u32 = 18;
pub const RADIUS_SM: u32 = 10;
pub const INPUT_HEIGHT: u32 = 42;

#[derive(Clone, Copy)]
pub enum ButtonKind { Primary, Secondary, Quiet, Danger, AccentQuiet }

pub fn column() -> ui::Element {
    ui::Element::new(ui::ElementType::Div, None).flex()
        .flex_direction(ui::FlexDirection::Column).width_full().min_width(0).gap(12)
}

pub fn row() -> ui::Element {
    ui::Element::new(ui::ElementType::Div, None).flex()
        .flex_direction(ui::FlexDirection::Row).align_center().width_full().min_width(0).gap(10)
}

pub fn text(value: &str, size: u32, color: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::P, Some(value)).size(size).text_color(color)
        .min_width(0).prop("style", "overflow-wrap:anywhere; margin:0")
}

pub fn hint(value: &str) -> ui::Element { text(value, 12, MUTED) }

pub fn panel() -> ui::Element {
    column().bg(PANEL).radius(RADIUS).border(1, OUTLINE).padding(16)
}

pub fn section(title: &str, description: &str) -> ui::Element {
    column().gap(4).margin_top(2).margin_bottom(4)
        .child(text(title, 20, TEXT)).child(hint(description))
}

pub fn input(value: &str, placeholder: &str, event: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Input, Some(value)).without_default_styles()
        .on(ui::Event::Change, event).prop("placeholder", placeholder)
        .height(INPUT_HEIGHT).width_full().min_width(0).radius(12)
        .border(1, OUTLINE).bg(CONTROL).text_color(TEXT).size(14)
        .padding_left(14).padding_right(14)
}

pub fn button(label: &str, event: &str, kind: ButtonKind, enabled: bool) -> ui::Element {
    let (bg, color) = match kind {
        ButtonKind::Primary => (ACCENT, TEXT),
        ButtonKind::Secondary => (CONTROL, SECONDARY),
        ButtonKind::Quiet => ("transparent", MUTED),
        ButtonKind::Danger => (DANGER_BG, DANGER),
        ButtonKind::AccentQuiet => (ACCENT_BG, ACCENT),
    };
    let mut button = ui::Element::new(ui::ElementType::Button, None).without_default_styles()
        .radius(12).min_height(38).padding_left(16).padding_right(16)
        .padding_top(8).padding_bottom(8).bg(bg).text_color(color)
        .flex().align_center().justify_center().min_width(0)
        .prop("aria-label", label).child(text(label, 13, color));
    if enabled { button = button.on(ui::Event::Click, event); }
    else { button = button.disabled().opacity(0.35); }
    button
}

/// AstroBox / ClaraLight 风格圆形导航返回按钮（36x36 纯圆，精致矢量箭头）
pub fn back_button(event: &str) -> ui::Element {
    let arrow_svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>"#;
    let icon = ui::Element::new(ui::ElementType::Svg, Some(arrow_svg))
        .width(18).height(18).text_color(TEXT);

    ui::Element::new(ui::ElementType::Button, None).without_default_styles()
        .on(ui::Event::Click, event)
        .width(36).height(36).radius(999)
        .bg(CONTROL).border(1, OUTLINE)
        .flex().align_center().justify_center().flex_shrink(0.0)
        .prop("aria-label", "返回")
        .child(icon)
}

/// 标准子页面头部：圆形返回按钮 + 大标题 + 辅助描述
pub fn subpage_header(title: &str, description: &str, back_event: &str) -> ui::Element {
    let top_bar = row().gap(12)
        .child(back_button(back_event))
        .child(text(title, 19, TEXT).flex_grow(1.0));

    column().gap(6).margin_bottom(4)
        .child(top_bar)
        .child(hint(description).margin_left(48))
}

/// ClaraLight 经典分段控制器（Segmented Control 胶囊底座 + 互斥选项药丸）
pub fn segmented_control(items: &[(&str, &str, bool)]) -> ui::Element {
    let mut root = ui::Element::new(ui::ElementType::Div, None).flex()
        .flex_direction(ui::FlexDirection::Row).align_center().width_full()
        .bg("rgba(255, 255, 255, 0.05)").radius(14).border(1, OUTLINE).padding(4).gap(4);

    for (label, event, active) in items {
        let (bg, text_color, font_size) = if *active {
            ("rgba(255, 255, 255, 0.14)", TEXT, 13)
        } else {
            ("transparent", MUTED, 13)
        };

        let mut btn = ui::Element::new(ui::ElementType::Button, None).without_default_styles()
            .radius(10).height(34).flex().align_center().justify_center().flex_grow(1.0)
            .bg(bg).text_color(text_color).child(text(label, font_size, text_color));

        if !*active {
            btn = btn.on(ui::Event::Click, event);
        }
        root = root.child(btn);
    }
    root
}

pub fn icon(name: &str, color: &str) -> ui::Element {
    let path = match name {
        "book" => "<path d='M4 19.5A2.5 2.5 0 0 1 6.5 17H20'/><path d='M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z'/>",
        "pages" => "<rect x='5' y='3' width='14' height='18' rx='2'/><path d='M3 7v12M9 8h6M9 12h6M9 16h4'/>",
        "cover" => "<rect x='3' y='3' width='18' height='18' rx='3'/><path d='m3 16 5-5 4 4 3-3 6 6'/><circle cx='15' cy='8' r='1.5'/>",
        "device" => "<rect x='6' y='4' width='12' height='16' rx='3'/><path d='M9 4V2h6v2M9 20v2h6v-2'/>",
        "source" => "<circle cx='12' cy='12' r='9'/><path d='M3 12h18M12 3c5 5 5 13 0 18-5-5-5-13 0-18'/>",
        "back" => "<polyline points='15 18 9 12 15 6'/>",
        "chevron" => "<polyline points='9 18 15 12 9 6'/>",
        "plus" => "<line x1='12' y1='5' x2='12' y2='19'/><line x1='5' y1='12' x2='19' y2='12'/>",
        "trash" => "<polyline points='3 6 5 6 21 6'/><path d='M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2'/>",
        _ => "<path d='M12 5v14M5 12h14'/>",
    };
    let svg = format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>{path}</svg>");
    ui::Element::new(ui::ElementType::Svg, Some(&svg)).width(20).height(20)
        .flex_shrink(0.0).text_color(color)
}

pub fn card(title: &str, description: &str, symbol: &str, event: Option<&str>) -> ui::Element {
    let icon_wrapper = ui::Element::new(ui::ElementType::Div, None)
        .width(36).height(36).radius(10).bg(CONTROL)
        .flex().align_center().justify_center().flex_shrink(0.0)
        .child(icon(symbol, SECONDARY));

    let info = column().gap(3).flex_grow(1.0)
        .child(text(title, 15, TEXT)).child(hint(description));

    let mut card = row().bg(PANEL).radius(RADIUS).border(1, OUTLINE).padding(14).gap(12)
        .child(icon_wrapper).child(info);

    if let Some(event) = event {
        card = card.on(ui::Event::Click, event).prop("role", "button").tab_index(0)
            .child(icon("chevron", MUTED));
    }
    card
}

pub fn pager(cursor: usize, total: usize, prev: &str, next: &str) -> ui::Element {
    let (start, end) = super::state::page_window(cursor, total);
    let current = start / super::state::PAGE_WINDOW;
    let pages = total.div_ceil(super::state::PAGE_WINDOW).max(1);
    row().gap(8)
        .child(button("上一页", prev, ButtonKind::Secondary, current > 0).flex_grow(1.0))
        .child(text(&format!("{} / {}", current + 1, pages), 12, MUTED).flex_shrink(0.0))
        .child(button("下一页", next, ButtonKind::Secondary, end < total).flex_grow(1.0))
}

pub fn size(bytes: usize) -> String {
    if bytes < 1024 { format!("{bytes} B") }
    else if bytes < 1024 * 1024 { format!("{:.1} KB", bytes as f64 / 1024.0) }
    else { format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)) }
}

pub fn thumbnail(file: &super::state::UploadFile, width: u32, height: u32) -> ui::Element {
    let mime = match image::guess_format(&file.thumbnail) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::WebP) => "image/webp",
        Ok(image::ImageFormat::Bmp) => "image/bmp",
        Ok(image::ImageFormat::Gif) => "image/gif",
        _ => "image/jpeg",
    };
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(file.thumbnail.len().div_ceil(3) * 4);
    for chunk in file.thumbnail.chunks(3) {
        let a = chunk[0]; let b = *chunk.get(1).unwrap_or(&0); let c = *chunk.get(2).unwrap_or(&0);
        encoded.push(alphabet[(a >> 2) as usize] as char);
        encoded.push(alphabet[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 { alphabet[(((b & 15) << 2) | (c >> 6)) as usize] as char } else { '=' });
        encoded.push(if chunk.len() > 2 { alphabet[(c & 63) as usize] as char } else { '=' });
    }
    ui::Element::new(ui::ElementType::Image, Some(&format!("data:{mime};base64,{encoded}")))
        .width(width).height(height).radius(8).flex_shrink(0.0).prop("style", "object-fit:cover")
}
