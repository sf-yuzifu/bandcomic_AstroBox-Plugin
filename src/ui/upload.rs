use crate::astrobox::psys_host_v4::ui;
use super::state::*;
use super::theme::{self as t, ButtonKind as B};

pub fn build(state: &UiState) -> ui::Element {
    match state.upload_view {
        UploadView::Overview => overview(state),
        UploadView::Info => info(state),
        UploadView::Pages(chapter) => pages(state, chapter),
        UploadView::Cover => cover(state),
        UploadView::Connection => t::column()
            .child(t::subpage_header("连接设置", "发送时会自动建立连接，备用地址仅在必要时填写。", UPLOAD_OVERVIEW_EVENT))
            .child(super::build::build_connection_ui()),
    }
}

fn selected_cover(state: &UiState) -> Option<&UploadFile> {
    match state.upload_mode {
        UploadMode::Single => state.upload_items.first().and_then(|i| i.cover.as_ref()),
        UploadMode::Multi => state.multi_cover.as_ref(),
    }
}

fn target_card(state: &UiState) -> ui::Element {
    let Some(target) = &state.upload_target else {
        return t::panel().gap(6).child(t::text("发送目标", 14, t::TEXT))
            .child(t::hint("当前作品身份：首次新建，再次发送更新同一作品。可在设备书架选择已有本地作品。"));
    };
    let mut preview = t::row().gap(12);
    preview = if target.cover_base64.is_empty() {
        preview.child(ui::Element::new(ui::ElementType::Div, None).width(48).height(66).radius(8).bg(t::CONTROL)
            .flex().align_center().justify_center().flex_shrink(0.0).child(t::icon("book", t::MUTED)))
    } else {
        preview.child(ui::Element::new(ui::ElementType::Image, Some(&target.cover_base64))
            .width(48).height(66).radius(8).flex_shrink(0.0).prop("style", "object-fit:cover"))
    };
    let summary = if target.is_serial { format!("选择时设备概要：{} 页正文 · {} 话", target.page_count, target.chapters) }
        else { format!("选择时设备概要：{} 页正文 · 单本漫画", target.page_count) };
    preview = preview.child(t::column().gap(4).flex_grow(1.0)
        .child(t::text(&target.name, 16, t::TEXT))
        .child(t::hint(&format!("ID：{}", target.id))).child(t::hint(&summary)));
    t::panel().gap(10)
        .child(t::text("已选择设备作品作为发送目标", 14, t::ACCENT))
        .child(preview)
        .child(t::hint(&format!("目标设备：{}（{}）", target.device_name, target.device_addr)))
        .child(t::hint("此处是目标作品概要；现有章节和正文未载入编辑。下面整理的是本次待发送草稿。"))
        .child(t::hint("仅发送本章：追加/更新指定章，保留设备作品书名、封面及其他章。发送整本：以本次草稿替换目标整书。"))
        .child(t::button("取消目标选择，使用当前作品身份", IMPORT_TARGET_CLEAR, B::Quiet, !state.upload_locked()).width_full())
}

