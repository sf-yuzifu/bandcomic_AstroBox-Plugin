use crate::astrobox::psys_host_v4::{self as psys_host, ui};
use super::state::*;
use super::theme::{self as t, ButtonKind as B};

pub fn render_main_ui(element_id: &str) {
    ui_state().write().unwrap_or_else(|p| p.into_inner()).root_element_id = Some(element_id.into());
    ui::render(element_id, build_main_ui());
}

pub fn rerender_main_ui() {
    let id = ui_state().read().unwrap_or_else(|p| p.into_inner()).root_element_id.clone();
    if let Some(id) = id { ui::render(&id, build_main_ui()); }
}

pub fn build_main_ui() -> ui::Element {
    let state = ui_state().read().unwrap_or_else(|p| p.into_inner());

    // 顶部主导航：AstroBox 经典分段控制胶囊
    let tabs = t::segmented_control(&[
        ("本地漫画", TAB_UPLOAD_EVENT, state.current_tab == TabPage::Upload),
        ("设备书架", TAB_DATA_EVENT, state.current_tab == TabPage::Data),
        ("漫画源配置", TAB_SYNC_EVENT, state.current_tab == TabPage::Sync),
    ]);

    let content = match state.current_tab {
        TabPage::Upload => super::upload::build(&state),
        TabPage::Data => build_data_ui(&state),
        TabPage::Sync => build_sync_ui(&state),
    };

    t::column().bg(t::CANVAS).padding(16).gap(16).child(tabs).child(content)
}

pub(super) fn build_connection_ui() -> ui::Element {
    let status = crate::http_server::status();
    let error = status.error.as_deref().or_else(|| status.bind_status.as_deref()
        .filter(|message| !status.busy && !message.starts_with("原生 HTTP 已连接")));
    let (title, description, color) = if status.busy {
        ("正在连接设备", status.bind_status.as_deref().unwrap_or("正在准备本地服务…"), t::ACCENT)
    } else if let Some(error) = error {
        ("连接异常", error, t::DANGER)
    } else if status.bound {
        ("原生连接已就绪", "与手环直连正常，发送漫画时自动复用。", t::SUCCESS)
    } else {
        ("发送时自动握手", "保持 AstroBox 正常连接手环即可，无需手动设置。", t::MUTED)
    };

    let mut ip_input = t::input(&status.fallback_input, "选填，例如 192.168.1.100", crate::http_server::IP_INPUT_EVENT)
        .on(ui::Event::Blur, crate::http_server::IP_SAVE_EVENT);
    if status.busy { ip_input = ip_input.disabled().opacity(0.5); }

    let mut root = t::column()
        .child(t::panel().gap(6)
            .child(t::row().gap(8)
                .child(t::icon("device", color))
                .child(t::text(title, 15, color).flex_grow(1.0)))
            .child(t::hint(description)))
        .child(t::panel().gap(8)
            .child(t::text("备用宿主 IPv4（可选）", 14, t::TEXT))
            .child(ip_input)
            .child(t::hint("系统优先使用 127.0.0.1 回环通信。如连接超时可填入运行 AstroBox 电脑的局域网 IP，无需端口。")));

    if let Some(endpoint) = &status.endpoint {
        root = root.child(t::card("当前绑定服务地址", endpoint, "source", None));
    }

    let (label, event) = if status.port.is_some() {
        ("停止本地服务", crate::http_server::STOP_EVENT)
    } else {
        ("启动本地服务", crate::http_server::START_EVENT)
    };

    root.child(t::button("重新测试连接", crate::http_server::BIND_EVENT, B::Secondary, !status.busy).width_full())
        .child(t::button(label, event, B::Quiet, !status.busy).width_full())
}

fn status_card(status: &StatusState) -> ui::Element {
    let (title, message, color) = match status {
        StatusState::Processing(message) => ("正在处理", message.as_str(), t::ACCENT),
        StatusState::Success(message) => ("完成", message.as_str(), t::SUCCESS),
        StatusState::Error(message) => ("提示", message.as_str(), t::DANGER),
        StatusState::Default => return t::column(),
    };
    t::panel().gap(6)
        .child(t::text(title, 14, color))
        .child(t::text(message, 13, t::SECONDARY))
}

fn build_sync_ui(state: &UiState) -> ui::Element {
    let busy = matches!(state.current_status, StatusState::Processing(_));
    let mut domain = t::input(&state.config.domain, "https://你的漫画源地址", DOMAIN_INPUT_CHANGE_EVENT)
        .on(ui::Event::Blur, DOMAIN_INPUT_BLUR_EVENT);
    let mut cookie = t::input(&state.config.cookie, "如该漫画源无需鉴权可留空", COOKIE_INPUT_EVENT);
    if busy {
        domain = domain.disabled().opacity(0.5);
        cookie = cookie.disabled().opacity(0.5);
    }
    let source = state.fetched_source_name.as_deref()
        .or_else(|| (!state.config.source_name.is_empty()).then_some(state.config.source_name.as_str()))
        .unwrap_or("输入域名后自动检测");

    let mut root = t::column()
        .child(t::section("漫画源同步", "将自定义漫画源与 Cookie 认证下发到设备端的腕上漫画。"))
        .child(t::panel().gap(8)
            .child(t::text("漫画源 API 地址", 14, t::TEXT))
            .child(domain)
            .child(t::hint("需提供标准 /config 接口的漫画源。离开输入框自动拉取解析。")))
        .child(t::card("检测到的漫画源名称", source, "source", None))
        .child(t::panel().gap(8)
            .child(t::text("Cookie 凭据（可选）", 14, t::TEXT))
            .child(cookie)
            .child(t::hint("仅针对需要登录鉴权的漫画源填写，将同步写入设备 cookie.json。")))
        .child(t::button("同步到手环设备", SYNC_BUTTON_EVENT, B::Primary,
            !busy && !state.config.domain.trim().is_empty()).width_full());

    if !matches!(state.current_status, StatusState::Default) {
        root = root.child(status_card(&state.current_status));
    }
    root
}

