//! Feature hub actions, filtering and rendering.
use crate::i18n::right_panel as rp;
use crate::theme::Palette;
use gpui_kit::{
    assets::IconName,
    base::StyledExt,
    component::input::{Input, InputState},
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RightPanelAction {
    OpenAi,
    OpenKnowledgeBase,
    OpenTerminal,
    TogglePreview,
    ToggleLogs,
    ToggleZen,
    OpenFile,
    OpenPreferences,
    ActivateAiTask(String),
}

pub(crate) struct HubActionItem {
    pub(crate) action: RightPanelAction,
    pub(crate) icon: IconName,
    pub(crate) label: &'static str,
    keywords: &'static str,
    pub(crate) badge: Option<String>,
    pub(crate) active: bool,
    pub(crate) danger: bool,
}

pub(crate) struct RightPanelHubProps {
    pub(crate) query: String,
    pub(crate) selected_index: usize,
    pub(crate) panel_width: f32,
    pub(crate) preview_open: bool,
    pub(crate) logs_open: bool,
    pub(crate) zen_mode: bool,
    pub(crate) build_running: bool,
    pub(crate) build_failed: bool,
    pub(crate) build_target: String,
    pub(crate) ai_shortcut: String,
    pub(crate) terminal_shortcut: String,
    pub(crate) zen_shortcut: String,
    pub(crate) terminal_sessions: usize,
    pub(crate) ai_tasks: Vec<(String, String, bool)>,
    pub(crate) kb_badge: Option<(String, bool)>,
    pub(crate) kb_syncing: bool,
}

fn fg_alpha(hex: u32, alpha: f32) -> Rgba {
    let mut color = gpui_kit::rgb(hex);
    color.a = alpha;
    color
}

pub(crate) fn core_actions(props: &RightPanelHubProps) -> Vec<HubActionItem> {
    vec![
        HubActionItem {
            action: RightPanelAction::OpenAi,
            icon: IconName::Sparkles,
            label: rp::ACTION_AI.get(),
            keywords: "ai assistant chat 对话 智能 助手",
            badge: (!props.ai_shortcut.is_empty()).then(|| props.ai_shortcut.clone()),
            active: false,
            danger: false,
        },
        HubActionItem {
            action: RightPanelAction::OpenKnowledgeBase,
            icon: IconName::Library,
            label: rp::ACTION_KNOWLEDGE_BASE.get(),
            keywords: "knowledge base kb rag search index pdf ocr 知识库 检索 索引",
            badge: props.kb_badge.as_ref().map(|(text, _)| text.clone()),
            active: props.kb_syncing,
            danger: props.kb_badge.as_ref().is_some_and(|(_, warn)| *warn),
        },
        HubActionItem {
            action: RightPanelAction::OpenTerminal,
            icon: IconName::Terminal,
            label: rp::ACTION_TERMINAL.get(),
            keywords: "terminal shell console cmd 终端 命令行",
            badge: (!props.terminal_shortcut.is_empty()).then(|| props.terminal_shortcut.clone()),
            active: false,
            danger: false,
        },
    ]
}

fn all_actions(props: &RightPanelHubProps) -> Vec<HubActionItem> {
    let preview_badge = props.preview_open.then(|| rp::BADGE_OPENED.get().to_string());
    let logs_badge = if props.logs_open {
        Some(rp::BADGE_EXPANDED.get().to_string())
    } else if props.build_failed {
        Some(rp::BADGE_BUILD_FAILED.get().to_string())
    } else {
        None
    };

    let mut all = core_actions(props);
    all.extend([
        HubActionItem {
            action: RightPanelAction::TogglePreview,
            icon: IconName::PanelRightOpen,
            label: rp::ACTION_PDF_PREVIEW.get(),
            keywords: "pdf preview 预览 分栏",
            badge: preview_badge,
            active: props.preview_open,
            danger: false,
        },
        HubActionItem {
            action: RightPanelAction::ToggleLogs,
            icon: IconName::ListTree,
            label: rp::ACTION_BUILD_LOGS.get(),
            keywords: "log build output 日志 编译 输出",
            badge: logs_badge,
            active: props.logs_open,
            danger: props.build_failed && !props.logs_open,
        },
        HubActionItem {
            action: RightPanelAction::ToggleZen,
            icon: IconName::Maximize2,
            label: rp::ACTION_ZEN_MODE.get(),
            keywords: "zen focus distraction free fullscreen 专注 全屏 禅模式",
            badge: (!props.zen_shortcut.is_empty()).then(|| props.zen_shortcut.clone()),
            active: props.zen_mode,
            danger: false,
        },
        HubActionItem {
            action: RightPanelAction::OpenFile,
            icon: IconName::FolderOpen,
            label: rp::ACTION_OPEN_FILE.get(),
            keywords: "open file 打开 文件",
            badge: Some("Ctrl+Shift+O".into()),
            active: false,
            danger: false,
        },
        HubActionItem {
            action: RightPanelAction::OpenPreferences,
            icon: IconName::Settings,
            label: rp::ACTION_PREFERENCES.get(),
            keywords: "settings preferences config 设置 偏好 主题",
            badge: None,
            active: false,
            danger: false,
        },
    ]);
    all
}

pub(crate) fn filtered_actions(props: &RightPanelHubProps) -> Vec<HubActionItem> {
    let query = props.query.trim().to_lowercase();
    if query.is_empty() {
        return core_actions(props);
    }
    all_actions(props)
        .into_iter()
        .filter(|item| {
            item.label.to_lowercase().contains(&query)
                || item.keywords.to_lowercase().contains(&query)
        })
        .collect()
}

pub(crate) fn render_hub(
    props: &RightPanelHubProps,
    search_input: &Entity<InputState>,
    palette: Palette,
    window: &Window,
    cx: &App,
    on_action: impl Fn(RightPanelAction, &mut Window, &mut App) + Clone + 'static,
    on_clear: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> AnyElement {
    let items = filtered_actions(props);
    let search_focused = search_input.read(cx).focus_handle(cx).is_focused(window);
    let has_query = !props.query.trim().is_empty();
    let compact = props.panel_width < 340.0;
    let h_pad = if compact { 14.0 } else { 20.0 };
    let top_pad = if has_query { 20.0 } else { 52.0 };
    let selected = if items.is_empty() {
        0
    } else {
        props.selected_index.min(items.len() - 1)
    };

    let ring_color = if search_focused {
        fg_alpha(palette.accent, 0.72)
    } else {
        fg_alpha(palette.border, if palette.is_dark() { 0.85 } else { 0.95 })
    };
    let outer_halo = fg_alpha(palette.accent, 0.14);
    let row_hover = gpui_kit::rgb(palette.hover);
    let row_selected = fg_alpha(palette.text, 0.06);
    let badge_bg = fg_alpha(palette.text, 0.06);
    let card_bg = if palette.is_dark() {
        fg_alpha(palette.text, 0.055)
    } else {
        gpui_kit::rgb(0xf4f4f2)
    };
    let card_hover = if palette.is_dark() {
        fg_alpha(palette.text, 0.095)
    } else {
        gpui_kit::rgb(0xecebe8)
    };
    let card_border = fg_alpha(palette.border, if palette.is_dark() { 0.5 } else { 0.65 });
    let search_shadow = vec![
        BoxShadow::new(
            px(0.),
            px(1.),
            hsla(0., 0., 0., if palette.is_dark() { 0.24 } else { 0.04 }),
        )
        .blur_radius(px(4.)),
    ];

    let search_bar = div()
        .w_full()
        .rounded_full()
        .when(search_focused, |el| el.p(px(2.)).bg(outer_halo))
        .child(
            div()
                .w_full()
                .h(px(40.))
                .px(px(16.))
                .rounded_full()
                .bg(gpui_kit::rgb(palette.surface))
                .border_1()
                .border_color(ring_color)
                .shadow(search_shadow)
                .flex()
                .items_center()
                .gap(px(9.))
                .child(
                    crate::ui::icon(IconName::Search)
                        .w(px(15.))
                        .h(px(15.))
                        .flex_shrink_0()
                        .text_color(gpui_kit::rgb(palette.muted)),
                )
                .child(
                    Input::new(search_input)
                        .appearance(false)
                        .bordered(false)
                        .flex_1()
                        .min_w_0()
                        .px_0()
                        .py_0()
                        .text_size(px(13.5))
                        .text_color(gpui_kit::rgb(palette.text)),
                )
                .when(has_query, |bar| {
                    let clear = on_clear.clone();
                    bar.child(
                        div()
                            .id("right-hub-clear-search")
                            .size(px(20.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .hover(move |s| s.bg(row_hover).text_color(gpui_kit::rgb(palette.text)))
                            .on_click(move |_, window, cx| clear(window, cx))
                            .child(crate::ui::icon(IconName::Close).w(px(12.)).h(px(12.))),
                    )
                }),
        );

    let main_section = if !has_query {
        let card_gap = if compact { 8.0 } else { 10.0 };
        let cards_row = div()
            .w_full()
            .flex()
            .items_stretch()
            .gap(px(card_gap))
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                let action = item.action.clone();
                let trigger = on_action.clone();
                let icon_color = if item.danger {
                    gpui_kit::rgb(palette.danger)
                } else if item.active {
                    gpui_kit::rgb(palette.accent)
                } else {
                    gpui_kit::rgb(palette.text)
                };
                let status_el = match action {
                    RightPanelAction::OpenKnowledgeBase => item.badge.map(|badge| {
                        let color = if item.danger {
                            gpui_kit::rgb(palette.danger)
                        } else if item.active {
                            gpui_kit::rgb(palette.accent)
                        } else {
                            gpui_kit::rgb(palette.muted)
                        };
                        div()
                            .text_size(px(11.))
                            .text_color(color)
                            .text_ellipsis()
                            .child(badge)
                            .into_any_element()
                    }),
                    RightPanelAction::OpenAi => (!props.ai_tasks.is_empty()).then(|| {
                        let any_waiting = props.ai_tasks.iter().any(|(_, _, waiting)| *waiting);
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(gpui_kit::rgb(if any_waiting {
                                palette.danger
                            } else {
                                palette.accent
                            }))
                            .into_any_element()
                    }),
                    RightPanelAction::OpenTerminal => (props.terminal_sessions > 0).then(|| {
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(gpui_kit::rgb(palette.accent))
                            .into_any_element()
                    }),
                    _ => None,
                };

                div()
                    .id(SharedString::from(format!("right-hub-card-{idx}")))
                    .flex_1()
                    .min_w_0()
                    .h(px(92.))
                    .p(px(14.))
                    .rounded(px(12.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(card_border)
                    .flex()
                    .flex_col()
                    .justify_between()
                    .cursor_pointer()
                    .hover(move |s| s.bg(card_hover))
                    .on_click(move |_, window, cx| trigger(action.clone(), window, cx))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                crate::ui::icon(item.icon)
                                    .w(px(18.))
                                    .h(px(18.))
                                    .flex_shrink_0()
                                    .text_color(icon_color),
                            )
                            .children(status_el),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(px(13.5))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.text))
                            .text_ellipsis()
                            .child(item.label),
                    )
            }));

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .px(px(2.))
                    .text_size(px(12.5))
                    .font_medium()
                    .text_color(gpui_kit::rgb(palette.muted))
                    .child(rp::HUB_SECTION_OPEN.get()),
            )
            .child(cards_row)
            .into_any_element()
    } else if items.is_empty() {
        div()
            .px(px(10.))
            .py(px(12.))
            .text_size(px(12.5))
            .text_color(gpui_kit::rgb(palette.muted))
            .child(rp::NO_MATCHING_FEATURES.get())
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                let action = item.action.clone();
                let trigger = on_action.clone();
                let is_selected = idx == selected;
                let icon_color = if item.active {
                    gpui_kit::rgb(palette.accent)
                } else {
                    gpui_kit::rgb(palette.muted)
                };
                let badge_color = if item.danger {
                    gpui_kit::rgb(palette.danger)
                } else if item.active {
                    gpui_kit::rgb(palette.accent)
                } else {
                    gpui_kit::rgb(palette.muted)
                };
                div()
                    .id(SharedString::from(format!("right-hub-item-{idx}")))
                    .h(px(34.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .cursor_pointer()
                    .when(is_selected, |row| row.bg(row_selected))
                    .hover(move |s| s.bg(row_hover))
                    .on_click(move |_, window, cx| trigger(action.clone(), window, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .min_w_0()
                            .child(
                                crate::ui::icon(item.icon)
                                    .w(px(15.))
                                    .h(px(15.))
                                    .flex_shrink_0()
                                    .text_color(icon_color),
                            )
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .text_ellipsis()
                                    .child(item.label),
                            ),
                    )
                    .when_some(item.badge, |row, badge| {
                        row.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.5))
                                .text_color(badge_color)
                                .child(badge),
                        )
                    })
            }))
            .into_any_element()
    };

    let mut task_rows: Vec<(SharedString, IconName, String, Option<(String, bool)>, RightPanelAction)> =
        Vec::new();
    if props.build_running {
        let target = if props.build_target.is_empty() {
            rp::DEFAULT_BUILD_TARGET.get().to_string()
        } else {
            props.build_target.clone()
        };
        task_rows.push((
            "hub-task-build".into(),
            IconName::RefreshCw,
            rp::compiling_target(&target),
            Some((rp::STATUS_RUNNING.get().into(), false)),
            RightPanelAction::ToggleLogs,
        ));
    }
    for (id, title, waiting) in &props.ai_tasks {
        task_rows.push((
            SharedString::from(format!("hub-task-ai-{id}")),
            IconName::Sparkles,
            format!("AI · {title}"),
            Some((
                if *waiting {
                    rp::STATUS_WAITING.get().into()
                } else {
                    rp::STATUS_GENERATING.get().into()
                },
                *waiting,
            )),
            RightPanelAction::ActivateAiTask(id.clone()),
        ));
    }
    if props.kb_syncing {
        task_rows.push((
            "hub-task-kb".into(),
            IconName::Library,
            rp::ACTION_KNOWLEDGE_BASE.get().to_owned(),
            Some((rp::BADGE_SYNCING.get().into(), false)),
            RightPanelAction::OpenKnowledgeBase,
        ));
    }
    if props.terminal_sessions > 0 {
        task_rows.push((
            "hub-task-terminal".into(),
            IconName::Terminal,
            rp::terminal_active_sessions(props.terminal_sessions),
            None,
            RightPanelAction::OpenTerminal,
        ));
    }

    let task_count = task_rows.len();

    div()
        .id("right-panel-hub")
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(gpui_kit::rgb(palette.surface))
        .border_l_1()
        .border_color(gpui_kit::rgb(palette.border))
        .overflow_y_scroll()
        .child(
            div()
                .w_full()
                .pt(px(top_pad))
                .px(px(h_pad))
                .pb(px(24.))
                .flex()
                .flex_col()
                .gap(px(22.))
                .child(search_bar)
                .child(main_section)
                .when(!has_query && task_count > 0, |col| {
                    col.child(
                        div()
                            .pt(px(4.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_medium()
                                            .text_color(gpui_kit::rgb(palette.muted))
                                            .child(rp::RUNNING_TASKS.get()),
                                    )
                                    .child(
                                        div()
                                            .px(px(6.))
                                            .py(px(1.))
                                            .rounded_full()
                                            .bg(badge_bg)
                                            .text_size(px(11.))
                                            .text_color(gpui_kit::rgb(palette.muted))
                                            .child(format!("{task_count}")),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .children(task_rows.into_iter().map(
                                        |(id, icon, title, status, action)| {
                                            let trigger = on_action.clone();
                                            div()
                                                .id(id)
                                                .h(px(32.))
                                                .px(px(10.))
                                                .rounded(px(8.))
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap_3()
                                                .cursor_pointer()
                                                .hover(move |s| s.bg(row_hover))
                                                .on_click(move |_, window, cx| {
                                                    trigger(action.clone(), window, cx)
                                                })
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(10.))
                                                        .min_w_0()
                                                        .child(
                                                            crate::ui::icon(icon)
                                                                .w(px(14.))
                                                                .h(px(14.))
                                                                .flex_shrink_0()
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.muted,
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .text_size(px(13.))
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.text,
                                                                ))
                                                                .text_ellipsis()
                                                                .child(title),
                                                        ),
                                                )
                                                .when_some(status, |row, (label, is_waiting)| {
                                                    row.child(
                                                        div()
                                                            .flex_shrink_0()
                                                            .text_size(px(11.))
                                                            .text_color(gpui_kit::rgb(
                                                                if is_waiting {
                                                                    palette.danger
                                                                } else {
                                                                    palette.accent
                                                                },
                                                            ))
                                                            .child(label),
                                                    )
                                                })
                                        },
                                    )),
                            ),
                    )
                }),
        )
        .into_any_element()
}