fn overview(state: &UiState) -> ui::Element {
    let count = state.page_count();
    let locked = state.upload_locked();
    let connection = crate::http_server::status();

    // 1. 顶部模式切换：对齐 AstroBox 经典 SegmentedControl 胶囊横排互斥药丸
    let mode_tabs = t::segmented_control(&[
        ("单本漫画", UPLOAD_MODE_SINGLE_EVENT, state.upload_mode == UploadMode::Single),
        ("多章节漫画", UPLOAD_MODE_MULTI_EVENT, state.upload_mode == UploadMode::Multi),
    ]);

    let mut root = t::column().gap(14)
        .child(mode_tabs);

    // 2. 状态区域（若有）
    if !matches!(state.upload_status, StatusState::Default) {
        root = root.child(status(state));
    }
    if !state.image_notices.is_empty() {
        let mut panel = t::panel().gap(6).child(t::text("图片处理说明（最近一次发送）", 14, t::WARNING));
        for notice in &state.image_notices { panel = panel.child(t::hint(notice)); }
        root = root.child(panel);
    }

    // Show the selected device record separately from the editable outgoing draft.
    root = root.child(target_card(state));

    // 3. 漫画基础信息主卡片（封面 + 漫画名 + 模式/页数统计，点击修改名称）
    let name = if state.upload_comic_name_input.trim().is_empty() {
        "未命名漫画（点击设置名称）"
    } else {
        state.upload_comic_name_input.trim()
    };
    let detail = match state.upload_mode {
        UploadMode::Single => format!("单本模式 · 已整理 {} 页正文", count),
        UploadMode::Multi => format!("多章模式 · 共 {} 章 / {} 页正文", state.upload_chapters.len(), count),
    };

    let mut book_card = t::row().bg(t::PANEL).radius(t::RADIUS).border(1, t::OUTLINE).padding(14).gap(12)
        .on(ui::Event::Click, UPLOAD_INFO_EVENT).prop("role", "button").tab_index(0);

    book_card = if let Some(file) = selected_cover(state) {
        book_card.child(t::thumbnail(file, 48, 66))
    } else {
        let placeholder = ui::Element::new(ui::ElementType::Div, None)
            .width(48).height(66).radius(8).bg(t::CONTROL)
            .flex().align_center().justify_center().flex_shrink(0.0)
            .child(t::icon("book", t::MUTED));
        book_card.child(placeholder)
    };

    book_card = book_card
        .child(t::column().gap(4).flex_grow(1.0)
            .child(t::text("待发送草稿（点击编辑名称）", 12, t::MUTED))
            .child(t::text(name, 16, t::TEXT))
            .child(t::hint(&detail)))
        .child(t::icon("chevron", t::MUTED));

    root = root.child(book_card);

    // 4. 内容与操作分区
    if state.upload_mode == UploadMode::Single {
        // 单本模式：正文页面卡片 + 封面设置卡片并列展示
        let pages_desc = if count == 0 {
            "暂无图片 · 点击添加或整理页面"
        } else {
            "点击调整图片顺序或移除页面"
        };
        root = root.child(t::card("正文页面", pages_desc, "pages", Some(UPLOAD_PAGES_EVENT)));

        let follows = state.single_cover_follows_first;
        let cover_desc = if selected_cover(state).is_none() {
            "未设置封面 · 可自动使用首张或自定义"
        } else if follows {
            "首张正文同时作为封面（正文完整保留）"
        } else {
            "独立封面图片（不计入正文页数）"
        };
        root = root.child(t::card("漫画封面", cover_desc, "cover", Some(UPLOAD_COVER_EVENT)));

    } else {
        // 多章节模式：章节列表与章节内页面
        let follows = state.multi_cover_follows_first;
        let cover_desc = if selected_cover(state).is_none() {
            "未设置作品封面 · 可使用首图或自定义"
        } else if follows {
            "首张正文同时作为作品封面（正文保留）"
        } else {
            "独立作品封面（不计入正文页数）"
        };
        root = root.child(t::card("作品封面", cover_desc, "cover", Some(UPLOAD_COVER_EVENT)));

        let chapter_header = t::row()
            .child(t::text(&format!("章节列表（{}）", state.upload_chapters.len()), 15, t::TEXT).flex_grow(1.0))
            .child(t::button("＋ 添加章节", UPLOAD_ADD_CHAPTER_EVENT, B::Secondary, !locked));

        root = root.child(chapter_header);

        if state.upload_chapters.is_empty() {
            root = root.child(t::panel().gap(6)
                .child(t::text("还没有任何章节", 15, t::TEXT))
                .child(t::hint("点击上方「添加章节」开始配置多话内容。")));
        } else {
            let (start, end) = page_window(state.upload_page_cursor, state.upload_chapters.len());
            for (index, chapter) in state.upload_chapters.iter().enumerate().take(end).skip(start) {
                let title = if chapter.name.trim().is_empty() {
                    format!("第 {} 章", chapter.number)
                } else {
                    format!("第 {} 章 · {}", chapter.number, chapter.name)
                };
                let desc = format!("{} 页正文 · 点击进入整理", chapter.files.len());
                root = root.child(t::card(&title, &desc, "pages", Some(&format!("{CHAPTER_EDIT_PREFIX}{index}"))));
            }
            if state.upload_chapters.len() > PAGE_WINDOW {
                root = root.child(t::pager(state.upload_page_cursor, state.upload_chapters.len(),
                    UPLOAD_PAGE_PREV_EVENT, UPLOAD_PAGE_NEXT_EVENT));
            }
        }
    }

    // 5. 设备连接摘要
    let connection_text = if connection.busy {
        "正在准备连接…"
    } else if connection.bound {
        "原生通道已连接 · 发送时自动校验"
    } else {
        "发送时自动连接 · 无需手动配置"
    };
    root = root.child(t::card("设备连接", connection_text, "device", Some(UPLOAD_CONNECTION_EVENT)));

    // 6. 底部主操作区
    let send_label = if locked { "正在处理中…" } else { "发送整本到设备" };
    root = root.child(t::button(send_label, UPLOAD_START_EVENT, B::Primary, count > 0 && !locked).width_full());

    if count == 0 {
        root = root.child(t::hint("请添加至少一页正文图片即可发送。支持 JPG/PNG/WebP/BMP/GIF。"));
    }

    if count > 0 || !state.upload_chapters.is_empty() || !state.upload_items.is_empty() {
        root = root.child(t::button("清空整理内容", UPLOAD_CLEAR_EVENT, B::Quiet, !locked).width_full());
    }

    root
}