fn build_data_ui(state: &UiState) -> ui::Element {
    let busy = matches!(state.app_data_status, StatusState::Processing(_));
    let mut root = t::column()
        .child(t::section("设备书架管理", "读取手环已缓存的漫画和漫画源，支持封面回传与内容移除。"))
        .child(t::button(if busy { "正在读取设备数据…" } else { "读取快应用数据" }, FETCH_APP_DATA_EVENT, B::Primary, !busy).width_full());

    if !matches!(state.app_data_status, StatusState::Default) {
        root = root.child(status_card(&state.app_data_status));
    }

    if state.app_comic_count.is_none() && state.app_source_count.is_none() {
        return root.child(t::panel().gap(6)
            .child(t::text("尚未获取设备数据", 15, t::TEXT))
            .child(t::hint("保持手环屏幕亮起并与 AstroBox 连接，点击上方按钮拉取书架清单。")));
    }

    root = root.child(t::card(
        "设备当前数据概览",
        &format!("已缓存 {} 本漫画 · 已配置 {} 个漫画源", state.app_comic_count.unwrap_or(0), state.app_source_count.unwrap_or(0)),
        "book",
        None
    ));

    if state.app_comics.is_empty() {
        root = root.child(t::hint("手环离线书架为空，可通过本地漫画功能导入。"));
    } else {
        root = root.child(t::text(&format!("漫画列表（共 {} 本）", state.app_comics.len()), 15, t::TEXT));
        let (start, end) = page_window(state.comic_page_cursor, state.app_comics.len());
        for (index, comic) in state.app_comics.iter().enumerate().take(end).skip(start) {
            let detail = format!("{} 页 · {} 话", comic.page_count, comic.chapters.max(1));
            let mut row = t::row().gap(12);
            if !comic.cover_base64.is_empty() {
                row = row.child(ui::Element::new(ui::ElementType::Image, Some(&comic.cover_base64))
                    .width(44).height(62).radius(8).flex_shrink(0.0).prop("style", "object-fit:cover"));
            } else {
                let placeholder = ui::Element::new(ui::ElementType::Div, None)
                    .width(44).height(62).radius(8).bg(t::CONTROL)
                    .flex().align_center().justify_center().flex_shrink(0.0)
                    .child(t::icon("book", t::MUTED));
                row = row.child(placeholder);
            }
            root = root.child(t::panel().gap(10)
                .child(row.child(t::column().gap(4).flex_grow(1.0)
                    .child(t::text(&comic.name, 15, t::TEXT))
                    .child(t::hint(&detail))))
                .child(t::button("从设备删除", &format!("{DELETE_COMIC_PREFIX}{index}"), B::Danger, !busy).width_full()));
        }
        if state.app_comics.len() > PAGE_WINDOW {
            root = root.child(t::pager(state.comic_page_cursor, state.app_comics.len(), COMIC_PAGE_PREV_EVENT, COMIC_PAGE_NEXT_EVENT));
        }
    }

    if !state.app_sources.is_empty() {
        root = root.child(t::text(&format!("漫画源列表（共 {} 个）", state.app_sources.len()), 15, t::TEXT));
        let (start, end) = page_window(state.source_page_cursor, state.app_sources.len());
        for (index, source) in state.app_sources.iter().enumerate().take(end).skip(start) {
            root = root.child(t::panel().gap(10)
                .child(t::text(&source.name, 15, t::TEXT))
                .child(t::hint(&source.api_url))
                .child(t::button("删除漫画源", &format!("{DELETE_SOURCE_PREFIX}{index}"), B::Danger, !busy).width_full()));
        }
        if state.app_sources.len() > PAGE_WINDOW {
            root = root.child(t::pager(state.source_page_cursor, state.app_sources.len(), SOURCE_PAGE_PREV_EVENT, SOURCE_PAGE_NEXT_EVENT));
        }
    }

    root
}

pub fn render_comic_data_card(card_id: &str) {
    let state = ui_state().read().unwrap_or_else(|p| p.into_inner());
    let value = |count: Option<usize>| count.map(|c| c.to_string()).unwrap_or_else(|| "—".into());
    let tree = t::panel().gap(8)
        .child(t::text("腕上漫画 · 数据概览", 14, t::TEXT))
        .child(t::row().gap(16)
            .child(t::column().flex_grow(1.0)
                .child(t::text(&value(state.app_comic_count), 26, t::TEXT))
                .child(t::hint("已下载漫画")))
            .child(t::column().flex_grow(1.0)
                .child(t::text(&value(state.app_source_count), 26, t::TEXT))
                .child(t::hint("已同步书源"))));
    psys_host::ui::render(card_id, tree);
}
