use crate::astrobox::psys_host_v4::{self as psys_host, ui};
use super::state::*;
use super::theme::{self as t, ButtonKind as B};
use super::data_browser::{DataPhase, format_sync_time};
use super::deletion::{DeletePhase, DELETE_QUERY_PREFIX};
use crate::source_config::{CatalogPhase, CookieAction, SyncPhase};

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
            .child(t::text("宿主局域网 IPv4（手机端使用必填）", 14, t::TEXT))
            .child(ip_input)
            .child(t::hint("💡 电脑端通常直接使用 127.0.0.1 即可。\n📱 安卓手机端使用说明：手环与手机在不同系统内，无法直接用 127.0.0.1。请确保手环与手机在同一 Wi-Fi（或手环连接手机开的热点），并在此填入手机的局域网 IP：\n• 连 Wi-Fi 时：手机「设置 → WLAN → 点击当前 Wi-Fi → 查看 IP 地址」（如 192.168.1.xxx）\n• 开热点时：一般填 192.168.43.1 即可。")));

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
    let mut card = t::panel().gap(6)
        .child(t::text(title, 14, color))
        .child(t::text(message, 13, t::SECONDARY));

    if matches!(status, StatusState::Error(_)) && crate::jobs::can_resume_task().is_some() {
        card = card.child(
            t::button("继续未完成的下载", crate::ui::state::RESUME_UPLOAD_EVENT, B::Secondary, true).width_full()
        );
    }
    card
}