fn info(state: &UiState) -> ui::Element {
    let locked = state.upload_locked();
    let mut name_input = t::input(
        &state.upload_comic_name_input,
        "例如：电锯人（留空则自动使用图片原名）",
        UPLOAD_NAME_INPUT_EVENT,
    );
    if locked {
        name_input = name_input.disabled().opacity(0.5);
    }

    t::column()
        .child(t::subpage_header(
            "漫画名称",
            "设置将在手环离线书架中显示的作品标题，完成后返回概览。",
            UPLOAD_OVERVIEW_EVENT,
        ))
        .child(
            t::panel()
                .gap(10)
                .child(t::text("作品标题", 14, t::TEXT))
                .child(name_input)
                .child(t::hint(
                    "建议输入简洁明了的书名。若保持为空，将自动根据首张图片的文件名命名。",
                )),
        )
        .child(t::button("保存并返回", UPLOAD_OVERVIEW_EVENT, B::Primary, true).width_full())
}

fn pages(state: &UiState, chapter_index: Option<usize>) -> ui::Element {
    let locked = state.upload_locked();
    let files: Vec<&UploadFile> = match chapter_index {
        Some(index) => state.upload_chapters.get(index).map(|c| c.files.iter().collect()).unwrap_or_default(),
        None => state.upload_items.iter().flat_map(|i| &i.files).collect(),
    };

    let title = if chapter_index.is_some() { "整理章节正文" } else { "整理正文页面" };
    let desc = "按顺序排列正文。长按或点击上移/下移可调整顺序，移除不会删除母版文件。";

    let mut root = t::column()
        .child(t::subpage_header(title, desc, UPLOAD_OVERVIEW_EVENT));

    if let Some(index) = chapter_index {
        if let Some(chapter) = state.upload_chapters.get(index) {
            let mut input = t::input(&chapter.name, &format!("第 {} 章（可选标题）", chapter.number),
                &format!("{CHAPTER_NAME_INPUT_PREFIX}{index}"));
            if locked { input = input.disabled().opacity(0.5); }
            root = root.child(t::panel().gap(8)
                .child(t::text("章节名称", 14, t::TEXT))
                .child(input));
            let mut number = t::input(&chapter.number.to_string(), "真实章号（1..100000）", &format!("{CHAPTER_NUMBER_INPUT_PREFIX}{index}"));
            if locked { number = number.disabled(); }
            root = root.child(t::panel().gap(6).child(t::text("真实章号", 14, t::TEXT)).child(number)
                .child(t::hint("追加/更新按此章号定位，删除其他编辑项不会改变本章章号。")));
        } else {
            return root.child(t::hint("该章节已被移除，请点击上方返回按钮返回。"));
        }
    }

    let bytes: usize = files.iter().map(|f| f.size).sum();
    let pick_event = chapter_index
        .map(|i| format!("{CHAPTER_PICK_FILES_PREFIX}{i}"))
        .unwrap_or_else(|| UPLOAD_PICK_FILES_EVENT.to_string());

    root = root
        .child(t::panel().gap(10)
            .child(t::row()
                .child(t::text(&format!("正文共 {} 页", files.len()), 15, t::TEXT).flex_grow(1.0))
                .child(t::hint(&format!("母版体积：{}", t::size(bytes)))))
            .child(t::button("＋ 添加漫画图片", &pick_event, B::Secondary, !locked).width_full())
            .child(t::hint("支持多次分批选取图片追加到当前章节末尾。")));

    if files.is_empty() {
        root = root.child(t::panel().gap(6)
            .child(t::text("正文暂无图片", 15, t::TEXT))
            .child(t::hint("点击上方按钮添加图片。首张图片会默认作为封面且保留在正文中。")));
    }

    let (start, end) = page_window(state.upload_page_cursor, files.len());
    for (index, file) in files.iter().enumerate().take(end).skip(start) {
        let (up, down, delete) = if let Some(chapter) = chapter_index {
            (format!("{CHAPTER_MOVE_UP_PREFIX}{chapter}_{index}"),
             format!("{CHAPTER_MOVE_DOWN_PREFIX}{chapter}_{index}"),
             format!("{CHAPTER_DEL_FILE_PREFIX}{chapter}_{index}"))
        } else {
            (format!("{UPLOAD_MOVE_UP_PREFIX}{index}"),
             format!("{UPLOAD_MOVE_DOWN_PREFIX}{index}"),
             format!("{UPLOAD_DELETE_PREFIX}{index}"))
        };

        let card = t::panel().gap(10)
            .child(t::row().gap(12)
                .child(t::thumbnail(file, 48, 66))
                .child(t::column().gap(4).flex_grow(1.0)
                    .child(t::text(&format!("第 {} 页 · {}", index + 1, file.name), 14, t::TEXT))
                    .child(t::hint(&format!("原图 {} · 预处理 {}", t::size(file.original_size), t::size(file.size))))))
            .child(t::row().gap(8)
                .child(t::button("▲ 上移", &up, B::Secondary, !locked && index > 0).flex_grow(1.0))
                .child(t::button("▼ 下移", &down, B::Secondary, !locked && index + 1 < files.len()).flex_grow(1.0))
                .child(t::button("移除", &delete, B::Danger, !locked).flex_grow(1.0)));

        root = root.child(card);
    }

    if files.len() > PAGE_WINDOW {
        root = root.child(t::pager(state.upload_page_cursor, files.len(), UPLOAD_PAGE_PREV_EVENT, UPLOAD_PAGE_NEXT_EVENT));
    }

    root = root.child(t::button("完成，返回概览", UPLOAD_OVERVIEW_EVENT, B::Primary, true).width_full());

    if let Some(index) = chapter_index {
        root = root.child(t::button("仅发送本章到设备", &format!("{CHAPTER_UPLOAD_PREFIX}{index}"), B::Secondary, !locked && !files.is_empty()).width_full())
            .child(t::hint("新版追加/更新原作品；未声明章节导入能力的旧端发送为独立单本。"))
            .child(t::button("删除本章节", &format!("{CHAPTER_DELETE_PREFIX}{index}"), B::Danger, !locked).width_full());
    }

    root
}

