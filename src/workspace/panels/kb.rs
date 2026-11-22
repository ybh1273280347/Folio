//! Right-panel Knowledge Base view: human search, light status, and explicit Accurate PDF refinement.

use super::*;
use crate::i18n::right_panel as rp;
use crate::rag::AccuratePdfMode;
use crate::rag::store::StoreStats;
use gpui_kit::base::Disableable;
use std::collections::{BTreeSet, HashSet};

pub(in crate::workspace) struct KbPanelState {
    pub(in crate::workspace) search: Entity<InputState>,
    pub(in crate::workspace) last_query: String,
    pub(in crate::workspace) searching: bool,
    pub(in crate::workspace) results: Vec<crate::rag::SearchHit>,
    pub(in crate::workspace) degraded_reason: Option<String>,
    pub(in crate::workspace) search_error: Option<String>,
    pub(in crate::workspace) sync_error: Option<String>,
    pub(in crate::workspace) stats: Option<StoreStats>,
    pub(in crate::workspace) embed_ready: Option<bool>,
    pub(in crate::workspace) accurate_configured: bool,
    pub(in crate::workspace) selected_ocr: BTreeSet<String>,
    known_ocr: HashSet<String>,
    pending_search_clear: bool,
    in_flight_search_id: u64,
}

impl KbPanelState {
    pub(in crate::workspace) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rp::KB_SEARCH_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &search,
            window,
            |app, input, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => {
                    app.submit_kb_search(cx);
                }
                InputEvent::Change => {
                    if input.read(cx).value().trim().is_empty()
                        && (!app.right_panel.kb.last_query.is_empty()
                            || !app.right_panel.kb.results.is_empty()
                            || app.right_panel.kb.search_error.is_some())
                    {
                        app.right_panel.kb.clear_search_results();
                        cx.notify();
                    }
                }
                _ => {}
            },
        )
        .detach();
        Self {
            search,
            last_query: String::new(),
            searching: false,
            results: Vec::new(),
            degraded_reason: None,
            search_error: None,
            sync_error: None,
            stats: None,
            embed_ready: None,
            accurate_configured: false,
            selected_ocr: BTreeSet::new(),
            known_ocr: HashSet::new(),
            pending_search_clear: false,
            in_flight_search_id: 0,
        }
    }

    pub(in crate::workspace) fn next_search_id(&mut self) -> u64 {
        self.in_flight_search_id = self.in_flight_search_id.wrapping_add(1);
        self.in_flight_search_id
    }

    pub(in crate::workspace) fn is_current_search(&self, id: u64) -> bool {
        self.in_flight_search_id == id
    }

    pub(in crate::workspace) fn clear_search_results(&mut self) {
        self.in_flight_search_id = self.in_flight_search_id.wrapping_add(1);
        self.last_query.clear();
        self.searching = false;
        self.results.clear();
        self.degraded_reason = None;
        self.search_error = None;
    }

    pub(in crate::workspace) fn reset_for_project(&mut self) {
        self.pending_search_clear = true;
        self.clear_search_results();
        self.sync_error = None;
        self.stats = None;
        self.embed_ready = None;
        self.selected_ocr.clear();
        self.known_ocr.clear();
    }

    pub(in crate::workspace) fn flush_pending_clear(&mut self, window: &mut Window, cx: &mut App) {
        if self.pending_search_clear {
            self.pending_search_clear = false;
            self.search.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(in crate::workspace) fn apply_stats(&mut self, stats: StoreStats) {
        let live: HashSet<String> = stats.needs_ocr_files.iter().cloned().collect();
        self.selected_ocr.retain(|p| live.contains(p));
        self.known_ocr.retain(|p| live.contains(p));
        for path in &stats.needs_ocr_files {
            if self.known_ocr.insert(path.clone()) {
                self.selected_ocr.insert(path.clone());
            }
        }
        self.stats = Some(stats);
    }

    pub(in crate::workspace) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |input, cx| {
            input.set_placeholder(rp::KB_SEARCH_PLACEHOLDER.get(), window, cx);
        });
    }
}