fn build_sync_ui(state: &UiState) -> ui::Element {
    let form = &state.source_form;
    let busy = form.phase == CatalogPhase::Loading || state.source_sync_busy();
    let mut domain = t::input(&form.input, "https://你的漫画源地址", DOMAIN_INPUT_CHANGE_EVENT);
    if busy { domain = domain.disabled().opacity(0.5); }
    let mut root = t::column()
        .child(t::section("漫画源同步", "读取完整配置，选择源并分别设置 Cookie，再发送到设备。"))
        .child(t::panel().gap(8)
            .child(t::text("漫画源 API 地址", 14, t::TEXT))
            .child(domain)
            .child(t::hint("输入完整地址后点击「读取 / 刷新配置」获取 /config；同步复用本次配置。支持域名、IP、localhost、端口及路径前缀，裸地址默认 HTTPS。"))
            .child(t::button(if form.phase == CatalogPhase::Loading { "正在读取配置…" } else { "读取 / 刷新配置" }, SOURCE_FETCH_EVENT,
                B::Secondary, !busy).width_full()));
    let (title, color) = match form.phase {
        CatalogPhase::Empty | CatalogPhase::Dirty => ("尚未就绪", t::MUTED),
        CatalogPhase::Loading => ("正在读取配置", t::ACCENT),
        CatalogPhase::Error => ("配置未就绪", t::DANGER),
        CatalogPhase::Ready => ("配置已读取", if form.catalog.as_ref().is_some_and(|c| c.valid_count() < c.entries.len()) { t::WARNING } else { t::ACCENT }),
    };
    root = root.child(t::panel().gap(6).child(t::text(title, 14, color))
        .child(t::hint(if form.message.is_empty() { "请输入漫画源 API 地址" } else { &form.message })));
    if let Some(catalog) = &form.catalog {
        root = root.child(t::text(&format!("源目录 · 共 {} 个 · 已选择 {} 个", catalog.entries.len(), form.selected_count()), 15, t::TEXT))
            .child(t::row().gap(8)
                .child(t::button("全选有效源", &format!("{SOURCE_ALL_PREFIX}{}", form.generation), B::Quiet, !busy && form.phase == CatalogPhase::Ready))
                .child(t::button("清除选择", &format!("{SOURCE_NONE_PREFIX}{}", form.generation), B::Quiet, !busy && form.phase == CatalogPhase::Ready)))
            .child(t::hint("设备按 key 更新覆盖同 key 配置。Cookie 的保留不会发送该项，清空才下发空字符串。"));
        let (start, end) = page_window(form.page, catalog.entries.len());
        for index in start..end {
            let entry = &catalog.entries[index];
            let choice = &form.choices[index];
            let enabled = !busy && entry.error.is_none() && form.phase == CatalogPhase::Ready;
            let suffix = format!("{}_{index}", form.generation);
            let mut check = ui::Element::new(ui::ElementType::Checkbox, None)
                .prop("checked", if choice.selected { "true" } else { "false" })
                .prop("aria-label", &format!("同步 {}（{}）", entry.name, entry.key));
            if enabled { check = check.on(ui::Event::Change, &format!("{SOURCE_SELECT_PREFIX}{suffix}")); }
            else { check = check.disabled(); }
            let mut card = t::panel().gap(8)
                .child(t::row().gap(8).child(check).child(t::text(&entry.name, 15, t::TEXT).flex_grow(1.0)))
                .child(t::hint(&format!("Key：{}", entry.key)))
                .child(t::hint(&entry.api_url));
            if let Some(error) = &entry.error {
                card = card.child(t::text(&error.to_string(), 13, t::DANGER));
            } else {
                let mut actions = t::row().gap(6);
                for (label, prefix, action) in [("保留 Cookie", SOURCE_COOKIE_KEEP_PREFIX, CookieAction::Keep),
                    ("更新 Cookie", SOURCE_COOKIE_UPDATE_PREFIX, CookieAction::Update),
                    ("清空 Cookie", SOURCE_COOKIE_CLEAR_PREFIX, CookieAction::Clear)] {
                    actions = actions.child(t::button(label, &format!("{prefix}{suffix}"),
                        if choice.cookie_action == action { B::AccentQuiet } else { B::Quiet }, enabled));
                }
                card = card.child(actions);
                if choice.cookie_action == CookieAction::Update {
                    let mut input = t::input(&choice.cookie, "仅更新此 key 的 Cookie；清空请使用清空按钮", &format!("{SOURCE_COOKIE_INPUT_PREFIX}{suffix}"))
                        .prop("type", "password").prop("maxlength", "16384");
                    if !enabled { input = input.disabled().opacity(0.5); }
                    card = card.child(input);
                } else {
                    card = card.child(t::hint(if choice.cookie_action == CookieAction::Keep { "保留设备已有 Cookie，不发送此项" }
                        else { "将该 key 的设备 Cookie 清空" }));
                }
            }
            root = root.child(card);
        }
        if catalog.entries.len() > PAGE_WINDOW {
            root = root.child(t::pager(form.page, catalog.entries.len(), SOURCE_CATALOG_PREV, SOURCE_CATALOG_NEXT));
        }
    }
    let plan = form.build_plan();
    if form.phase == CatalogPhase::Ready && let Err(error) = &plan {
        root = root.child(t::hint(&error.to_string()));
    }
    let device_busy = state.data_browser.busy() || state.deletes.busy() || state.upload_locked();
    if device_busy { root = root.child(t::hint("其他设备操作正在进行，完成后可发送源配置。")); }
    // Text edits update drafts without rendering. Keep this action reachable while
    // editing a Cookie; handle_sync validates the latest plan before any launch/send.
    root = root.child(t::button("同步所选源到设备", &format!("{SOURCE_SYNC_PREFIX}{}", form.generation), B::Primary,
        !busy && !device_busy && form.phase == CatalogPhase::Ready && form.selected_count() > 0).width_full());
    if let Some(sync) = &state.source_sync {
        let (title, color) = match sync.phase {
            SyncPhase::Preparing | SyncPhase::Sending => ("正在发送所选配置", t::ACCENT),
            SyncPhase::Sent => ("命令已发送，待设备核实", t::ACCENT),
            SyncPhase::Partial => ("部分命令已发送", t::WARNING),
            SyncPhase::Failed => ("本次发送未完成", t::DANGER),
            SyncPhase::Cancelled => ("本次发送已停止", t::MUTED),
        };
        root = root.child(t::panel().gap(6).child(t::text(title, 14, color)).child(t::hint(&sync.message))
            .child(t::hint(&format!("目标设备：{}（{}）", sync.device_name, sync.device_addr)))
            .child(t::hint(&format!("原配置入口：{}", sync.plan.endpoint))));
    }
    if !matches!(state.current_status, StatusState::Default) {
        root = root.child(status_card(&state.current_status));
    }
    root
}

