//! Right-panel Knowledge Base view: fast keyword search, book-style result cards,
//! jump-to-location with temporary highlight, and explicit PaddleOCR refinement.

use super::*;
use crate::i18n::right_panel as rp;
use crate::rag::store::{PdfLibraryItem, StoreStats};
use crate::rag::{SearchHit, pipeline::clean_heading_text};
use gpui_kit::base::Disableable;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) enum KbFilter {
    #[default]
    All,
    Notes,
    Pdf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) enum PdfManagerFilter {
    #[default]
    All,
    NeedsOcr,
    Fast,
    Accurate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum PdfOcrPhase {
    Queued,
    Running {
        extracted_pages: usize,
        total_pages: Option<usize>,
    },
}

pub(in crate::workspace) struct KbPanelState {
    pub(in crate::workspace) search: Entity<InputState>,
    pub(in crate::workspace) last_query: String,
    pub(in crate::workspace) searching: bool,
    pub(in crate::workspace) results: Vec<SearchHit>,
    pub(in crate::workspace) filter: KbFilter,
    pub(in crate::workspace) expanded_files: HashSet<String>,
    pub(in crate::workspace) search_error: Option<String>,
    pub(in crate::workspace) sync_error: Option<String>,
    pub(in crate::workspace) stats: Option<StoreStats>,
    pub(in crate::workspace) accurate_configured: bool,
    pub(in crate::workspace) pdf_manager_open: bool,
    pub(in crate::workspace) pdf_filter: PdfManagerFilter,
    pub(in crate::workspace) pdf_items: Vec<PdfLibraryItem>,
    pub(in crate::workspace) selected_pdfs: BTreeSet<String>,
    pub(in crate::workspace) ocr_progress: HashMap<String, PdfOcrPhase>,
    pub(in crate::workspace) ocr_error: Option<String>,
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
            filter: KbFilter::All,
            expanded_files: HashSet::new(),
            search_error: None,
            sync_error: None,
            stats: None,
            accurate_configured: false,
            pdf_manager_open: false,
            pdf_filter: PdfManagerFilter::All,
            pdf_items: Vec::new(),
            selected_pdfs: BTreeSet::new(),
            ocr_progress: HashMap::new(),
            ocr_error: None,
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
        self.expanded_files.clear();
        self.search_error = None;
    }

    pub(in crate::workspace) fn reset_for_project(&mut self) {
        self.pending_search_clear = true;
        self.clear_search_results();
        self.filter = KbFilter::All;
        self.sync_error = None;
        self.stats = None;
        self.pdf_manager_open = false;
        self.pdf_filter = PdfManagerFilter::All;
        self.pdf_items.clear();
        self.selected_pdfs.clear();
        self.ocr_progress.clear();
        self.ocr_error = None;
    }

    pub(in crate::workspace) fn flush_pending_clear(&mut self, window: &mut Window, cx: &mut App) {
        if self.pending_search_clear {
            self.pending_search_clear = false;
            self.search.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(in crate::workspace) fn apply_snapshot(
        &mut self,
        stats: StoreStats,
        pdf_items: Vec<PdfLibraryItem>,
    ) {
        let live_pdfs: HashSet<&str> = pdf_items.iter().map(|i| i.rel_path.as_str()).collect();
        self.selected_pdfs.retain(|p| live_pdfs.contains(p.as_str()));
        self.pdf_items = pdf_items;
        self.stats = Some(stats);
    }

    pub(in crate::workspace) fn red_dot_count(&self) -> usize {
        if !self.pdf_items.is_empty() {
            self.pdf_items
                .iter()
                .filter(|item| item.status == "needs_ocr" || item.last_error.is_some())
                .count()
        } else {
            self.stats.as_ref().map_or(0, |s| s.needs_ocr_files.len())
        }
    }

    pub(in crate::workspace) fn has_ocr_red_dot(&self) -> bool {
        self.red_dot_count() > 0
    }

    pub(in crate::workspace) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |input, cx| {
            input.set_placeholder(rp::KB_SEARCH_PLACEHOLDER.get(), window, cx);
        });
    }
}

struct FileHitGroup<'a> {
    clean_path: &'a str,
    file_name: String,
    dir_hint: Option<String>,
    is_pdf: bool,
    is_accurate: bool,
    hits: Vec<&'a SearchHit>,
}

fn group_hits_by_file<'a>(hits: &'a [SearchHit], filter: KbFilter) -> Vec<FileHitGroup<'a>> {
    let mut order: Vec<String> = Vec::new();
    let mut by_file: BTreeMap<String, Vec<&'a SearchHit>> = BTreeMap::new();
    for hit in hits {
        let clean = hit.path.strip_suffix("#annotations").unwrap_or(&hit.path);
        let is_pdf = clean.to_ascii_lowercase().ends_with(".pdf");
        let keep = match filter {
            KbFilter::All => true,
            KbFilter::Notes => !is_pdf,
            KbFilter::Pdf => is_pdf,
        };
        if !keep {
            continue;
        }
        if !by_file.contains_key(clean) {
            order.push(clean.to_owned());
        }
        by_file.entry(clean.to_owned()).or_default().push(hit);
    }

    order
        .into_iter()
        .filter_map(|clean_key| {
            let file_hits = by_file.remove(&clean_key)?;
            let first = file_hits[0];
            let clean_path = first.path.strip_suffix("#annotations").unwrap_or(&first.path);
            let file_name = clean_path.rsplit('/').next().unwrap_or(clean_path).to_owned();
            let dir_hint = middle_ellipsis_dir(clean_path);
            let is_pdf = clean_path.to_ascii_lowercase().ends_with(".pdf");
            let is_accurate = file_hits
                .iter()
                .any(|h| matches!(h.parse_tier.as_str(), "accurate" | "mineru"));
            Some(FileHitGroup {
                clean_path,
                file_name,
                dir_hint,
                is_pdf,
                is_accurate,
                hits: file_hits,
            })
        })
        .collect()
}