fn fg_alpha(hex: u32, alpha: f32) -> Rgba {
    let mut color = gpui_kit::rgb(hex);
    color.a = alpha;
    color
}

fn mix_color(base: u32, tint: u32, weight: f32) -> Rgba {
    let w = weight.clamp(0.0, 1.0);
    let mix = |shift: u32| -> f32 {
        let a = ((base >> shift) & 0xff) as f32;
        let b = ((tint >> shift) & 0xff) as f32;
        (a * (1.0 - w) + b * w) / 255.0
    };
    Rgba {
        r: mix(16),
        g: mix(8),
        b: mix(0),
        a: 1.0,
    }
}

impl Workspace {
    pub(super) fn render_knowledge_base_panel(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.settings.theme.palette;
        let header_bg = if palette.is_dark() {
            palette.background
        } else {
            palette.hover
        };
        let is_project = self.projects.directory().is_some();
        let enabled = self.kb_enabled();
        let syncing = self.kb_syncing();

        let menu_app = cx.entity();
        let header = div()
            .h(px(32.))
            .px_1p5()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .bg(gpui_kit::rgb(header_bg))
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Button::new("kb-back-to-hub")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(22.))
                            .h(px(22.))
                            .icon(crate::ui::icon(IconName::ChevronLeft))
                            .tooltip(crate::i18n::common::BACK_TO_HUB.get())
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.open_right_panel(RightPanelView::Hub, window, cx);
                            })),
                    )
                    .child(
                        crate::ui::icon(IconName::Library)
                            .w(px(14.))
                            .h(px(14.))
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.muted)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.text))
                            .text_ellipsis()
                            .child(rp::ACTION_KNOWLEDGE_BASE.get()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .when(is_project && enabled, |bar| {
                        let app_menu = menu_app.clone();
                        bar.child(
                            Button::new("kb-sync-now")
                                .ghost()
                                .compact()
                                .with_size(px(16.))
                                .w(px(22.))
                                .h(px(22.))
                                .disabled(syncing)
                                .icon(crate::ui::icon(IconName::RefreshCw))
                                .tooltip(if syncing {
                                    rp::BADGE_SYNCING.get()
                                } else {
                                    rp::KB_SYNC_TOOLTIP.get()
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.run_kb_sync(false, None, Vec::new(), cx);
                                })),
                        )
                        .child(
                            Button::new("kb-more-menu")
                                .ghost()
                                .compact()
                                .with_size(px(16.))
                                .w(px(22.))
                                .h(px(22.))
                                .disabled(syncing)
                                .icon(crate::ui::icon(IconName::Ellipsis))
                                .tooltip(rp::KB_MENU_TOOLTIP.get())
                                .dropdown_menu(move |menu, _, _| {
                                    let rebuild_app = app_menu.clone();
                                    let ocr_app = app_menu.clone();
                                    let all_app = app_menu.clone();
                                    let disable_app = app_menu.clone();
                                    menu.item(
                                        super::super::compact_menu_icon_item(
                                            IconName::RotateCcw,
                                            rp::KB_MENU_REBUILD.get(),
                                        )
                                        .on_click(move |_, _, cx| {
                                            rebuild_app.update(cx, |app, cx| {
                                                app.run_kb_sync(true, None, Vec::new(), cx);
                                            });
                                        }),
                                    )
                                    .item(
                                        super::super::compact_menu_icon_item(
                                            IconName::Sparkles,
                                            rp::KB_MENU_ACCURATE_OCR.get(),
                                        )
                                        .on_click(move |_, _, cx| {
                                            ocr_app.update(cx, |app, cx| {
                                                app.run_kb_sync(
                                                    false,
                                                    Some(AccuratePdfMode::OnlyOcr),
                                                    Vec::new(),
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .item(
                                        super::super::compact_menu_icon_item(
                                            IconName::Sparkles,
                                            rp::KB_MENU_ACCURATE_ALL.get(),
                                        )
                                        .on_click(move |_, _, cx| {
                                            all_app.update(cx, |app, cx| {
                                                app.run_kb_sync(
                                                    false,
                                                    Some(AccuratePdfMode::All),
                                                    Vec::new(),
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .separator()
                                    .item(
                                        super::super::compact_menu_icon_item(
                                            IconName::Trash,
                                            rp::KB_MENU_DISABLE.get(),
                                        )
                                        .on_click(move |_, window, cx| {
                                            disable_app.update(cx, |app, cx| {
                                                app.disable_kb(window, cx);
                                            });
                                        }),
                                    )
                                }),
                        )
                    })
                    .child(
                        Button::new("kb-maximize")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(22.))
                            .h(px(22.))
                            .icon(crate::ui::icon(IconName::Maximize2))
                            .tooltip(crate::i18n::ai::MAXIMIZE_RESTORE.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.right_panel.maximized = !app.right_panel.maximized;
                                cx.notify();
                            })),
                    ),
            );

        let body = if !is_project {
            self.render_kb_centered_state(
                IconName::FolderOpen,
                rp::KB_OPEN_PROJECT_HINT.get(),
                None,
                None,
            )
        } else if !enabled && !syncing {
            self.render_kb_centered_state(
                IconName::Library,
                rp::KB_ENABLE_TITLE.get(),
                Some(rp::KB_ENABLE_DESC.get()),
                Some(
                    Button::new("kb-enable-btn")
                        .primary()
                        .compact()
                        .h(px(30.))
                        .px_4()
                        .icon(crate::ui::icon(IconName::Sparkles).w(px(13.)).h(px(13.)))
                        .label(rp::KB_ENABLE_BTN.get())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.run_kb_sync(false, None, Vec::new(), cx);
                        }))
                        .into_any_element(),
                ),
            )
        } else {
            self.render_kb_active_body(window, cx)
        };

        div()
            .id("right-panel-kb")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(gpui_kit::rgb(palette.surface))
            .border_l_1()
            .border_color(gpui_kit::rgb(palette.border))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_kb_centered_state(
        &self,
        icon: IconName,
        title: &'static str,
        subtitle: Option<&'static str>,
        action: Option<AnyElement>,
    ) -> AnyElement {
        let palette = self.settings.theme.palette;
        div()
            .flex_1()
            .min_h_0()
            .px_6()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size(px(40.))
                    .rounded_full()
                    .bg(fg_alpha(palette.text, 0.05))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::ui::icon(icon)
                            .w(px(18.))
                            .h(px(18.))
                            .text_color(gpui_kit::rgb(palette.muted)),
                    ),
            )
            .child(
                div()
                    .text_size(px(13.5))
                    .font_medium()
                    .text_color(gpui_kit::rgb(palette.text))
                    .text_center()
                    .child(title),
            )
            .when_some(subtitle, |col, subtitle| {
                col.child(
                    div()
                        .text_size(px(12.))
                        .text_color(gpui_kit::rgb(palette.muted))
                        .text_center()
                        .child(subtitle),
                )
            })
            .when_some(action, |col, btn| col.child(div().pt_1().child(btn)))
            .into_any_element()
    }

    fn render_kb_active_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.settings.theme.palette;
        let kb = &self.right_panel.kb;
        let syncing = self.kb_syncing();
        let search_focused = kb.search.read(cx).focus_handle(cx).is_focused(window);
        let input_non_empty = !kb.search.read(cx).value().trim().is_empty();
        let has_submitted_search =
            !kb.last_query.is_empty() || kb.searching || !kb.results.is_empty() || kb.search_error.is_some();

        let ring_color = if search_focused {
            fg_alpha(palette.accent, 0.72)
        } else {
            gpui_kit::rgb(palette.border)
        };
        let outer_halo = fg_alpha(palette.accent, 0.14);
        let hover_bg = gpui_kit::rgb(palette.hover);

        let search_bar = div()
            .w_full()
            .rounded_full()
            .when(search_focused, |el| el.p(px(2.)).bg(outer_halo))
            .child(
                div()
                    .w_full()
                    .h(px(34.))
                    .px(px(12.))
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_1()
                    .border_color(ring_color)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        crate::ui::icon(IconName::Search)
                            .w(px(14.))
                            .h(px(14.))
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.muted)),
                    )
                    .child(
                        Input::new(&kb.search)
                            .appearance(false)
                            .bordered(false)
                            .flex_1()
                            .min_w_0()
                            .px_0()
                            .py_0()
                            .text_size(px(13.))
                            .text_color(gpui_kit::rgb(palette.text)),
                    )
                    .when(input_non_empty, |bar| {
                        bar.child(
                            div()
                                .id("kb-clear-search")
                                .size(px(20.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.clear_kb_search(window, cx);
                                }))
                                .child(crate::ui::icon(IconName::Close).w(px(12.)).h(px(12.))),
                        )
                    }),
            );

        let doc_count = kb.stats.as_ref().map_or(0, |s| s.indexed_files);
        let keyword_only = kb.embed_ready == Some(false)
            || (kb.degraded_reason.is_some() && !kb.results.is_empty());

        let status_strip = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_1()
            .text_size(px(11.5))
            .text_color(gpui_kit::rgb(palette.muted))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                crate::ui::icon(if syncing {
                                    IconName::RefreshCw
                                } else {
                                    IconName::FileText
                                })
                                .w(px(12.))
                                .h(px(12.))
                                .text_color(gpui_kit::rgb(if syncing {
                                    palette.accent
                                } else {
                                    palette.muted
                                })),
                            )
                            .child(if syncing {
                                rp::BADGE_SYNCING.get().to_owned()
                            } else {
                                rp::kb_doc_count(doc_count)
                            }),
                    ),
            )
            .when(keyword_only, |row| {
                let tip = kb
                    .degraded_reason
                    .clone()
                    .unwrap_or_else(|| rp::KB_KEYWORD_ONLY_TIP.get().to_owned());
                row.child(
                    Button::new("kb-semantic-settings")
                        .ghost()
                        .compact()
                        .h(px(20.))
                        .px_1p5()
                        .text_size(px(11.))
                        .icon(crate::ui::icon(IconName::ShieldAlert).w(px(11.)).h(px(11.)))
                        .label(rp::KB_KEYWORD_ONLY.get())
                        .tooltip(tip)
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.open_kb_api_settings(window, cx);
                        })),
                )
            });

        let error_banner = kb
            .sync_error
            .clone()
            .or_else(|| kb.search_error.clone());

        let content: AnyElement = if has_submitted_search {
            self.render_kb_search_results(cx)
        } else if let Some(stats) = &kb.stats
            && !stats.needs_ocr_files.is_empty()
        {
            self.render_kb_ocr_card(stats, syncing, cx)
        } else {
            self.render_kb_centered_state(
                IconName::Search,
                rp::KB_EMPTY_HINT.get(),
                None,
                None,
            )
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3p5()
                    .pt_3()
                    .pb_2()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(search_bar)
                    .child(status_strip)
                    .when_some(error_banner, |col, msg| {
                        col.child(
                            div()
                                .px_2p5()
                                .py_1p5()
                                .rounded(px(6.))
                                .bg(mix_color(palette.surface, palette.danger, 0.1))
                                .border_1()
                                .border_color(mix_color(palette.border, palette.danger, 0.35))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .min_w_0()
                                        .child(
                                            crate::ui::icon(IconName::ShieldAlert)
                                                .w(px(13.))
                                                .h(px(13.))
                                                .flex_shrink_0()
                                                .text_color(gpui_kit::rgb(palette.danger)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(gpui_kit::rgb(palette.danger))
                                                .min_w_0()
                                                .child(msg),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .flex_shrink_0()
                                        .when(!kb.accurate_configured, |actions| {
                                            actions.child(
                                                Button::new("kb-err-settings")
                                                    .ghost()
                                                    .compact()
                                                    .h(px(20.))
                                                    .px_1p5()
                                                    .text_size(px(11.))
                                                    .label(rp::KB_OPEN_SETTINGS.get())
                                                    .on_click(cx.listener(|app, _, window, cx| {
                                                        app.open_kb_api_settings(window, cx);
                                                    })),
                                            )
                                        })
                                        .child(
                                            crate::ui::control("kb-dismiss-err", IconName::Close)
                                                .on_click(cx.listener(|app, _, _, cx| {
                                                    app.right_panel.kb.sync_error = None;
                                                    app.right_panel.kb.search_error = None;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                    }),
            )
            .child(content)
            .into_any_element()
    }

    fn render_kb_search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.settings.theme.palette;
        let kb = &self.right_panel.kb;
        let card_hover = gpui_kit::rgb(palette.hover);
        let badge_bg = fg_alpha(palette.text, 0.06);

        if kb.searching && kb.results.is_empty() {
            return self.render_kb_centered_state(
                IconName::RefreshCw,
                rp::KB_SEARCHING.get(),
                None,
                None,
            );
        }
        if kb.results.is_empty() {
            return self.render_kb_centered_state(
                IconName::Search,
                rp::KB_NO_RESULTS.get(),
                None,
                None,
            );
        }

        div()
            .id("kb-results-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1p5()
            .children(kb.results.iter().enumerate().map(|(idx, hit)| {
                let open_path = hit.path.clone();
                let clean_path = hit.path.strip_suffix("#annotations").unwrap_or(&hit.path);
                let file_name = clean_path.rsplit('/').next().unwrap_or(clean_path).to_owned();
                let file_icon = match hit.kind.as_str() {
                    "pdf" => IconName::Library,
                    "annotation" => IconName::NotebookPen,
                    _ => IconName::FileText,
                };
                let loc = match (hit.page, hit.line_start) {
                    (Some(page), _) => Some(format!("p.{page}")),
                    (None, Some(line)) => Some(format!("L{line}")),
                    _ => None,
                };
                let extra_tag = if hit.dirty_buffer {
                    Some(rp::KB_DRAFT_BADGE.get())
                } else if hit.kind == "annotation" {
                    Some(rp::KB_ANNOT_BADGE.get())
                } else {
                    None
                };
                let is_accurate = hit.parse_tier == "mineru";

                div()
                    .id(SharedString::from(format!("kb-hit-{idx}")))
                    .p_2p5()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .bg(gpui_kit::rgb(palette.surface))
                    .cursor_pointer()
                    .hover(move |s| s.bg(card_hover))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.open_kb_result(&open_path, window, cx);
                    }))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .min_w_0()
                                    .child(
                                        crate::ui::icon(file_icon)
                                            .w(px(13.))
                                            .h(px(13.))
                                            .flex_shrink_0()
                                            .text_color(gpui_kit::rgb(palette.muted)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_medium()
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .text_ellipsis()
                                            .child(file_name),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .flex_shrink_0()
                                    .when(is_accurate, |row| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(fg_alpha(palette.accent, 0.12))
                                                .flex()
                                                .items_center()
                                                .gap(px(3.))
                                                .text_size(px(10.5))
                                                .text_color(gpui_kit::rgb(palette.accent))
                                                .child(
                                                    crate::ui::icon(IconName::Sparkles)
                                                        .w(px(10.))
                                                        .h(px(10.)),
                                                )
                                                .child(rp::KB_ACCURATE_BADGE.get()),
                                        )
                                    })
                                    .when_some(extra_tag, |row, tag| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(badge_bg)
                                                .text_size(px(10.5))
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .child(tag),
                                        )
                                    })
                                    .when_some(loc, |row, loc| {
                                        row.child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .child(loc),
                                        )
                                    }),
                            ),
                    )
                    .when(hit.heading != clean_path, |card| {
                        card.child(
                            div()
                                .text_size(px(11.))
                                .text_color(gpui_kit::rgb(palette.muted))
                                .text_ellipsis()
                                .child(hit.heading.clone()),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(relative(1.45))
                            .text_color(fg_alpha(palette.text, 0.84))
                            .child(hit.snippet.clone()),
                    )
            }))
            .into_any_element()
    }

    fn render_kb_ocr_card(
        &self,
        stats: &StoreStats,
        syncing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.settings.theme.palette;
        let kb = &self.right_panel.kb;
        let row_hover = gpui_kit::rgb(palette.hover);
        let selected_count = kb.selected_ocr.len();

        div()
            .id("kb-ocr-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3p5()
            .pt_1()
            .pb_4()
            .child(
                div()
                    .p_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .bg(fg_alpha(palette.text, 0.02))
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        crate::ui::icon(IconName::ShieldAlert)
                                            .w(px(14.))
                                            .h(px(14.))
                                            .text_color(gpui_kit::rgb(palette.accent)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_medium()
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .child(rp::kb_ocr_heading(stats.needs_ocr_files.len())),
                                    ),
                            )
                            .child(
                                Button::new("kb-refine-ocr-btn")
                                    .primary()
                                    .compact()
                                    .h(px(24.))
                                    .px_2p5()
                                    .text_size(px(11.5))
                                    .disabled(syncing || selected_count == 0)
                                    .icon(
                                        crate::ui::icon(IconName::Sparkles)
                                            .w(px(11.))
                                            .h(px(11.)),
                                    )
                                    .label(rp::kb_refine_selected(selected_count))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        let paths: Vec<String> = app
                                            .right_panel
                                            .kb
                                            .selected_ocr
                                            .iter()
                                            .cloned()
                                            .collect();
                                        if !paths.is_empty() {
                                            app.run_kb_sync(
                                                false,
                                                Some(AccuratePdfMode::OnlyOcr),
                                                paths,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(stats.needs_ocr_files.iter().enumerate().map(|(idx, rel_path)| {
                                let checked = kb.selected_ocr.contains(rel_path);
                                let toggle_path = rel_path.clone();
                                let open_path = rel_path.clone();
                                let file_name = rel_path.rsplit('/').next().unwrap_or(rel_path).to_owned();
                                div()
                                    .id(SharedString::from(format!("kb-ocr-item-{idx}")))
                                    .h(px(26.))
                                    .px_1p5()
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .hover(move |s| s.bg(row_hover))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("kb-ocr-check-{idx}")))
                                            .size(px(15.))
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(gpui_kit::rgb(if checked {
                                                palette.accent
                                            } else {
                                                palette.border
                                            }))
                                            .when(checked, |b| b.bg(gpui_kit::rgb(palette.accent)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .flex_shrink_0()
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                cx.stop_propagation();
                                                if !app.right_panel.kb.selected_ocr.remove(&toggle_path) {
                                                    app.right_panel.kb.selected_ocr.insert(toggle_path.clone());
                                                }
                                                cx.notify();
                                            }))
                                            .when(checked, |b| {
                                                b.child(
                                                    crate::ui::icon(IconName::Check)
                                                        .w(px(10.))
                                                        .h(px(10.))
                                                        .text_color(gpui_kit::rgb(palette.surface)),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("kb-ocr-open-{idx}")))
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .text_ellipsis()
                                            .cursor_pointer()
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    open_path.clone(),
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener({
                                                let rel_path = rel_path.clone();
                                                move |app, _, window, cx| {
                                                    app.open_kb_result(&rel_path, window, cx);
                                                }
                                            }))
                                            .child(file_name),
                                    )
                            })),
                    ),
            )
            .into_any_element()
    }
}