fn build_data_ui(state: &UiState) -> ui::Element {
    let busy = state.data_browser.busy() || state.source_sync_busy() || matches!(state.app_data_status, StatusState::Processing(_));
    let can_delete = !busy && state.data_browser.lists_complete && state.data_browser.owner.as_ref()
        .is_some_and(|owner| !state.deletes.blocks_owner(&owner.addr));
    let preparing_delete = state.deletes.requests.iter().any(|r| matches!(r.phase, DeletePhase::Preparing | DeletePhase::Querying));
    let mut root = t::column()
        .child(t::section("设备书架管理", "读取手环已缓存的漫画和漫画源，支持封面回传与内容移除。"))
        .child(build_data_origin(state))
        .child(t::button(if state.source_sync_busy() { "漫画源配置发送中…" } else if busy { "正在读取设备数据…" } else { "读取快应用数据" }, FETCH_APP_DATA_EVENT, B::Primary, !busy && !preparing_delete).width_full());

    if let Some(owner) = &state.data_browser.owner && let Some(request) = state.deletes.latest_for(&owner.addr) {
        let (title, color) = match request.phase {
            DeletePhase::Preparing => ("准备删除", t::ACCENT),
            DeletePhase::Waiting => ("等待设备删除结果", t::ACCENT),
            DeletePhase::Querying => ("查询原删除结果", t::ACCENT),
            DeletePhase::Success => ("设备已确认删除", t::SUCCESS),
            DeletePhase::Failed => ("设备删除未完成", t::DANGER),
            DeletePhase::Partial => ("设备部分操作完成", t::WARNING),
            DeletePhase::Unknown | DeletePhase::LegacySent => ("删除结果待核实", t::WARNING),
            DeletePhase::VerifiedAbsent | DeletePhase::VerifiedPresent => ("已重新读取核实", t::MUTED),
        };
        let mut card = t::panel().gap(6).child(t::text(title, 14, color))
            .child(t::text(request.target.item.name(), 14, t::TEXT)).child(t::hint(&request.message))
            .child(t::hint(&format!("目标设备：{}（{}）", request.target.owner.name, request.target.owner.addr)));
        if request.phase == DeletePhase::Unknown && !request.session.is_empty() {
            card = card.child(t::button("查询原删除结果", &format!("{DELETE_QUERY_PREFIX}{}", request.id), B::Secondary,
                !busy && !state.deletes.busy()).width_full());
        }
        root = root.child(card);
    }

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
        &format!("漫画 {}/{} 本 · 漫画源 {}/{} 个",
            state.app_comics.iter().filter(|c| !c.name.is_empty()).count(), state.app_comic_count.unwrap_or(0),
            state.app_sources.iter().filter(|s| !s.name.is_empty()).count(), state.app_source_count.unwrap_or(0)),
        "book",
        None
    ));

    let comics = state.comic_matches();
    root = root.child(build_data_search("漫画名称", &state.comic_search, COMIC_SEARCH_EVENT, COMIC_SEARCH_CLEAR_EVENT))
        .child(t::text(&format!("漫画列表 · 已收到 {} 本 · 匹配 {} 本",
            state.app_comics.iter().filter(|c| !c.name.is_empty()).count(), comics.len()), 15, t::TEXT));
    if comics.is_empty() {
        root = root.child(t::hint(if !state.comic_search.trim().is_empty() { "没有匹配的漫画，试试其他名称或清除搜索。" }
            else if state.app_comic_count == Some(0) { "手环离线书架为空，可通过本地漫画功能导入。" }
            else { "尚未收到漫画条目，当前列表可能不完整。" }));
    } else {
        let (start, end) = page_window(state.comic_page_cursor, comics.len());
        for &index in &comics[start..end] {
            let comic = &state.app_comics[index];
            let detail = if comic.chapters == 0 { format!("{} 页 · 单本漫画", comic.page_count) }
                else { format!("{} 页 · {} 话", comic.page_count, comic.chapters) };
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
            let mut comic_card = t::panel().gap(10)
                .child(row.child(t::column().gap(4).flex_grow(1.0)
                    .child(t::text(&comic.name, 15, t::TEXT))
                    .child(t::hint(&format!("ID：{}", comic.id)))
                    .child(t::hint(&detail))));
            if comic.id.starts_with("local_") {
                let selected = state.upload_target.as_ref().is_some_and(|target| target.id == comic.id &&
                    state.data_browser.owner.as_ref().is_some_and(|owner| owner.addr == target.device_addr));
                comic_card = comic_card.child(t::button(if selected { "✓ 已选为导入目标" } else { "作为本地导入目标" },
                    &format!("{IMPORT_TARGET_PREFIX}{}_{index}", state.data_browser.revision),
                    if selected { B::AccentQuiet } else { B::Secondary }, can_delete && !state.upload_locked()).width_full());
            }
            root = root.child(comic_card.child(t::button("从设备删除", &format!("{DELETE_COMIC_PREFIX}{}_{index}",
                state.data_browser.revision), B::Danger, can_delete).width_full()));
        }
        if comics.len() > PAGE_WINDOW {
            root = root.child(t::pager(state.comic_page_cursor, comics.len(), COMIC_PAGE_PREV_EVENT, COMIC_PAGE_NEXT_EVENT));
        }
    }

    let sources = state.source_matches();
    root = root.child(build_data_search("漫画源名称", &state.source_search, SOURCE_SEARCH_EVENT, SOURCE_SEARCH_CLEAR_EVENT))
        .child(t::text(&format!("漫画源列表 · 已收到 {} 个 · 匹配 {} 个",
            state.app_sources.iter().filter(|s| !s.name.is_empty()).count(), sources.len()), 15, t::TEXT));
    if sources.is_empty() {
        root = root.child(t::hint(if !state.source_search.trim().is_empty() { "没有匹配的漫画源，试试其他名称或清除搜索。" }
            else if state.app_source_count == Some(0) { "设备未回传任何自定义漫画源。" }
            else { "尚未收到漫画源条目，当前列表可能不完整。" }));
    } else {
        let (start, end) = page_window(state.source_page_cursor, sources.len());
        for &index in &sources[start..end] {
            let source = &state.app_sources[index];
            root = root.child(t::panel().gap(10)
                .child(t::text(&source.name, 15, t::TEXT))
                .child(t::hint(&format!("Key：{}", if source.key.is_empty() { "旧端未回传" } else { &source.key })))
                .child(t::hint(&source.api_url))
                .child(t::button("删除漫画源", &format!("{DELETE_SOURCE_PREFIX}{}_{index}", state.data_browser.revision), B::Danger, can_delete).width_full()));
        }
        if sources.len() > PAGE_WINDOW {
            root = root.child(t::pager(state.source_page_cursor, sources.len(), SOURCE_PAGE_PREV_EVENT, SOURCE_PAGE_NEXT_EVENT));
        }
    }

    root
}