fn middle_ellipsis_dir(rel_path: &str) -> Option<String> {
    let (dir, _) = rel_path.rsplit_once('/')?;
    if dir.is_empty() {
        return None;
    }
    if dir.chars().count() <= 24 {
        return Some(dir.to_owned());
    }
    let parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 3 {
        return Some(format!("{}/…/{}", parts[0], parts[parts.len() - 1]));
    }
    let head: String = dir.chars().take(10).collect();
    let tail: String = dir
        .chars()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(format!("{head}…{tail}"))
}

fn section_subtitle(hit: &SearchHit, file_name: &str) -> Option<String> {
    let raw = hit.heading.rsplit(" > ").next()?.trim();
    let cleaned = clean_heading_text(raw);
    if cleaned.is_empty() || cleaned == file_name || cleaned == hit.path {
        return None;
    }
    if let Some(page) = hit.page
        && cleaned == format!("p.{page}")
    {
        return None;
    }
    Some(cleaned)
}

fn build_highlighted_text(
    text: &str,
    query: &str,
    base_style: TextStyle,
    highlight_bg: Hsla,
    highlight_fg: Hsla,
) -> StyledText {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() || text.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()))
            .with_runs(vec![base_style.to_run(text.len())]);
    }

    let lower = text.to_ascii_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for tok in &tokens {
        let mut cursor = 0usize;
        while let Some(pos) = lower[cursor..].find(tok.as_str()) {
            let start = cursor + pos;
            let end = start + tok.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                ranges.push((start, end));
            }
            cursor = end.max(cursor + 1);
        }
    }
    if ranges.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()))
            .with_runs(vec![base_style.to_run(text.len())]);
    }

    ranges.sort_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }

    let hi_style = HighlightStyle {
        background_color: Some(highlight_bg),
        color: Some(highlight_fg),
        ..Default::default()
    };
    let mut runs = Vec::with_capacity(merged.len() * 2 + 1);
    let mut pos = 0usize;
    for (s, e) in merged {
        if s > pos {
            runs.push(base_style.clone().to_run(s - pos));
        }
        runs.push(base_style.clone().highlight(hi_style).to_run(e - s));
        pos = e;
    }
    if pos < text.len() {
        runs.push(base_style.to_run(text.len() - pos));
    }

    StyledText::new(SharedString::from(text.to_owned())).with_runs(runs)
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
        if self.right_panel.kb.pdf_manager_open && self.projects.directory().is_some() && self.kb_enabled() {
            return self.render_pdf_manager_panel(cx);
        }
        let palette = self.settings.theme.palette;
        let header_bg = if palette.is_dark() {
            palette.background
        } else {
            palette.hover
        };
        let is_project = self.projects.directory().is_some();
        let enabled = self.kb_enabled();
        let syncing = self.kb_syncing();
        let kb = &self.right_panel.kb;
        let ocr_active = !kb.ocr_progress.is_empty();
        let has_red_dot = kb.has_ocr_red_dot();

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
                        let sync_icon = if syncing {
                            crate::ui::icon(IconName::LoaderCircle)
                                .text_color(gpui_kit::rgb(palette.accent))
                                .with_animation(
                                    "kb-header-sync-spin",
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    |icon, t| {
                                        icon.rotate(gpui::radians(t * std::f32::consts::TAU))
                                    },
                                )
                                .into_any_element()
                        } else {
                            crate::ui::icon(IconName::FolderSync).into_any_element()
                        };
                        let pdf_mgr_icon = if ocr_active {
                            crate::ui::icon(IconName::LoaderCircle)
                                .text_color(gpui_kit::rgb(palette.accent))
                                .with_animation(
                                    "kb-header-ocr-spin",
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    |icon, t| {
                                        icon.rotate(gpui::radians(t * std::f32::consts::TAU))
                                    },
                                )
                                .into_any_element()
                        } else {
                            crate::ui::icon(IconName::BookOpenText).into_any_element()
                        };
                        bar.child(
                            div()
                                .id("kb-sync-now")
                                .size(px(22.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(!syncing, |el| {
                                    el.cursor_pointer()
                                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.run_kb_sync(false, None, Vec::new(), cx);
                                        }))
                                })
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(if syncing {
                                        rp::BADGE_SYNCING.get()
                                    } else {
                                        rp::KB_SYNC_TOOLTIP.get()
                                    })
                                    .build(window, cx)
                                })
                                .child(sync_icon),
                        )
                        .child(
                            div()
                                .id("kb-open-pdf-manager")
                                .relative()
                                .size(px(22.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        rp::KB_PDF_MANAGER_TOOLTIP.get(),
                                    )
                                    .build(window, cx)
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.right_panel.kb.pdf_manager_open = true;
                                    app.refresh_kb_snapshot(cx);
                                    cx.notify();
                                }))
                                .child(pdf_mgr_icon)
                                .when(has_red_dot, |btn| {
                                    btn.child(
                                        div()
                                            .absolute()
                                            .top(px(2.))
                                            .right(px(2.))
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(0xef4444)),
                                    )
                                }),
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
                false,
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
                false,
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
        spin: bool,
    ) -> AnyElement {
        let palette = self.settings.theme.palette;
        let icon_el = if spin {
            crate::ui::icon(icon)
                .w(px(18.))
                .h(px(18.))
                .text_color(gpui_kit::rgb(palette.accent))
                .with_animation(
                    "kb-center-spin",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |el, t| el.rotate(gpui::radians(t * std::f32::consts::TAU)),
                )
                .into_any_element()
        } else {
            crate::ui::icon(icon)
                .w(px(18.))
                .h(px(18.))
                .text_color(gpui_kit::rgb(palette.muted))
                .into_any_element()
        };

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
                    .child(icon_el),
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

        let search_prefix = if kb.searching {
            crate::ui::icon(IconName::LoaderCircle)
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0()
                .text_color(gpui_kit::rgb(palette.accent))
                .with_animation(
                    "kb-search-spin",
                    Animation::new(Duration::from_millis(850)).repeat(),
                    |el, t| el.rotate(gpui::radians(t * std::f32::consts::TAU)),
                )
                .into_any_element()
        } else {
            crate::ui::icon(IconName::Search)
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0()
                .text_color(gpui_kit::rgb(palette.muted))
                .into_any_element()
        };

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
                    .child(search_prefix)
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

        let groups = group_hits_by_file(&kb.results, kb.filter);
        let filtered_hit_count: usize = groups.iter().map(|g| g.hits.len()).sum();
        let filtered_doc_count = groups.len();
        let total_doc_count = kb.stats.as_ref().map_or(0, |s| s.indexed_files);

        let chip = |id: &'static str, filter_val: KbFilter, icon: Option<IconName>, label: &'static str| {
            let active = kb.filter == filter_val;
            div()
                .id(id)
                .h(px(22.))
                .px_2()
                .rounded_full()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(fg_alpha(palette.accent, 0.14))
                        .text_color(gpui_kit::rgb(palette.accent))
                        .font_medium()
                })
                .when(!active, |el| {
                    el.text_color(gpui_kit::rgb(palette.muted))
                        .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                })
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.right_panel.kb.filter = filter_val;
                    cx.notify();
                }))
                .when_some(icon, |el, ic| {
                    el.child(crate::ui::icon(ic).w(px(11.)).h(px(11.)))
                })
                .child(label)
        };

        let status_text = if kb.searching {
            rp::KB_SEARCHING.get().to_owned()
        } else if has_submitted_search {
            rp::kb_search_summary(filtered_hit_count, filtered_doc_count, kb.results.len() >= 20)
        } else if syncing {
            rp::BADGE_SYNCING.get().to_owned()
        } else {
            rp::kb_doc_count(total_doc_count)
        };

        let status_strip = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(has_submitted_search, |row| {
                        row.child(chip("kb-filter-all", KbFilter::All, None, rp::KB_FILTER_ALL.get()))
                            .child(chip(
                                "kb-filter-notes",
                                KbFilter::Notes,
                                Some(IconName::FileText),
                                rp::KB_FILTER_NOTES.get(),
                            ))
                            .child(chip(
                                "kb-filter-pdf",
                                KbFilter::Pdf,
                                Some(IconName::Library),
                                rp::KB_FILTER_PDF.get(),
                            ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(gpui_kit::rgb(if syncing && !has_submitted_search {
                        palette.accent
                    } else {
                        palette.muted
                    }))
                    .child(
                        crate::ui::icon(if syncing && !has_submitted_search {
                            IconName::LoaderCircle
                        } else {
                            IconName::FileText
                        })
                        .w(px(11.))
                        .h(px(11.)),
                    )
                    .child(status_text),
            );

        let error_banner = kb
            .sync_error
            .clone()
            .or_else(|| kb.search_error.clone());

        let content: AnyElement = if has_submitted_search {
            self.render_kb_search_results(groups, window, cx)
        } else {
            self.render_kb_centered_state(
                IconName::Search,
                rp::KB_EMPTY_HINT.get(),
                None,
                None,
                false,
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

    fn render_kb_search_results(
        &self,
        groups: Vec<FileHitGroup<'_>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kb = &self.right_panel.kb;
        if kb.searching && groups.is_empty() {
            return self.render_kb_centered_state(
                IconName::LoaderCircle,
                rp::KB_SEARCHING.get(),
                None,
                None,
                true,
            );
        }
        if groups.is_empty() {
            return self.render_kb_centered_state(
                IconName::Search,
                rp::KB_NO_RESULTS.get(),
                None,
                None,
                false,
            );
        }

        let two_col = self.right_panel.maximized || self.settings.terminal.panel_width >= 440.0;
        let palette = self.settings.theme.palette;
        let card_hover = gpui_kit::rgb(palette.hover);
        let badge_bg = fg_alpha(palette.text, 0.06);
        let hi_bg = fg_alpha(palette.accent, 0.20).into();
        let hi_fg = gpui_kit::rgb(palette.text).into();

        let mut snippet_style = window.text_style();
        snippet_style.font_family = crate::ui::sans_font(&self.settings.interface_font).into();
        snippet_style.font_size = px(11.5).into();
        snippet_style.color = fg_alpha(palette.text, 0.85).into();

        let cards: Vec<AnyElement> = groups
            .into_iter()
            .enumerate()
            .map(|(group_idx, group)| {
                let clean_path_owned = group.clean_path.to_owned();
                let expanded = kb.expanded_files.contains(group.clean_path);
                let visible_limit = if expanded { group.hits.len() } else { 2 };
                let extra_count = group.hits.len().saturating_sub(visible_limit);
                let first_hit = group.hits[0];
                let spine_color = if group.is_pdf {
                    gpui_kit::rgb(palette.accent)
                } else {
                    fg_alpha(palette.text, 0.24)
                };
                let file_icon = if group.is_pdf {
                    IconName::Library
                } else if first_hit.kind == "annotation" {
                    IconName::NotebookPen
                } else {
                    IconName::FileText
                };

                let header_open_path = first_hit.path.clone();
                let header_line = first_hit.line_start;
                let header_page = first_hit.page;
                let header_heading = first_hit.heading.clone();
                let header_snippet = first_hit.snippet.clone();

                let mut card = div()
                    .id(SharedString::from(format!("kb-book-{group_idx}")))
                    .flex_1()
                    .min_w_0()
                    .min_h(px(142.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .bg(gpui_kit::rgb(palette.surface))
                    .overflow_hidden()
                    .flex()
                    .child(div().w(px(3.)).flex_shrink_0().bg(spine_color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_2p5()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .child(
                                div()
                                    .id(SharedString::from(format!("kb-book-head-{group_idx}")))
                                    .cursor_pointer()
                                    .rounded(px(4.))
                                    .px_1()
                                    .py_0p5()
                                    .hover(move |s| s.bg(card_hover))
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.open_kb_result(
                                            &header_open_path,
                                            header_line,
                                            header_page,
                                            Some(&header_heading),
                                            Some(&header_snippet),
                                            window,
                                            cx,
                                        );
                                    }))
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_1p5()
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
                                                            .text_color(if group.is_pdf {
                                                                gpui_kit::rgb(palette.accent)
                                                            } else {
                                                                gpui_kit::rgb(palette.muted)
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.5))
                                                            .font_medium()
                                                            .text_color(gpui_kit::rgb(palette.text))
                                                            .text_ellipsis()
                                                            .child(group.file_name.clone()),
                                                    ),
                                            )
                                            .when(group.is_accurate, |row| {
                                                row.child(
                                                    div()
                                                        .id(SharedString::from(format!("kb-acc-{group_idx}")))
                                                        .flex_shrink_0()
                                                        .text_color(gpui_kit::rgb(palette.accent))
                                                        .tooltip(|window, cx| {
                                                            gpui_kit::component::tooltip::Tooltip::new(
                                                                rp::KB_ACCURATE_BADGE.get(),
                                                            )
                                                            .build(window, cx)
                                                        })
                                                        .child(
                                                            crate::ui::icon(IconName::Sparkles)
                                                                .w(px(11.))
                                                                .h(px(11.)),
                                                        ),
                                                )
                                            }),
                                    )
                                    .when_some(group.dir_hint.clone(), |head, dir| {
                                        head.child(
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(fg_alpha(palette.muted, 0.85))
                                                .text_ellipsis()
                                                .child(dir),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .children(group.hits.iter().take(visible_limit).enumerate().map(
                                        |(hit_idx, hit)| {
                                            let open_path = hit.path.clone();
                                            let open_line = hit.line_start;
                                            let open_page = hit.page;
                                            let open_heading = hit.heading.clone();
                                            let open_snippet = hit.snippet.clone();
                                            let subtitle = section_subtitle(hit, &group.file_name);
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
                                            let styled_snippet = build_highlighted_text(
                                                &hit.snippet,
                                                &kb.last_query,
                                                snippet_style.clone(),
                                                hi_bg,
                                                hi_fg,
                                            );

                                            div()
                                                .id(SharedString::from(format!(
                                                    "kb-hit-{group_idx}-{hit_idx}"
                                                )))
                                                .px_1p5()
                                                .py_1()
                                                .rounded(px(6.))
                                                .bg(fg_alpha(palette.text, 0.025))
                                                .cursor_pointer()
                                                .hover(move |s| s.bg(card_hover))
                                                .on_click(cx.listener(move |app, _, window, cx| {
                                                    app.open_kb_result(
                                                        &open_path,
                                                        open_line,
                                                        open_page,
                                                        Some(&open_heading),
                                                        Some(&open_snippet),
                                                        window,
                                                        cx,
                                                    );
                                                }))
                                                .flex()
                                                .flex_col()
                                                .gap_0p5()
                                                .when(
                                                    subtitle.is_some()
                                                        || loc.is_some()
                                                        || extra_tag.is_some(),
                                                    |row| {
                                                        row.child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .justify_between()
                                                                .gap_1p5()
                                                                .child(
                                                                    div()
                                                                        .flex_1()
                                                                        .min_w_0()
                                                                        .text_size(px(11.))
                                                                        .text_color(fg_alpha(
                                                                            palette.text,
                                                                            0.68,
                                                                        ))
                                                                        .text_ellipsis()
                                                                        .children(subtitle),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .flex()
                                                                        .items_center()
                                                                        .gap_1()
                                                                        .flex_shrink_0()
                                                                        .when_some(
                                                                            extra_tag,
                                                                            |r, tag| {
                                                                                r.child(
                                                                                    div()
                                                                                        .px_1()
                                                                                        .rounded(px(
                                                                                            3.,
                                                                                        ))
                                                                                        .bg(badge_bg)
                                                                                        .text_size(px(
                                                                                            10.,
                                                                                        ))
                                                                                        .text_color(gpui_kit::rgb(palette.muted))
                                                                                        .child(tag),
                                                                                )
                                                                            },
                                                                        )
                                                                        .when_some(loc, |r, loc| {
                                                                            r.child(
                                                                                div()
                                                                                    .text_size(px(
                                                                                        10.5,
                                                                                    ))
                                                                                    .text_color(fg_alpha(
                                                                                        palette.muted,
                                                                                        0.85,
                                                                                    ))
                                                                                    .child(loc),
                                                                            )
                                                                        }),
                                                                ),
                                                        )
                                                    },
                                                )
                                                .child(
                                                    div()
                                                        .max_h(px(52.))
                                                        .overflow_hidden()
                                                        .line_height(relative(1.42))
                                                        .child(styled_snippet),
                                                )
                                        },
                                    )),
                            ),
                    );

                if extra_count > 0 || expanded {
                    let toggle_path = clean_path_owned;
                    let footer_label = if expanded {
                        rp::KB_COLLAPSE_HITS.get().to_owned()
                    } else {
                        rp::kb_more_hits(extra_count)
                    };
                    card = div()
                        .id(SharedString::from(format!("kb-book-wrap-{group_idx}")))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(card)
                        .child(
                            div()
                                .id(SharedString::from(format!("kb-book-more-{group_idx}")))
                                .px_2()
                                .py_0p5()
                                .text_size(px(11.))
                                .text_color(gpui_kit::rgb(palette.accent))
                                .cursor_pointer()
                                .hover(move |s| s.bg(card_hover))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if !app.right_panel.kb.expanded_files.remove(&toggle_path) {
                                        app.right_panel
                                            .kb
                                            .expanded_files
                                            .insert(toggle_path.clone());
                                    }
                                    cx.notify();
                                }))
                                .child(footer_label),
                        );
                }

                card.into_any_element()
            })
            .collect();

        let rows: AnyElement = if two_col {
            let mut row_els: Vec<AnyElement> = Vec::new();
            let mut iter = cards.into_iter();
            while let Some(first) = iter.next() {
                let second = iter
                    .next()
                    .unwrap_or_else(|| div().flex_1().min_w_0().into_any_element());
                row_els.push(
                    div()
                        .flex()
                        .items_stretch()
                        .gap_2()
                        .child(first)
                        .child(second)
                        .into_any_element(),
                );
            }
            div().flex().flex_col().gap_2().children(row_els).into_any_element()
        } else {
            div().flex().flex_col().gap_2().children(cards).into_any_element()
        };

        div()
            .id("kb-results-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .pb_4()
            .child(rows)
            .into_any_element()
    }

    fn render_pdf_manager_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.settings.theme.palette;
        let header_bg = if palette.is_dark() {
            palette.background
        } else {
            palette.hover
        };
        let hover_bg = gpui_kit::rgb(palette.hover);
        let kb = &self.right_panel.kb;
        let syncing = self.kb_syncing();
        let ocr_active = !kb.ocr_progress.is_empty();

        let is_blocked = |item: &PdfLibraryItem| item.status == "needs_ocr" || item.last_error.is_some();
        let is_accurate = |item: &PdfLibraryItem| matches!(item.parse_tier.as_str(), "accurate" | "mineru");

        let total_count = kb.pdf_items.len();
        let red_count = kb.pdf_items.iter().filter(|i| is_blocked(i)).count();
        let acc_count = kb.pdf_items.iter().filter(|i| is_accurate(i)).count();
        let fast_count = kb
            .pdf_items
            .iter()
            .filter(|i| !is_blocked(i) && !is_accurate(i))
            .count();

        let filtered: Vec<&PdfLibraryItem> = kb
            .pdf_items
            .iter()
            .filter(|item| match kb.pdf_filter {
                PdfManagerFilter::All => true,
                PdfManagerFilter::NeedsOcr => is_blocked(item),
                PdfManagerFilter::Fast => !is_blocked(item) && !is_accurate(item),
                PdfManagerFilter::Accurate => is_accurate(item),
            })
            .collect();

        let filtered_paths: Vec<String> = filtered.iter().map(|i| i.rel_path.clone()).collect();
        let all_visible_selected = !filtered.is_empty()
            && filtered
                .iter()
                .all(|item| kb.selected_pdfs.contains(&item.rel_path));
        let selected_count = kb.selected_pdfs.len();
        let selected_accurate_count = kb
            .pdf_items
            .iter()
            .filter(|i| kb.selected_pdfs.contains(&i.rel_path) && is_accurate(i))
            .count();

        let only_ocr_targets: Vec<String> = {
            let selected_blocked: Vec<String> = kb
                .pdf_items
                .iter()
                .filter(|i| kb.selected_pdfs.contains(&i.rel_path) && is_blocked(i))
                .map(|i| i.rel_path.clone())
                .collect();
            if !selected_blocked.is_empty() {
                selected_blocked
            } else {
                kb.pdf_items
                    .iter()
                    .filter(|i| is_blocked(i))
                    .map(|i| i.rel_path.clone())
                    .collect()
            }
        };

        let sync_icon = if syncing {
            crate::ui::icon(IconName::LoaderCircle)
                .text_color(gpui_kit::rgb(palette.accent))
                .with_animation(
                    "kb-pdf-mgr-sync-spin",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |icon, t| icon.rotate(gpui::radians(t * std::f32::consts::TAU)),
                )
                .into_any_element()
        } else {
            crate::ui::icon(IconName::FolderSync).into_any_element()
        };

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
                        Button::new("kb-pdf-mgr-back")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(22.))
                            .h(px(22.))
                            .icon(crate::ui::icon(IconName::ChevronLeft))
                            .tooltip(rp::KB_BACK_TO_SEARCH.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.right_panel.kb.pdf_manager_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::ui::icon(IconName::BookOpenText)
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
                            .child(rp::KB_PDF_MANAGER_TITLE.get()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        div()
                            .id("kb-pdf-mgr-sync")
                            .size(px(22.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(!syncing, |el| {
                                el.cursor_pointer()
                                    .hover(move |s| s.bg(hover_bg))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.run_kb_sync(false, None, Vec::new(), cx);
                                    }))
                            })
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if syncing {
                                    rp::BADGE_SYNCING.get()
                                } else {
                                    rp::KB_SYNC_TOOLTIP.get()
                                })
                                .build(window, cx)
                            })
                            .child(sync_icon),
                    )
                    .child(
                        Button::new("kb-pdf-mgr-maximize")
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

        let filter_chip = |id: &'static str,
                           val: PdfManagerFilter,
                           prefix: AnyElement,
                           label: &'static str,
                           count: usize| {
            let active = kb.pdf_filter == val;
            div()
                .id(id)
                .h(px(22.))
                .px_2()
                .rounded_full()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(fg_alpha(palette.accent, 0.14))
                        .text_color(gpui_kit::rgb(palette.accent))
                        .font_medium()
                })
                .when(!active, |el| {
                    el.text_color(gpui_kit::rgb(palette.muted))
                        .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                })
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.right_panel.kb.pdf_filter = val;
                    cx.notify();
                }))
                .child(prefix)
                .child(format!("{label} {count}"))
        };

        let controls = div()
            .px_3p5()
            .pt_2p5()
            .pb_1p5()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_1()
                    .child(filter_chip(
                        "pdf-f-all",
                        PdfManagerFilter::All,
                        div().into_any_element(),
                        rp::KB_FILTER_ALL.get(),
                        total_count,
                    ))
                    .child(filter_chip(
                        "pdf-f-ocr",
                        PdfManagerFilter::NeedsOcr,
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(gpui_kit::rgb(0xef4444))
                            .into_any_element(),
                        rp::KB_PDF_FILTER_NEEDS_OCR.get(),
                        red_count,
                    ))
                    .child(filter_chip(
                        "pdf-f-fast",
                        PdfManagerFilter::Fast,
                        crate::ui::icon(IconName::Zap)
                            .w(px(10.))
                            .h(px(10.))
                            .into_any_element(),
                        rp::KB_PDF_FILTER_FAST.get(),
                        fast_count,
                    ))
                    .child(filter_chip(
                        "pdf-f-acc",
                        PdfManagerFilter::Accurate,
                        crate::ui::icon(IconName::Sparkles)
                            .w(px(10.))
                            .h(px(10.))
                            .into_any_element(),
                        rp::KB_PDF_FILTER_ACCURATE.get(),
                        acc_count,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .child(
                        div()
                            .id("pdf-select-all")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(gpui_kit::rgb(palette.muted))
                            .hover(move |s| s.text_color(gpui_kit::rgb(palette.text)))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                let kb = &mut app.right_panel.kb;
                                if all_visible_selected {
                                    for p in &filtered_paths {
                                        kb.selected_pdfs.remove(p);
                                    }
                                } else {
                                    for p in &filtered_paths {
                                        kb.selected_pdfs.insert(p.clone());
                                    }
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(3.5))
                                    .border_1()
                                    .border_color(gpui_kit::rgb(if all_visible_selected {
                                        palette.accent
                                    } else {
                                        palette.border
                                    }))
                                    .when(all_visible_selected, |b| b.bg(gpui_kit::rgb(palette.accent)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(all_visible_selected, |b| {
                                        b.child(
                                            crate::ui::icon(IconName::Check)
                                                .w(px(9.))
                                                .h(px(9.))
                                                .text_color(gpui_kit::rgb(palette.surface)),
                                        )
                                    }),
                            )
                            .child(rp::KB_SELECT_ALL.get()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child(rp::kb_selected_count(selected_count)),
                    ),
            )
            .when_some(kb.ocr_error.clone(), |col, msg| {
                col.child(
                    div()
                        .px_2p5()
                        .py_1p5()
                        .rounded(px(6.))
                        .bg(fg_alpha(0xdc2626, 0.08))
                        .border_1()
                        .border_color(fg_alpha(0xdc2626, 0.25))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .text_size(px(11.5))
                        .text_color(gpui_kit::rgb(0xdc2626))
                        .child(div().flex_1().min_w_0().text_ellipsis().child(msg))
                        .when(!kb.accurate_configured, |banner| {
                            banner.child(
                                div()
                                    .id("pdf-mgr-open-settings")
                                    .text_size(px(11.))
                                    .text_color(gpui_kit::rgb(palette.accent))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.open_kb_api_settings(window, cx);
                                    }))
                                    .child(rp::KB_OPEN_SETTINGS.get()),
                            )
                        }),
                )
            });

        let list_body: AnyElement = if filtered.is_empty() {
            self.render_kb_centered_state(
                IconName::BookOpenText,
                rp::KB_PDF_EMPTY.get(),
                None,
                None,
                false,
            )
        } else {
            div()
                .id("kb-pdf-mgr-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_3()
                .pb_3()
                .flex()
                .flex_col()
                .gap_1()
                .children(filtered.into_iter().enumerate().map(|(idx, item)| {
                    let rel_path = item.rel_path.clone();
                    let content_hash = item.content_hash.clone();
                    let checked = kb.selected_pdfs.contains(&rel_path);
                    let blocked = is_blocked(item);
                    let accurate = is_accurate(item);
                    let stem = display_pdf_stem(&rel_path);
                    let dir_hint = middle_ellipsis_dir(&rel_path);
                    let tooltip_path = rel_path.clone();
                    let phase = kb.ocr_progress.get(&rel_path).cloned();
                    let err_tip = item
                        .last_error
                        .clone()
                        .unwrap_or_else(|| rp::KB_NEEDS_OCR_BADGE.get().to_owned());

                    let toggle_path = rel_path.clone();
                    let open_path = rel_path.clone();
                    let cancel_path = rel_path.clone();
                    let export_path = rel_path.clone();
                    let export_hash = content_hash.clone();

                    let status_slot: AnyElement = if let Some(phase) = phase {
                        let (fill_w, label) = match phase {
                            PdfOcrPhase::Queued => (14.0_f32, "…".to_owned()),
                            PdfOcrPhase::Running {
                                extracted_pages,
                                total_pages: Some(total),
                            } if total > 0 => {
                                let ratio = (extracted_pages as f32 / total as f32).clamp(0.08, 1.0);
                                (44.0 * ratio, format!("{extracted_pages}/{total}"))
                            }
                            PdfOcrPhase::Running { extracted_pages, .. } => {
                                (18.0_f32, format!("{extracted_pages}/…"))
                            }
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .flex_shrink_0()
                            .child(
                                crate::ui::icon(IconName::LoaderCircle)
                                    .w(px(11.))
                                    .h(px(11.))
                                    .text_color(gpui_kit::rgb(palette.accent))
                                    .with_animation(
                                        SharedString::from(format!("pdf-row-spin-{idx}")),
                                        Animation::new(Duration::from_millis(850)).repeat(),
                                        |el, t| el.rotate(gpui::radians(t * std::f32::consts::TAU)),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(44.))
                                    .h(px(4.))
                                    .rounded_full()
                                    .bg(fg_alpha(palette.text, 0.10))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(fill_w))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(palette.accent)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("pdf-row-cancel-{idx}")))
                                    .size(px(18.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .hover(move |s| {
                                        s.bg(hover_bg).text_color(gpui_kit::rgb(0xdc2626))
                                    })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        cx.stop_propagation();
                                        app.cancel_pdf_ocr_item(&cancel_path, cx);
                                    }))
                                    .child(crate::ui::icon(IconName::Close).w(px(10.)).h(px(10.))),
                            )
                            .into_any_element()
                    } else if accurate {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id(SharedString::from(format!("pdf-row-export-{idx}")))
                                    .size(px(20.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .hover(move |s| {
                                        s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text))
                                    })
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            rp::KB_EXPORT_MD_BTN.get(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        cx.stop_propagation();
                                        app.export_pdf_markdown_item(&export_path, &export_hash, cx);
                                    }))
                                    .child(
                                        crate::ui::icon(IconName::FileDown)
                                            .w(px(12.))
                                            .h(px(12.)),
                                    ),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("pdf-row-acc-{idx}")))
                                    .size(px(18.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(gpui_kit::rgb(palette.accent))
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            rp::KB_ACCURATE_BADGE.get(),
                                        )
                                        .build(window, cx)
                                    })
                                    .child(
                                        crate::ui::icon(IconName::Sparkles)
                                            .w(px(12.))
                                            .h(px(12.)),
                                    ),
                            )
                            .into_any_element()
                    } else if blocked {
                        div()
                            .id(SharedString::from(format!("pdf-row-red-{idx}")))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_shrink_0()
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(err_tip.clone())
                                    .build(window, cx)
                            })
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(gpui_kit::rgb(0xef4444)),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .id(SharedString::from(format!("pdf-row-fast-{idx}")))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    rp::KB_FAST_BADGE.get(),
                                )
                                .build(window, cx)
                            })
                            .child(crate::ui::icon(IconName::Zap).w(px(11.5)).h(px(11.5)))
                            .into_any_element()
                    };

                    div()
                        .id(SharedString::from(format!("pdf-mgr-row-{idx}")))
                        .min_h(px(32.))
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .hover(move |s| s.bg(hover_bg))
                        .child(
                            div()
                                .id(SharedString::from(format!("pdf-row-check-{idx}")))
                                .size(px(14.))
                                .rounded(px(3.5))
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
                                    if !app.right_panel.kb.selected_pdfs.remove(&toggle_path) {
                                        app.right_panel.kb.selected_pdfs.insert(toggle_path.clone());
                                    }
                                    cx.notify();
                                }))
                                .when(checked, |b| {
                                    b.child(
                                        crate::ui::icon(IconName::Check)
                                            .w(px(9.))
                                            .h(px(9.))
                                            .text_color(gpui_kit::rgb(palette.surface)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("pdf-row-open-{idx}")))
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .cursor_pointer()
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tooltip_path.clone())
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    app.open_kb_result(
                                        &open_path,
                                        None,
                                        Some(1),
                                        None,
                                        None,
                                        window,
                                        cx,
                                    );
                                }))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_medium()
                                        .text_color(gpui_kit::rgb(palette.text))
                                        .text_ellipsis()
                                        .child(stem),
                                )
                                .when_some(dir_hint, |col, dir| {
                                    col.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(gpui_kit::rgb(palette.muted))
                                            .text_ellipsis()
                                            .child(dir),
                                    )
                                }),
                        )
                        .child(status_slot)
                }))
                .into_any_element()
        };

        let selected_paths_for_run: Vec<String> = kb
            .pdf_items
            .iter()
            .filter(|i| kb.selected_pdfs.contains(&i.rel_path))
            .map(|i| i.rel_path.clone())
            .collect();
        let only_ocr_count = only_ocr_targets.len();

        let footer = div()
            .px_3()
            .py_2()
            .flex_shrink_0()
            .border_t_1()
            .border_color(gpui_kit::rgb(palette.border))
            .bg(gpui_kit::rgb(header_bg))
            .flex()
            .items_center()
            .justify_between()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Button::new("pdf-mgr-run-selected")
                            .primary()
                            .compact()
                            .h(px(26.))
                            .px_2p5()
                            .text_size(px(11.5))
                            .disabled(selected_count == 0 || ocr_active)
                            .icon(
                                crate::ui::icon(IconName::Sparkles)
                                    .w(px(11.))
                                    .h(px(11.)),
                            )
                            .label(rp::kb_refine_selected(selected_count))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.run_pdf_ocr_batch(selected_paths_for_run.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new("pdf-mgr-run-only-ocr")
                            .ghost()
                            .compact()
                            .h(px(26.))
                            .px_2()
                            .text_size(px(11.5))
                            .disabled(only_ocr_count == 0 || ocr_active)
                            .label(if only_ocr_count > 0 {
                                format!("{} ({only_ocr_count})", rp::KB_ONLY_OCR_BTN.get())
                            } else {
                                rp::KB_ONLY_OCR_BTN.get().to_owned()
                            })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.run_pdf_ocr_batch(only_ocr_targets.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new("pdf-mgr-export-selected")
                            .ghost()
                            .compact()
                            .h(px(26.))
                            .px_2()
                            .text_size(px(11.5))
                            .disabled(selected_accurate_count == 0)
                            .icon(
                                crate::ui::icon(IconName::FileDown)
                                    .w(px(11.))
                                    .h(px(11.)),
                            )
                            .label(rp::KB_EXPORT_MD_BTN.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.export_selected_pdf_markdowns(cx);
                            })),
                    ),
            )
            .when(ocr_active, |bar| {
                bar.child(
                    Button::new("pdf-mgr-cancel-all")
                        .ghost()
                        .compact()
                        .h(px(26.))
                        .px_2()
                        .text_size(px(11.5))
                        .text_color(gpui_kit::rgb(0xdc2626))
                        .icon(crate::ui::icon(IconName::Close).w(px(11.)).h(px(11.)))
                        .label(rp::KB_CANCEL_BTN.get())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.cancel_all_pdf_ocr(cx);
                        })),
                )
            });

        div()
            .id("right-panel-pdf-manager")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(gpui_kit::rgb(palette.surface))
            .border_l_1()
            .border_color(gpui_kit::rgb(palette.border))
            .child(header)
            .child(controls)
            .child(list_body)
            .child(footer)
            .into_any_element()
    }
}

fn display_pdf_stem(rel_path: &str) -> String {
    let file_name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    file_name
        .strip_suffix(".pdf")
        .or_else(|| file_name.strip_suffix(".PDF"))
        .unwrap_or(file_name)
        .to_owned()
}