fn cover(state: &UiState) -> ui::Element {
    let locked = state.upload_locked();
    let follows = if state.upload_mode == UploadMode::Single {
        state.single_cover_follows_first
    } else {
        state.multi_cover_follows_first
    };

    let mut root = t::column()
        .child(t::subpage_header("漫画封面", "封面独立配置，不影响正文阅读，修改封面不会改变正文页数。", UPLOAD_OVERVIEW_EVENT));

    let mut preview = t::panel().gap(10);
    if let Some(file) = selected_cover(state) {
        preview = preview.child(t::row().gap(14)
            .child(t::thumbnail(file, 64, 88))
            .child(t::column().gap(6).flex_grow(1.0)
                .child(t::text(&file.name, 15, t::TEXT))
                .child(t::hint(if follows {
                    "✓ 正在使用首张正文作为封面（正文完整保留）"
                } else {
                    "正在使用独立封面图片（不占用正文页数）"
                }))));
    } else {
        preview = preview
            .child(t::text("未配置封面", 15, t::TEXT))
            .child(t::hint("无封面不会影响手环阅读。可以设置为首张正文或选取专属图片。"));
    }
    root = root.child(preview);

    let pick_cover_event = if state.upload_mode == UploadMode::Single {
        UPLOAD_PICK_COVER_EVENT
    } else {
        UPLOAD_PICK_MULTI_COVER_EVENT
    };

    root = root
        .child(t::button(
            if follows { "✓ 使用首张正文作为封面" } else { "使用首张正文作为封面" },
            UPLOAD_COVER_FIRST_EVENT,
            if follows { B::AccentQuiet } else { B::Secondary },
            !locked && state.page_count() > 0
        ).width_full())
        .child(t::button("选择独立封面图片", pick_cover_event, B::Secondary, !locked).width_full())
        .child(t::button("不使用封面", UPLOAD_COVER_NONE_EVENT, B::Quiet, !locked).width_full())
        .child(t::button("完成，返回概览", UPLOAD_OVERVIEW_EVENT, B::Primary, true).width_full());

    root
}

fn status(state: &UiState) -> ui::Element {
    let (title, message, color) = match &state.upload_status {
        StatusState::Processing(message) => ("正在处理", message.as_str(), t::ACCENT),
        StatusState::Success(message) => ("本次结果", message.as_str(),
            if message.contains("待核实") { t::WARNING } else { t::SUCCESS }),
        StatusState::Error(message) => ("提示", message.as_str(), t::DANGER),
        StatusState::Default => return t::column(),
    };

    let mut panel = t::panel().gap(6)
        .child(t::row().gap(8)
            .child(t::icon(if color == t::SUCCESS { "pages" } else { "device" }, color))
            .child(t::text(title, 14, color).flex_grow(1.0)))
        .child(t::text(message, 13, t::SECONDARY));

    if matches!(state.upload_status, StatusState::Processing(_)) && state.upload_progress > 0.0 {
        let percent = (state.upload_progress * 100.0).clamp(0.0, 100.0) as u32;
        panel = panel.child(ui::Element::new(ui::ElementType::Progress, None)
            .prop("value", &percent.to_string()).width_full().height(6).radius(3))
            .child(t::hint(&format!("{percent}% · {}", state.upload_current_file)));
    }

    if matches!(state.upload_status, StatusState::Error(_)) {
        panel = panel.child(t::hint("已整理的内容保留。请按上述原因修正图片、文件或连接后重试。"));
    }

    panel
}