fn build_data_search(label: &str, query: &str, change: &str, clear: &str) -> ui::Element {
    t::row().gap(8)
        .child(t::input(query, &format!("搜索{label}…"), change).flex_grow(1.0))
        .child(t::button("清除", clear, B::Quiet, !query.is_empty()).flex_shrink(0.0))
}

fn build_data_origin(state: &UiState) -> ui::Element {
    let data = &state.data_browser;
    let mut card = t::panel().gap(6).child(t::text("数据来源与同步状态", 14, t::TEXT));
    if let Some(owner) = &data.owner {
        card = card.child(t::text(&owner.name, 15, t::TEXT)).child(t::hint(&owner.addr));
        let time = data.complete_time().map(format_sync_time).unwrap_or_else(|| "本设备尚无完整同步记录".into());
        card = card.child(t::hint(&format!("最近完整同步：{time}")));
        card = card.child(t::hint(if data.owner_matches_connection() {
            "连接核对：页面最近检查时，数据所属设备已连接"
        } else { "连接核对：页面最近检查时，当前连接与数据所属设备不同或已断开" }));
    } else {
        card = card.child(t::hint("尚未取得设备书架快照"));
    }
    if data.busy() && let Some(device) = &data.requested_device {
        card = card.child(t::hint(&format!("本次读取目标：{}（{}）", device.name, device.addr)));
    }
    let (message, color) = match data.phase {
        DataPhase::Idle => ("点击读取快应用数据开始同步".into(), t::MUTED),
        DataPhase::Connecting => (if data.owner.is_some() { "正在连接；新数据到达前仍显示上次列表" }
            else { "正在连接设备" }.into(), t::ACCENT),
        DataPhase::Lists => (format!("列表接收中：漫画 {}/{}，漫画源 {}/{}",
            state.sync_comics_seen.len(), state.app_comic_count.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
            state.sync_sources_seen.len(), state.app_source_count.map(|n| n.to_string()).unwrap_or_else(|| "?".into())), t::ACCENT),
        DataPhase::Covers => (format!("{}；已收到 {} 张封面", if data.lists_complete { "列表完整，正在接收封面" }
            else { "列表尚不完整，正在接收封面" }, data.covers_received), t::ACCENT),
        DataPhase::Finished => {
            let total = state.app_comic_count.unwrap_or(0);
            let missing = total.saturating_sub(data.covers_received);
            (if !data.lists_complete { "同步未完整：列表有缺项，请重新读取".into() }
                else if let Some(skipped) = data.covers_skipped {
                    if skipped == 0 { "同步完成：列表与封面已核对".into() }
                    else { format!("列表完整；{skipped} 张封面缺失或跳过") }
                } else if missing > 0 { format!("列表完整；{missing} 张封面未回传（无封面或缺失）") }
                else { "同步完成：列表与封面已接收".into() },
                if !data.lists_complete || missing > 0 { t::WARNING } else { t::SUCCESS })
        }
        DataPhase::Failed => (format!("{}{}", data.error.as_deref().unwrap_or("本次同步失败"),
            if !data.incoming && data.owner.is_some() { "；仍显示上次列表" }
            else if data.incoming { if data.lists_complete { "；列表完整，封面同步未完成" } else { "；当前仅有部分数据" } }
            else { "" }), t::DANGER),
    };
    card.child(t::text(&message, 12, color))
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
