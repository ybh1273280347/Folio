//! Right-panel Knowledge Base view: fast keyword search, book-style result cards,
//! jump-to-location with temporary highlight, and explicit PaddleOCR refinement.

use super::*;
use crate::i18n::right_panel as rp;
use crate::rag::store::{PdfLibraryItem, StoreStats};
use crate::rag::{
    SearchHit,
    pipeline::{KbSearchSnapshots, clean_heading_text},
};
use gpui_kit::base::Disableable;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
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
    Complete,
    Accurate,
}

#[derive(Clone, Copy)]
struct KbType {
    title: f32,
    body: f32,
    meta: f32,
    micro: f32,
}

impl KbType {
    fn from_ui_font(base: u8) -> Self {
        let s = f32::from(base);
        Self {
            title: (s - 1.0).max(11.0),
            body: (s - 2.0).max(10.0),
            meta: (s - 3.0).max(9.5),
            micro: (s - 3.5).max(9.0),
        }
    }
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
    pub(in crate::workspace) snapshot: KbSearchSnapshots,
    pub(in crate::workspace) filter: KbFilter,
    pub(in crate::workspace) expanded_files: HashSet<String>,
    pub(in crate::workspace) collapsing_files: HashMap<String, u64>,
    pub(in crate::workspace) card_anim_gen: HashMap<String, u64>,
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
    pub(in crate::workspace) pdf_covers: HashMap<String, Option<Arc<RenderImage>>>,
    pub(in crate::workspace) cover_loading: HashSet<String>,
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
                            || app.right_panel.kb.has_any_results()
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
            snapshot: KbSearchSnapshots::default(),
            filter: KbFilter::All,
            expanded_files: HashSet::new(),
            collapsing_files: HashMap::new(),
            card_anim_gen: HashMap::new(),
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
            pdf_covers: HashMap::new(),
            cover_loading: HashSet::new(),
            pending_search_clear: false,
            in_flight_search_id: 0,
        }
    }

    pub(in crate::workspace) fn active_results(&self) -> &[SearchHit] {
        match self.filter {
            KbFilter::All => &self.snapshot.all,
            KbFilter::Notes => &self.snapshot.notes,
            KbFilter::Pdf => &self.snapshot.pdf,
        }
    }

    pub(in crate::workspace) fn has_any_results(&self) -> bool {
        !self.snapshot.all.is_empty()
            || !self.snapshot.notes.is_empty()
            || !self.snapshot.pdf.is_empty()
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
        self.snapshot = KbSearchSnapshots::default();
        self.expanded_files.clear();
        self.collapsing_files.clear();
        self.card_anim_gen.clear();
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
        self.pdf_covers.clear();
        self.cover_loading.clear();
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
        let live_hashes: HashSet<&str> = pdf_items.iter().map(|i| i.content_hash.as_str()).collect();
        self.selected_pdfs.retain(|p| live_pdfs.contains(p.as_str()));
        self.pdf_covers.retain(|h, _| live_hashes.contains(h.as_str()));
        self.pdf_items = pdf_items;
        self.stats = Some(stats);
    }

    pub(in crate::workspace) fn remap_rel_path(
        path: &str,
        moves: &[(String, String)],
        is_folder: bool,
    ) -> Option<String> {
        let (base, suffix) = match path.strip_suffix("#annotations") {
            Some(base) => (base, "#annotations"),
            None => (path, ""),
        };
        for (old, new) in moves {
            if base == old {
                return Some(format!("{new}{suffix}"));
            }
            if is_folder {
                let prefix = format!("{old}/");
                if let Some(rest) = base.strip_prefix(&prefix) {
                    return Some(format!("{new}/{rest}{suffix}"));
                }
            }
        }
        None
    }

    pub(in crate::workspace) fn relocate_paths(
        &mut self,
        moves: &[(String, String)],
        is_folder: bool,
    ) {
        if moves.is_empty() {
            return;
        }
        let remap = |p: &str| Self::remap_rel_path(p, moves, is_folder);
        for item in &mut self.pdf_items {
            if let Some(next) = remap(&item.rel_path) {
                item.rel_path = next;
                if item
                    .last_error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("读取 PDF 失败 ("))
                {
                    item.last_error = None;
                }
            }
        }
        self.pdf_items.sort_by(|a, b| {
            let a_needs = a.status == "needs_ocr";
            let b_needs = b.status == "needs_ocr";
            b_needs.cmp(&a_needs).then_with(|| a.rel_path.cmp(&b.rel_path))
        });
        for list in [
            &mut self.snapshot.all,
            &mut self.snapshot.notes,
            &mut self.snapshot.pdf,
        ] {
            for hit in list {
                if let Some(next) = remap(&hit.path) {
                    hit.path = next;
                }
            }
        }
        if let Some(stats) = &mut self.stats {
            for path in &mut stats.needs_ocr_files {
                if let Some(next) = remap(path) {
                    *path = next;
                }
            }
        }
        self.selected_pdfs = self
            .selected_pdfs
            .iter()
            .map(|p| remap(p).unwrap_or_else(|| p.clone()))
            .collect();
        self.expanded_files = self
            .expanded_files
            .iter()
            .map(|p| remap(p).unwrap_or_else(|| p.clone()))
            .collect();
        self.collapsing_files = self
            .collapsing_files
            .iter()
            .map(|(p, v)| (remap(p).unwrap_or_else(|| p.clone()), *v))
            .collect();
        self.card_anim_gen = self
            .card_anim_gen
            .iter()
            .map(|(p, v)| (remap(p).unwrap_or_else(|| p.clone()), *v))
            .collect();
        self.ocr_progress = self
            .ocr_progress
            .iter()
            .map(|(p, v)| (remap(p).unwrap_or_else(|| p.clone()), v.clone()))
            .collect();
        self.sync_error = None;
        self.ocr_error = None;
    }

    pub(in crate::workspace) fn prune_missing_paths(&mut self, live_files: &HashSet<String>) {
        self.pdf_items.retain(|item| live_files.contains(&item.rel_path));
        self.selected_pdfs.retain(|p| live_files.contains(p));
        self.ocr_progress.retain(|p, _| live_files.contains(p));
        for list in [
            &mut self.snapshot.all,
            &mut self.snapshot.notes,
            &mut self.snapshot.pdf,
        ] {
            list.retain(|hit| {
                let base = hit.path.strip_suffix("#annotations").unwrap_or(&hit.path);
                live_files.contains(base)
            });
        }
        if let Some(stats) = &mut self.stats {
            stats.needs_ocr_files.retain(|p| live_files.contains(p));
        }
        let live_hashes: HashSet<&str> =
            self.pdf_items.iter().map(|i| i.content_hash.as_str()).collect();
        self.pdf_covers.retain(|h, _| live_hashes.contains(h.as_str()));
    }

    pub(in crate::workspace) fn red_dot_count(&self) -> usize {
        if !self.pdf_items.is_empty() {
            self.pdf_items
                .iter()
                .filter(|item| !item.is_complete || item.last_error.is_some())
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

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

fn kb_desk_bg(palette: crate::theme::Palette) -> Rgba {
    if palette.is_dark() {
        gpui_kit::rgb(palette.background)
    } else {
        mix_color(palette.background, palette.border, 0.62)
    }
}

fn kb_paper_shadow(is_dark: bool) -> Vec<BoxShadow> {
    vec![
        // Light contact shadow
        BoxShadow::new(
            px(0.),
            px(1.),
            hsla(0., 0., 0., if is_dark { 0.30 } else { 0.055 }),
        )
        .blur_radius(px(1.5)),
        // Very faint ambient shadow
        BoxShadow::new(
            px(0.),
            px(2.),
            hsla(0., 0., 0., if is_dark { 0.16 } else { 0.028 }),
        )
        .blur_radius(px(4.)),
    ]
}

fn render_grid_rows(cards: Vec<AnyElement>, cols: usize, gap_px: f32) -> AnyElement {
    if cols <= 1 {
        return div()
            .flex()
            .flex_col()
            .gap(px(gap_px))
            .children(cards)
            .into_any_element();
    }
    let mut row_els = Vec::new();
    let mut iter = cards.into_iter().peekable();
    while iter.peek().is_some() {
        let mut row = div().flex().items_stretch().gap(px(gap_px));
        for _ in 0..cols {
            let cell = iter
                .next()
                .unwrap_or_else(|| div().flex_1().min_w_0().into_any_element());
            row = row.child(cell);
        }
        row_els.push(row.into_any_element());
    }
    div()
        .flex()
        .flex_col()
        .gap(px(gap_px))
        .children(row_els)
        .into_any_element()
}

fn render_pdf_cover_thumb(
    cover: Option<&Arc<RenderImage>>,
    w: f32,
    h: f32,
    framed: bool,
    palette: crate::theme::Palette,
) -> AnyElement {
    let paper_border = fg_alpha(palette.text, if palette.is_dark() { 0.18 } else { 0.14 });
    let frame = div()
        .w(px(w))
        .h(px(h))
        .bg(gpui_kit::rgb(0xffffff))
        .overflow_hidden()
        .flex_shrink_0()
        .when(framed, |el| {
            el.rounded(px(3.5))
                .border_1()
                .border_color(paper_border)
                .shadow(vec![
                    BoxShadow::new(
                        px(0.),
                        px(1.5),
                        hsla(0., 0., 0., if palette.is_dark() { 0.42 } else { 0.11 }),
                    )
                    .blur_radius(px(4.)),
                ])
        });

    if let Some(image) = cover {
        frame
            .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
            .into_any_element()
    } else {
        let line_color = fg_alpha(0x64748b, 0.22);
        frame
            .bg(if palette.is_dark() {
                gpui_kit::rgb(0x282825)
            } else {
                gpui_kit::rgb(0xfaf9f7)
            })
            .p(px((w * 0.16).clamp(4., 10.)))
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px((h * 0.06).clamp(2.5, 5.)))
                    .child(
                        div()
                            .h(px(2.5))
                            .w(relative(0.68))
                            .rounded_full()
                            .bg(fg_alpha(palette.accent, 0.42)),
                    )
                    .child(div().h(px(2.)).w_full().rounded_full().bg(line_color))
                    .child(div().h(px(2.)).w(relative(0.84)).rounded_full().bg(line_color))
                    .child(div().h(px(2.)).w(relative(0.55)).rounded_full().bg(line_color)),
            )
            .child(
                div()
                    .text_size(px((w * 0.15).clamp(7.5, 10.)))
                    .font_medium()
                    .text_color(fg_alpha(palette.muted, 0.65))
                    .child("PDF"),
            )
            .into_any_element()
    }
}

impl Workspace {
    fn kb_panel_width(&self, window: &Window) -> f32 {
        if self.right_panel.maximized {
            let sidebar = if self.settings.sidebar_open {
                self.settings.sidebar_width.clamp(170., 520.)
            } else {
                0.0
            };
            let rail = if self.zen_mode {
                0.0
            } else {
                crate::workspace::RAIL_WIDTH
            };
            (f32::from(window.viewport_size().width) - rail - sidebar).max(360.0)
        } else {
            self.settings.terminal.panel_width.clamp(280., 1600.)
        }
    }

    pub(super) fn render_knowledge_base_panel(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.right_panel.kb.pdf_manager_open && self.projects.directory().is_some() && self.kb_enabled() {
            return self.render_pdf_manager_panel(window, cx);
        }
        let palette = self.settings.theme.palette;
        let t = KbType::from_ui_font(self.settings.interface_font_size);
        let chrome_bg = if palette.is_dark() {
            mix_color(palette.background, palette.surface, 0.35)
        } else {
            mix_color(palette.surface, palette.hover, 0.55)
        };
        let is_project = self.projects.directory().is_some();
        let enabled = self.kb_enabled();
        let syncing = self.kb_syncing();
        let kb = &self.right_panel.kb;
        let ocr_active = !kb.ocr_progress.is_empty();
        let has_red_dot = kb.has_ocr_red_dot();

        let menu_app = cx.entity();
        let header = div()
            .h(px(crate::ui::PANE_HEADER_HEIGHT))
            .px_2()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .bg(chrome_bg)
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        crate::ui::panel_toolbar_button("kb-back-to-hub", palette, cx)
                            .icon(crate::ui::icon(IconName::ChevronLeft).size(px(15.)))
                            .tooltip(crate::i18n::common::BACK_TO_HUB.get())
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.open_right_panel(RightPanelView::Hub, window, cx);
                            })),
                    )
                    .child(
                        crate::ui::icon(IconName::Library)
                            .w(px(15.))
                            .h(px(15.))
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(t.title))
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
                                .size(px(15.))
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
                            crate::ui::icon(IconName::FolderSync).size(px(15.)).into_any_element()
                        };
                        let pdf_mgr_icon = if ocr_active {
                            crate::ui::icon(IconName::LoaderCircle)
                                .size(px(15.))
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
                            crate::ui::icon(IconName::BookOpenText).size(px(15.)).into_any_element()
                        };
                        bar.child(
                            crate::ui::panel_toolbar_button("kb-sync-now", palette, cx)
                                .disabled(syncing)
                                .tooltip(if syncing {
                                    rp::BADGE_SYNCING.get()
                                } else {
                                    rp::KB_SYNC_TOOLTIP.get()
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.run_kb_sync(false, None, Vec::new(), cx);
                                }))
                                .child(sync_icon),
                        )
                        .child(
                            crate::ui::panel_toolbar_button("kb-open-pdf-manager", palette, cx)
                                .relative()
                                .tooltip(rp::KB_PDF_MANAGER_TOOLTIP.get())
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.right_panel.kb.pdf_manager_open = true;
                                    app.refresh_kb_snapshot(cx);
                                    app.ensure_pdf_covers_loaded(cx);
                                    cx.notify();
                                }))
                                .child(pdf_mgr_icon)
                                .when(has_red_dot, |btn| {
                                    btn.child(
                                        div()
                                            .absolute()
                                            .top(px(3.))
                                            .right(px(3.))
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(0xef4444)),
                                    )
                                }),
                        )
                        .child(
                            crate::ui::panel_toolbar_button("kb-more-menu", palette, cx)
                                .disabled(syncing)
                                .icon(crate::ui::icon(IconName::Ellipsis).size(px(15.)))
                                .tooltip(rp::KB_MENU_TOOLTIP.get())
                                .folio_dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                    let rebuild_app = app_menu.clone();
                                    let disable_app = app_menu.clone();
                                    menu.min_w(px(164.))
                                        .item(
                                            FolioMenuItem::new(rp::KB_MENU_REBUILD.get())
                                                .icon(IconName::RotateCcw)
                                                .on_click(move |_, _, cx| {
                                                    rebuild_app.update(cx, |app, cx| {
                                                        app.run_kb_sync(true, None, Vec::new(), cx);
                                                    });
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            FolioMenuItem::new(rp::KB_MENU_DISABLE.get())
                                                .icon(IconName::Trash)
                                                .danger()
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
                        crate::ui::panel_toolbar_button("kb-maximize", palette, cx)
                            .icon(crate::ui::icon(IconName::Maximize2).size(px(15.)))
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
        let t = KbType::from_ui_font(self.settings.interface_font_size);
        let canvas_bg = kb_desk_bg(palette);
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
            .bg(canvas_bg)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size(px(42.))
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon_el),
            )
            .child(
                div()
                    .text_size(px(t.title))
                    .font_medium()
                    .text_color(gpui_kit::rgb(palette.text))
                    .text_center()
                    .child(title),
            )
            .when_some(subtitle, |col, subtitle| {
                col.child(
                    div()
                        .text_size(px(t.body))
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
        let t = KbType::from_ui_font(self.settings.interface_font_size);
        let kb = &self.right_panel.kb;
        let syncing = self.kb_syncing();
        let search_focused = kb.search.read(cx).focus_handle(cx).is_focused(window);
        let input_non_empty = !kb.search.read(cx).value().trim().is_empty();
        let has_submitted_search =
            !kb.last_query.is_empty() || kb.searching || kb.has_any_results() || kb.search_error.is_some();

        let ring_color = if search_focused {
            fg_alpha(palette.accent, 0.72)
        } else {
            gpui_kit::rgb(palette.border)
        };
        let outer_halo = fg_alpha(palette.accent, 0.14);
        let hover_bg = gpui_kit::rgb(palette.hover);
        let seg_bg = fg_alpha(palette.text, 0.045);

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
            .rounded(px(9.))
            .when(search_focused, |el| el.p(px(2.)).bg(outer_halo))
            .child(
                div()
                    .w_full()
                    .h(px(32.))
                    .px(px(10.))
                    .rounded(px(8.))
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
                            .text_size(px(t.body))
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

        let active_hits = kb.active_results();
        let groups = group_hits_by_file(active_hits, kb.filter);
        let filtered_hit_count: usize = groups.iter().map(|g| g.hits.len()).sum();
        let filtered_doc_count = groups.len();
        let total_doc_count = kb.stats.as_ref().map_or(0, |s| s.indexed_files);

        let chip = |id: &'static str, filter_val: KbFilter, icon: Option<IconName>, label: &'static str| {
            let active = kb.filter == filter_val;
            div()
                .id(id)
                .h(px(22.))
                .px_2()
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(t.meta))
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(fg_alpha(palette.accent, 0.13))
                        .text_color(gpui_kit::rgb(palette.accent))
                        .font_medium()
                })
                .when(!active, |el| {
                    el.text_color(gpui_kit::rgb(palette.muted))
                        .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                })
                .on_click(cx.listener(move |app, _, _, cx| {
                    if app.right_panel.kb.filter == filter_val {
                        return;
                    }
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
            rp::kb_search_summary(filtered_hit_count, filtered_doc_count, active_hits.len() >= 20)
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(2.))
                    .rounded(px(7.))
                    .bg(seg_bg)
                    .gap_0p5()
                    .child(chip("kb-filter-all", KbFilter::All, None, rp::KB_FILTER_ALL.get()))
                    .child(chip(
                        "kb-filter-notes",
                        KbFilter::Notes,
                        Some(IconName::FileText),
                        rp::KB_FILTER_NOTES.get(),
                    ))
                    .child(chip(
                        "kb-filter-pdf",
                        KbFilter::Pdf,
                        Some(IconName::BookOpenText),
                        rp::KB_FILTER_PDF.get(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(t.meta))
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
                    .px_3()
                    .pt_2p5()
                    .pb_2p5()
                    .flex_shrink_0()
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
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
                                                .text_size(px(t.meta))
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
                                                div()
                                                    .id("kb-err-settings")
                                                    .px_1p5()
                                                    .py_0p5()
                                                    .rounded(px(4.))
                                                    .text_size(px(t.meta))
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.accent))
                                                    .cursor_pointer()
                                                    .hover(move |s| s.bg(hover_bg))
                                                    .on_click(cx.listener(|app, _, window, cx| {
                                                        app.open_kb_api_settings(window, cx);
                                                    }))
                                                    .child(rp::KB_OPEN_SETTINGS.get()),
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
        let t = KbType::from_ui_font(self.settings.interface_font_size);
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

        let panel_w = self.kb_panel_width(window);
        let cols = if panel_w >= 1100.0 {
            5
        } else if panel_w >= 860.0 {
            4
        } else if panel_w >= 620.0 {
            3
        } else if panel_w >= 390.0 {
            // Side padding and the gutter leave 178 px per card at this threshold.
            2
        } else {
            1
        };
        let palette = self.settings.theme.palette;
        let is_dark = palette.is_dark();
        let canvas_bg = kb_desk_bg(palette);
        let paper_bg = palette.paper_surface();
        let card_hover = palette.paper_hover();
        let head_bg = palette.paper_header();
        let paper_border = palette.paper_border();
        let divider_color = palette.paper_divider();
        let folio_color = palette.paper_folio();
        let stack_bg = palette.paper_stack();
        let badge_bg = fg_alpha(palette.text, 0.05);
        let hi_bg = palette.paper_highlight().into();
        let hi_fg = gpui_kit::rgb(palette.text).into();
        let card_shadow = kb_paper_shadow(is_dark);

        let snippet_max_h = (t.body * 1.5 * 3.0).ceil();

        let mut snippet_style = window.text_style();
        snippet_style.font_family = crate::ui::sans_font(&self.settings.interface_font).into();
        snippet_style.font_size = px(t.body).into();
        snippet_style.color = fg_alpha(palette.text, 0.88).into();

        let cards: Vec<AnyElement> = groups
            .into_iter()
            .enumerate()
            .map(|(group_idx, group)| {
                let clean_path_owned = group.clean_path.to_owned();
                let expanded = kb.expanded_files.contains(group.clean_path);
                let collapsing = kb.collapsing_files.contains_key(group.clean_path);
                let anim_gen = kb.card_anim_gen.get(group.clean_path).copied().unwrap_or(0);
                let total_hits = group.hits.len();
                let base_count = total_hits.min(2);
                let extra_count = total_hits.saturating_sub(base_count);
                let show_extra = (expanded || collapsing) && extra_count > 0;
                let has_stack = !expanded && !collapsing && extra_count > 0;
                let first_hit = group.hits[0];
                let is_tex = group.clean_path.to_ascii_lowercase().ends_with(".tex");
                let file_icon = if group.is_pdf {
                    IconName::BookOpenText
                } else if first_hit.kind == "annotation" {
                    IconName::NotebookPen
                } else {
                    IconName::FileText
                };
                let spine_hex = if group.is_pdf {
                    if is_dark { palette.accent } else { 0xb86443 }
                } else if is_tex {
                    if is_dark { 0x7e93b8 } else { 0x566b8f }
                } else {
                    palette.accent
                };
                let ribbon_bg = mix_color(palette.surface, spine_hex, if is_dark { 0.22 } else { 0.12 });
                let ribbon_border = fg_alpha(spine_hex, if is_dark { 0.34 } else { 0.22 });
                let ribbon_fg = if is_dark {
                    gpui_kit::rgb(palette.text)
                } else {
                    mix_color(spine_hex, palette.text, 0.58)
                };

                let header_hit = first_hit.clone();

                let mut body_col = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(SharedString::from(format!("kb-book-head-{group_idx}")))
                            .cursor_pointer()
                            .bg(head_bg)
                            .border_b_1()
                            .border_color(paper_border)
                            .px_3()
                            .py_2()
                            .hover(move |s| s.bg(card_hover))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.open_kb_location(
                                    &header_hit.path,
                                    header_hit.page,
                                    Some(&header_hit),
                                    window,
                                    cx,
                                );
                            }))
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        crate::ui::icon(file_icon)
                                            .w(px(14.))
                                            .h(px(14.))
                                            .flex_shrink_0()
                                            .text_color(gpui_kit::rgb(spine_hex)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .text_size(px(t.body))
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .text_ellipsis()
                                                    .child(group.file_name.clone()),
                                            )
                                            .when_some(group.dir_hint.clone(), |head, dir| {
                                                head.child(
                                                    div()
                                                        .text_size(px(t.micro))
                                                        .text_color(gpui_kit::rgb(palette.muted))
                                                        .text_ellipsis()
                                                        .child(dir),
                                                )
                                            }),
                                    ),
                            )
                            .when(group.is_accurate || total_hits > 1, |head| {
                                head.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .flex_shrink_0()
                                        .when(group.is_accurate, |row| {
                                            row.child(
                                                div()
                                                    .id(SharedString::from(format!("kb-acc-{group_idx}")))
                                                    .mt(px(1.))
                                                    .text_color(gpui_kit::rgb(spine_hex))
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
                                        })
                                        .when(total_hits > 1, |row| {
                                            row.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "kb-hits-bm-{group_idx}"
                                                    )))
                                                    .mt(px(-8.))
                                                    .px(px(5.))
                                                    .pt(px(3.5))
                                                    .pb(px(2.))
                                                    .rounded_t(px(0.))
                                                    .rounded_b(px(2.5))
                                                    .bg(ribbon_bg)
                                                    .border_x_1()
                                                    .border_b_1()
                                                    .border_color(ribbon_border)
                                                    .text_size(px((t.micro - 0.5).max(8.5)))
                                                    .font_medium()
                                                    .text_color(ribbon_fg)
                                                    .tooltip(move |window, cx| {
                                                        gpui_kit::component::tooltip::Tooltip::new(
                                                            rp::kb_hits_pill(total_hits),
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .child(total_hits.to_string()),
                                            )
                                        }),
                                )
                            }),
                    )
                    .child({
                        let render_hit_row = |hit_idx: usize, hit: &SearchHit| {
                            let open_hit = hit.clone();
                            let subtitle = section_subtitle(hit, &group.file_name);
                            let folio_label = match (hit.page, hit.line_start) {
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
                                &kb.snapshot.query,
                                snippet_style.clone(),
                                hi_bg,
                                hi_fg,
                            );

                            div()
                                .id(SharedString::from(format!(
                                    "kb-hit-{group_idx}-{hit_idx}"
                                )))
                                .px_3()
                                .pt_2()
                                .pb_1p5()
                                .when(hit_idx > 0, |row| {
                                    row.border_t_1().border_color(divider_color)
                                })
                                .cursor_pointer()
                                .hover(move |s| s.bg(card_hover))
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    app.open_kb_location(
                                        &open_hit.path,
                                        open_hit.page,
                                        Some(&open_hit),
                                        window,
                                        cx,
                                    );
                                }))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .when(
                                    subtitle.is_some() || extra_tag.is_some(),
                                    |row| {
                                        row.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_size(px(t.meta))
                                                        .font_medium()
                                                        .text_color(fg_alpha(
                                                            palette.text,
                                                            0.72,
                                                        ))
                                                        .text_ellipsis()
                                                        .children(subtitle),
                                                )
                                                .when_some(extra_tag, |r, tag| {
                                                    r.child(
                                                        div()
                                                            .px_1()
                                                            .rounded(px(3.))
                                                            .bg(badge_bg)
                                                            .text_size(px(t.micro))
                                                            .text_color(gpui_kit::rgb(palette.muted))
                                                            .child(tag),
                                                    )
                                                }),
                                        )
                                    },
                                )
                                .child(
                                    div()
                                        .max_h(px(snippet_max_h))
                                        .overflow_hidden()
                                        .text_size(px(t.body))
                                        .line_height(relative(1.5))
                                        .child(styled_snippet),
                                )
                                .when_some(folio_label, |row, folio| {
                                    row.child(
                                        div()
                                            .w_full()
                                            .pt_0p5()
                                            .flex()
                                            .justify_center()
                                            .text_size(px(t.micro))
                                            .text_color(folio_color)
                                            .child(folio),
                                    )
                                })
                        };

                        let base_rows: Vec<_> = group.hits[..base_count]
                            .iter()
                            .enumerate()
                            .map(|(hit_idx, hit)| render_hit_row(hit_idx, hit))
                            .collect();

                        let mut col = div().flex_1().flex().flex_col().children(base_rows);
                        if show_extra {
                            let extra_rows: Vec<_> = group.hits[base_count..]
                                .iter()
                                .enumerate()
                                .map(|(offset, hit)| render_hit_row(base_count + offset, hit))
                                .collect();
                            let est_extra_h = (extra_rows.len() as f32) * 116.0;
                            let extra_box = div()
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .children(extra_rows);
                            let animated_extra = if anim_gen > 0 {
                                extra_box
                                    .with_animation(
                                        SharedString::from(format!(
                                            "kb-book-extra-{group_idx}-{anim_gen}"
                                        )),
                                        Animation::new(Duration::from_millis(180))
                                            .with_easing(ease_out_cubic),
                                        move |el, p| {
                                            let progress = if collapsing { 1.0 - p } else { p };
                                            el.max_h(px(est_extra_h * progress))
                                                .opacity(0.2 + 0.8 * progress)
                                        },
                                    )
                                    .into_any_element()
                            } else {
                                extra_box.into_any_element()
                            };
                            col = col.child(animated_extra);
                        }
                        col
                    });

                if extra_count > 0 {
                    let toggle_path = clean_path_owned;
                    let footer_label = if expanded {
                        rp::KB_COLLAPSE_HITS.get().to_owned()
                    } else {
                        rp::kb_more_hits(extra_count)
                    };
                    let chevron = if anim_gen > 0 {
                        crate::ui::icon(IconName::ChevronDown)
                            .w(px(11.))
                            .h(px(11.))
                            .with_animation(
                                SharedString::from(format!(
                                    "kb-book-chev-{group_idx}-{anim_gen}"
                                )),
                                Animation::new(Duration::from_millis(180))
                                    .with_easing(ease_out_cubic),
                                move |el, p| {
                                    let progress = if collapsing {
                                        1.0 - p
                                    } else if expanded {
                                        p
                                    } else {
                                        0.0
                                    };
                                    el.rotate(gpui::radians(progress * std::f32::consts::PI))
                                },
                            )
                            .into_any_element()
                    } else {
                        crate::ui::icon(if expanded {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .w(px(11.))
                        .h(px(11.))
                        .into_any_element()
                    };
                    body_col = body_col.child(
                        div()
                            .id(SharedString::from(format!("kb-book-more-{group_idx}")))
                            .mt_auto()
                            .px_3()
                            .py_1p5()
                            .border_t_1()
                            .border_color(divider_color)
                            .bg(head_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .text_size(px(t.meta))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.accent))
                            .cursor_pointer()
                            .hover(move |s| s.bg(card_hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                let next_gen = app
                                    .right_panel
                                    .kb
                                    .card_anim_gen
                                    .get(&toggle_path)
                                    .copied()
                                    .unwrap_or(0)
                                    .wrapping_add(1)
                                    .max(1);
                                app.right_panel
                                    .kb
                                    .card_anim_gen
                                    .insert(toggle_path.clone(), next_gen);
                                if app.right_panel.kb.expanded_files.remove(&toggle_path) {
                                    app.right_panel
                                        .kb
                                        .collapsing_files
                                        .insert(toggle_path.clone(), next_gen);
                                    let clear_path = toggle_path.clone();
                                    cx.spawn(async move |this, cx| {
                                        cx.background_executor()
                                            .timer(Duration::from_millis(185))
                                            .await;
                                        let _ = this.update(cx, |app, cx| {
                                            if app
                                                .right_panel
                                                .kb
                                                .collapsing_files
                                                .get(&clear_path)
                                                .copied()
                                                == Some(next_gen)
                                            {
                                                app.right_panel
                                                    .kb
                                                    .collapsing_files
                                                    .remove(&clear_path);
                                                cx.notify();
                                            }
                                        });
                                    })
                                    .detach();
                                } else {
                                    app.right_panel.kb.collapsing_files.remove(&toggle_path);
                                    app.right_panel
                                        .kb
                                        .expanded_files
                                        .insert(toggle_path.clone());
                                }
                                cx.notify();
                            }))
                            .child(footer_label)
                            .child(chevron),
                    );
                }

                // Clean modern book-inspired card: 3px solid spine, asymmetric radii (2px spine, 6px fore-edge)
                let main_card = div()
                    .id(SharedString::from(format!("kb-book-{group_idx}")))
                    .w_full()
                    .when(cols > 1 && !expanded && !collapsing, |el| el.h(px(288.)))
                    .rounded_l(px(2.))
                    .rounded_r(px(6.))
                    .border_1()
                    .border_color(paper_border)
                    .bg(paper_bg)
                    .shadow(card_shadow.clone())
                    .overflow_hidden()
                    .flex()
                    .child(div().w(px(3.0)).flex_shrink_0().bg(gpui_kit::rgb(spine_hex)))
                    .child(body_col);

                // At most 1 subtle stacked sheet when collapsed with hidden hits
                div()
                    .flex_1()
                    .min_w_0()
                    .relative()
                    .when(has_stack, |wrap| {
                        wrap.pb(px(3.0)).pr(px(2.5)).child(
                            div()
                                .absolute()
                                .top(px(3.0))
                                .left(px(2.5))
                                .right(px(0.))
                                .bottom(px(0.))
                                .rounded_l(px(2.))
                                .rounded_r(px(6.))
                                .border_1()
                                .border_color(paper_border)
                                .bg(stack_bg),
                        )
                    })
                    .child(main_card)
                    .into_any_element()
            })
            .collect();

        let rows = render_grid_rows(cards, cols, 10.0);

        div()
            .id("kb-results-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .bg(canvas_bg)
            .p_3()
            .pb_4()
            .child(rows)
            .into_any_element()
    }

    fn render_pdf_manager_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.settings.theme.palette;
        let t = KbType::from_ui_font(self.settings.interface_font_size);
        let panel_w = self.kb_panel_width(window);
        let use_book_tiles = panel_w >= 340.0;
        let card_w = if panel_w >= 640.0 {
            144.0_f32
        } else if panel_w >= 380.0 {
            ((panel_w - 24.0 - 12.0) / 2.0).clamp(144.0, 176.0).floor()
        } else {
            132.0_f32
        };
        let thumb_h = (card_w * 1.28).round();
        let chrome_bg = if palette.is_dark() {
            mix_color(palette.background, palette.surface, 0.35)
        } else {
            mix_color(palette.surface, palette.hover, 0.55)
        };
        let canvas_bg = kb_desk_bg(palette);
        let hover_bg = gpui_kit::rgb(palette.hover);
        let row_hover = mix_color(palette.surface, palette.hover, 0.55);
        let card_shadow = kb_paper_shadow(palette.is_dark());
        let seg_bg = fg_alpha(palette.text, 0.045);
        let kb = &self.right_panel.kb;
        let syncing = self.kb_syncing();
        let ocr_active = !kb.ocr_progress.is_empty();

        let is_blocked = |item: &PdfLibraryItem| !item.is_complete || item.last_error.is_some();
        let is_accurate = |item: &PdfLibraryItem| matches!(item.parse_tier.as_str(), "accurate" | "mineru");
        let is_complete_clean = |item: &PdfLibraryItem| item.is_complete && item.last_error.is_none();

        let total_count = kb.pdf_items.len();
        let red_count = kb.pdf_items.iter().filter(|i| is_blocked(i)).count();
        let complete_count = kb.pdf_items.iter().filter(|i| is_complete_clean(i)).count();
        let acc_count = kb.pdf_items.iter().filter(|i| is_accurate(i)).count();

        let filtered: Vec<&PdfLibraryItem> = kb
            .pdf_items
            .iter()
            .filter(|item| match kb.pdf_filter {
                PdfManagerFilter::All => true,
                PdfManagerFilter::NeedsOcr => is_blocked(item),
                PdfManagerFilter::Complete => is_complete_clean(item),
                PdfManagerFilter::Accurate => is_accurate(item),
            })
            .collect();

        let filtered_paths: Vec<String> = filtered.iter().map(|i| i.rel_path.clone()).collect();
        let all_visible_selected = !filtered.is_empty()
            && filtered
                .iter()
                .all(|item| kb.selected_pdfs.contains(&item.rel_path));
        let selected_count = kb.selected_pdfs.len();
        let selected_exportable_count = kb
            .pdf_items
            .iter()
            .filter(|i| kb.selected_pdfs.contains(&i.rel_path) && i.is_complete)
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
                .size(px(15.))
                .text_color(gpui_kit::rgb(palette.accent))
                .with_animation(
                    "kb-pdf-mgr-sync-spin",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |icon, t| icon.rotate(gpui::radians(t * std::f32::consts::TAU)),
                )
                .into_any_element()
        } else {
            crate::ui::icon(IconName::FolderSync).size(px(15.)).into_any_element()
        };

        let header = div()
            .h(px(crate::ui::PANE_HEADER_HEIGHT))
            .px_2()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .bg(chrome_bg)
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        crate::ui::panel_toolbar_button("kb-pdf-mgr-back", palette, cx)
                            .icon(crate::ui::icon(IconName::ChevronLeft).size(px(15.)))
                            .tooltip(rp::KB_BACK_TO_SEARCH.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.right_panel.kb.pdf_manager_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::ui::icon(IconName::BookOpenText)
                            .w(px(15.))
                            .h(px(15.))
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(t.title))
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
                        crate::ui::panel_toolbar_button("kb-pdf-mgr-sync", palette, cx)
                            .disabled(syncing)
                            .tooltip(if syncing {
                                rp::BADGE_SYNCING.get()
                            } else {
                                rp::KB_SYNC_TOOLTIP.get()
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.run_kb_sync(false, None, Vec::new(), cx);
                            }))
                            .child(sync_icon),
                    )
                    .child(
                        crate::ui::panel_toolbar_button("kb-pdf-mgr-maximize", palette, cx)
                            .icon(crate::ui::icon(IconName::Maximize2).size(px(15.)))
                            .tooltip(crate::i18n::ai::MAXIMIZE_RESTORE.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.right_panel.maximized = !app.right_panel.maximized;
                                cx.notify();
                            })),
                    ),
            );

        let filter_chip = |id: &'static str,
                           val: PdfManagerFilter,
                           prefix: Option<AnyElement>,
                           label: &'static str,
                           count: usize| {
            let active = kb.pdf_filter == val;
            div()
                .id(id)
                .h(px(22.))
                .px_2()
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(t.meta))
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(fg_alpha(palette.accent, 0.13))
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
                .when_some(prefix, |el, p| el.child(p))
                .child(label)
                .child(
                    div()
                        .text_size(px(t.micro))
                        .text_color(if active {
                            fg_alpha(palette.accent, 0.85)
                        } else {
                            fg_alpha(palette.muted, 0.75)
                        })
                        .child(count.to_string()),
                )
        };

        let controls = div()
            .px_3()
            .pt_2p5()
            .pb_2()
            .flex_shrink_0()
            .bg(gpui_kit::rgb(palette.surface))
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .p(px(2.))
                    .rounded(px(7.))
                    .bg(seg_bg)
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_0p5()
                    .child(filter_chip(
                        "pdf-f-all",
                        PdfManagerFilter::All,
                        None,
                        rp::KB_FILTER_ALL.get(),
                        total_count,
                    ))
                    .child(filter_chip(
                        "pdf-f-ocr",
                        PdfManagerFilter::NeedsOcr,
                        Some(
                            div()
                                .size(px(6.))
                                .rounded_full()
                                .bg(gpui_kit::rgb(if red_count > 0 { 0xd97706 } else { palette.muted }))
                                .into_any_element(),
                        ),
                        rp::KB_PDF_FILTER_NEEDS_OCR.get(),
                        red_count,
                    ))
                    .child(filter_chip(
                        "pdf-f-complete",
                        PdfManagerFilter::Complete,
                        Some(
                            crate::ui::icon(IconName::Check)
                                .w(px(10.))
                                .h(px(10.))
                                .into_any_element(),
                        ),
                        rp::KB_PDF_FILTER_COMPLETE.get(),
                        complete_count,
                    ))
                    .child(filter_chip(
                        "pdf-f-acc",
                        PdfManagerFilter::Accurate,
                        Some(
                            crate::ui::icon(IconName::Sparkles)
                                .w(px(10.))
                                .h(px(10.))
                                .into_any_element(),
                        ),
                        rp::KB_PDF_FILTER_ACCURATE.get(),
                        acc_count,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_0p5()
                    .child(
                        div()
                            .id("pdf-select-all")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .text_size(px(t.meta))
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
                            .text_size(px(t.meta))
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
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(t.meta))
                                        .text_color(gpui_kit::rgb(palette.danger))
                                        .child(msg),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .flex_shrink_0()
                                .when(!kb.accurate_configured, |banner| {
                                    banner.child(
                                        div()
                                            .id("pdf-mgr-open-settings")
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .text_size(px(t.meta))
                                            .font_medium()
                                            .text_color(gpui_kit::rgb(palette.accent))
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(hover_bg))
                                            .on_click(cx.listener(|app, _, window, cx| {
                                                app.open_kb_api_settings(window, cx);
                                            }))
                                            .child(rp::KB_OPEN_SETTINGS.get()),
                                    )
                                })
                                .child(
                                    crate::ui::control("pdf-mgr-dismiss-err", IconName::Close).on_click(
                                        cx.listener(|app, _, _, cx| {
                                            app.right_panel.kb.ocr_error = None;
                                            cx.notify();
                                        }),
                                    ),
                                ),
                        ),
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
            let cards: Vec<AnyElement> = filtered
                .into_iter()
                .enumerate()
                .map(|(idx, item)| {
                    let rel_path = item.rel_path.clone();
                    let content_hash = item.content_hash.clone();
                    let cover_img = kb.pdf_covers.get(&content_hash).and_then(|opt| opt.as_ref());
                    let checked = kb.selected_pdfs.contains(&rel_path);
                    let accurate = is_accurate(item);
                    let hybrid = item.parse_tier == "hybrid";
                    let has_error = item.last_error.is_some();
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
                    let retry_path = rel_path.clone();

                    let (progress_ratio, progress_label) = match &phase {
                        Some(PdfOcrPhase::Queued) => (0.08_f32, rp::KB_QUEUED_OCR.get().to_owned()),
                        Some(PdfOcrPhase::Running {
                            extracted_pages,
                            total_pages: Some(total),
                        }) if *total > 0 => {
                            let ratio = (*extracted_pages as f32 / *total as f32).clamp(0.06, 1.0);
                            (ratio, rp::kb_ocr_running_label(&format!("{extracted_pages}/{total}")))
                        }
                        Some(PdfOcrPhase::Running { extracted_pages, .. }) => {
                            (0.25_f32, rp::kb_ocr_running_label(&format!("{extracted_pages}/…")))
                        }
                        None => (0.0_f32, String::new()),
                    };

                    let mut meta_parts: Vec<String> = Vec::new();
                    if let Some(dir) = dir_hint {
                        meta_parts.push(dir);
                    }
                    if let Some(pages) = item.total_pages {
                        meta_parts.push(rp::kb_pages_count(pages));
                    }
                    let subtitle_text = if phase.is_some() {
                        progress_label
                    } else if has_error {
                        err_tip.clone()
                    } else {
                        meta_parts.join(" · ")
                    };

                    let row_bg = if checked {
                        mix_color(palette.surface, palette.accent, 0.08)
                    } else {
                        gpui_kit::rgb(palette.surface).into()
                    };
                    let row_border = if checked {
                        fg_alpha(palette.accent, 0.48)
                    } else {
                        gpui_kit::rgb(palette.border).into()
                    };

                    let checkbox_el = div()
                        .id(SharedString::from(format!("pdf-row-check-{idx}")))
                        .size(px(14.))
                        .rounded(px(3.5))
                        .border_1()
                        .border_color(gpui_kit::rgb(if checked {
                            palette.accent
                        } else {
                            palette.border
                        }))
                        .bg(gpui_kit::rgb(if checked {
                            palette.accent
                        } else {
                            palette.surface
                        }))
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
                        });

                    let (tier_label, tier_color): (String, Rgba) = if phase.is_some() {
                        ("OCR…".to_owned(), gpui_kit::rgb(palette.accent).into())
                    } else if has_error {
                        (
                            rp::KB_FAILED_BADGE.get().to_owned(),
                            gpui_kit::rgb(palette.danger).into(),
                        )
                    } else if !item.is_complete {
                        let lbl = if item.missing_pages > 0 {
                            rp::kb_missing_pages_badge(item.missing_pages)
                        } else {
                            rp::KB_NEEDS_OCR_BADGE.get().to_owned()
                        };
                        (lbl, gpui_kit::rgb(0xc26d18).into())
                    } else if accurate {
                        (
                            rp::KB_ACCURATE_BADGE.get().to_owned(),
                            gpui_kit::rgb(palette.accent).into(),
                        )
                    } else if hybrid {
                        (
                            rp::KB_HYBRID_BADGE.get().to_owned(),
                            gpui_kit::rgb(palette.accent).into(),
                        )
                    } else {
                        (
                            rp::KB_FAST_BADGE.get().to_owned(),
                            gpui_kit::rgb(palette.muted).into(),
                        )
                    };

                    let action_btn: Option<AnyElement> = if phase.is_some() {
                        Some(
                            div()
                                .id(SharedString::from(format!("pdf-row-cancel-{idx}")))
                                .size(px(22.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .cursor_pointer()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .hover(move |s| {
                                    s.bg(mix_color(palette.surface, palette.danger, 0.12))
                                        .text_color(gpui_kit::rgb(palette.danger))
                                })
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(rp::KB_CANCEL_BTN.get())
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.cancel_pdf_ocr_item(&cancel_path, cx);
                                }))
                                .child(crate::ui::icon(IconName::Close).w(px(12.)).h(px(12.)))
                                .into_any_element(),
                        )
                    } else if has_error || !item.is_complete {
                        let icon_name = if has_error {
                            IconName::RotateCcw
                        } else {
                            IconName::Sparkles
                        };
                        let tip = if has_error {
                            rp::KB_RETRY_BTN.get()
                        } else {
                            rp::KB_ONLY_OCR_BTN.get()
                        };
                        Some(
                            div()
                                .id(SharedString::from(format!("pdf-row-act-{idx}")))
                                .size(px(22.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .cursor_pointer()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .hover(move |s| {
                                    s.bg(hover_bg).text_color(gpui_kit::rgb(palette.accent))
                                })
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.run_pdf_ocr_batch(vec![retry_path.clone()], true, cx);
                                }))
                                .child(crate::ui::icon(icon_name).w(px(12.)).h(px(12.)))
                                .into_any_element(),
                        )
                    } else if item.is_complete {
                        Some(
                            div()
                                .id(SharedString::from(format!("pdf-row-export-{idx}")))
                                .size(px(22.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
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
                                .child(crate::ui::icon(IconName::FileDown).w(px(12.)).h(px(12.)))
                                .into_any_element(),
                        )
                    } else {
                        None
                    };

                    if !use_book_tiles {
                        div()
                            .id(SharedString::from(format!("pdf-mgr-row-{idx}")))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(row_border)
                            .bg(row_bg)
                            .shadow(card_shadow.clone())
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .hover(move |s| {
                                if checked {
                                    s.bg(mix_color(palette.surface, palette.accent, 0.13))
                                } else {
                                    s.bg(row_hover)
                                }
                            })
                            .child(
                                div()
                                    .px_2p5()
                                    .py_2()
                                    .flex()
                                    .items_center()
                                    .gap_2p5()
                                    .child(checkbox_el)
                                    .child(render_pdf_cover_thumb(cover_img, 36.0, 48.0, true, palette))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("pdf-row-open-{idx}")))
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .cursor_pointer()
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    tooltip_path.clone(),
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                app.open_kb_location(
                                                    &open_path,
                                                    Some(1),
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }))
                                            .child(
                                                div()
                                                    .text_size(px(t.body))
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .text_ellipsis()
                                                    .child(stem),
                                            )
                                            .when(!subtitle_text.is_empty(), |col| {
                                                col.child(
                                                    div()
                                                        .text_size(px(t.micro))
                                                        .text_color(if has_error {
                                                            gpui_kit::rgb(palette.danger)
                                                        } else if phase.is_some() {
                                                            gpui_kit::rgb(palette.accent)
                                                        } else {
                                                            gpui_kit::rgb(palette.muted)
                                                        })
                                                        .text_ellipsis()
                                                        .child(subtitle_text),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .flex_shrink_0()
                                            .children(action_btn)
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "pdf-row-tier-{idx}"
                                                    )))
                                                    .text_size(px(t.micro))
                                                    .font_medium()
                                                    .text_color(tier_color)
                                                    .when(has_error, |el| {
                                                        let tip = err_tip.clone();
                                                        el.tooltip(move |window, cx| {
                                                            gpui_kit::component::tooltip::Tooltip::new(
                                                                tip.clone(),
                                                            )
                                                            .build(window, cx)
                                                        })
                                                    })
                                                    .child(tier_label),
                                            ),
                                    ),
                            )
                            .when(phase.is_some(), |card| {
                                card.child(
                                    div()
                                        .w_full()
                                        .h(px(2.5))
                                        .bg(fg_alpha(palette.text, 0.08))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(progress_ratio))
                                                .bg(gpui_kit::rgb(palette.accent)),
                                        ),
                                )
                            })
                            .into_any_element()
                    } else {
                        div()
                            .id(SharedString::from(format!("pdf-mgr-row-{idx}")))
                            .w(px(card_w))
                            .flex_shrink_0()
                            .rounded(px(7.))
                            .border_1()
                            .border_color(row_border)
                            .bg(row_bg)
                            .shadow(card_shadow.clone())
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .hover(move |s| {
                                if checked {
                                    s.bg(mix_color(palette.surface, palette.accent, 0.13))
                                } else {
                                    s.bg(row_hover)
                                }
                            })
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .h(px(thumb_h))
                                    .border_b_1()
                                    .border_color(fg_alpha(palette.border, 0.75))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("pdf-row-open-{idx}")))
                                            .size_full()
                                            .cursor_pointer()
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    tooltip_path.clone(),
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                app.open_kb_location(
                                                    &open_path,
                                                    Some(1),
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }))
                                            .child(render_pdf_cover_thumb(
                                                cover_img,
                                                card_w - 2.0,
                                                thumb_h,
                                                false,
                                                palette,
                                            )),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_1p5()
                                            .left_1p5()
                                            .rounded(px(4.))
                                            .bg(fg_alpha(palette.surface, 0.85))
                                            .p(px(1.5))
                                            .child(checkbox_el),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "pdf-row-tier-{idx}"
                                            )))
                                            .absolute()
                                            .top_1p5()
                                            .right_1p5()
                                            .px_1p5()
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .bg(fg_alpha(palette.surface, 0.90))
                                            .border_1()
                                            .border_color(fg_alpha(palette.border, 0.8))
                                            .text_size(px(t.micro))
                                            .font_medium()
                                            .text_color(tier_color)
                                            .when(has_error, |el| {
                                                let tip = err_tip.clone();
                                                el.tooltip(move |window, cx| {
                                                    gpui_kit::component::tooltip::Tooltip::new(
                                                        tip.clone(),
                                                    )
                                                    .build(window, cx)
                                                })
                                            })
                                            .child(tier_label),
                                    )
                                    .when_some(action_btn, |stage, btn| {
                                        stage.child(
                                            div()
                                                .absolute()
                                                .bottom_1p5()
                                                .right_1p5()
                                                .rounded(px(5.))
                                                .bg(fg_alpha(palette.surface, 0.92))
                                                .border_1()
                                                .border_color(fg_alpha(palette.border, 0.8))
                                                .child(btn),
                                        )
                                    }),
                            )
                            .when(phase.is_some(), |card| {
                                card.child(
                                    div()
                                        .w_full()
                                        .h(px(2.5))
                                        .bg(fg_alpha(palette.text, 0.08))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(progress_ratio))
                                                .bg(gpui_kit::rgb(palette.accent)),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .px_1p5()
                                    .py_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(0.5))
                                    .child(
                                        div()
                                            .text_size(px(t.meta))
                                            .line_height(relative(1.18))
                                            .font_medium()
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .text_ellipsis()
                                            .child(stem),
                                    )
                                    .when(!subtitle_text.is_empty(), |col| {
                                        col.child(
                                            div()
                                                .text_size(px((t.micro - 0.5).max(9.0)))
                                                .line_height(relative(1.12))
                                                .text_color(if has_error {
                                                    gpui_kit::rgb(palette.danger)
                                                } else if phase.is_some() {
                                                    gpui_kit::rgb(palette.accent)
                                                } else {
                                                    gpui_kit::rgb(palette.muted)
                                                })
                                                .text_ellipsis()
                                                .child(subtitle_text),
                                        )
                                    }),
                            )
                            .into_any_element()
                    }
                })
                .collect();

            let layout_el = if !use_book_tiles {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(cards)
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap(px(12.))
                    .children(cards)
                    .into_any_element()
            };

            div()
                .id("kb-pdf-mgr-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .bg(canvas_bg)
                .p_3()
                .pb_4()
                .child(layout_el)
                .into_any_element()
        };

        let selected_paths_for_run: Vec<String> = kb
            .pdf_items
            .iter()
            .filter(|i| kb.selected_pdfs.contains(&i.rel_path))
            .map(|i| i.rel_path.clone())
            .collect();
        let only_ocr_count = only_ocr_targets.len();

        let fill_disabled = only_ocr_count == 0 || ocr_active;
        let accurate_disabled = selected_count == 0 || ocr_active;
        let export_disabled = selected_exportable_count == 0;
        let fill_is_primary = only_ocr_count > 0 && !ocr_active;

        let footer_left: AnyElement = if ocr_active {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    crate::ui::icon(IconName::LoaderCircle)
                        .w(px(12.))
                        .h(px(12.))
                        .text_color(gpui_kit::rgb(palette.accent))
                        .with_animation(
                            "pdf-footer-spin",
                            Animation::new(Duration::from_millis(850)).repeat(),
                            |el, t| el.rotate(gpui::radians(t * std::f32::consts::TAU)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(t.meta))
                        .font_medium()
                        .text_color(gpui_kit::rgb(palette.accent))
                        .child(rp::kb_ocr_running_label(&kb.ocr_progress.len().to_string())),
                )
                .into_any_element()
        } else if selected_count == 0 {
            div()
                .text_size(px(t.meta))
                .text_color(gpui_kit::rgb(palette.muted))
                .child(if red_count > 0 {
                    format!("{total_count} PDF · {}", rp::kb_missing_pages_badge(red_count))
                } else {
                    rp::kb_doc_count(total_count)
                })
                .into_any_element()
        } else {
            div()
                .text_size(px(t.meta))
                .font_medium()
                .text_color(gpui_kit::rgb(palette.text))
                .child(rp::kb_selected_count(selected_count))
                .into_any_element()
        };

        let footer = div()
            .px_2p5()
            .py_2()
            .flex_shrink_0()
            .border_t_1()
            .border_color(gpui_kit::rgb(palette.border))
            .bg(chrome_bg)
            .flex()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap_1p5()
            .child(footer_left)
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_1()
                    .min_w_0()
                    .when(only_ocr_count > 0 && !ocr_active, |bar| {
                        bar.child(
                            div()
                                .id("pdf-mgr-run-only-ocr")
                                .h(px(26.))
                                .px_2p5()
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(t.body))
                                .font_medium()
                                .bg(if fill_disabled {
                                    fg_alpha(palette.text, 0.06)
                                } else if fill_is_primary {
                                    gpui_kit::rgb(palette.accent).into()
                                } else {
                                    gpui_kit::rgb(palette.surface).into()
                                })
                                .border_1()
                                .border_color(if fill_is_primary {
                                    gpui_kit::rgb(palette.accent)
                                } else {
                                    gpui_kit::rgb(palette.border)
                                })
                                .text_color(if fill_disabled {
                                    fg_alpha(palette.muted, 0.6)
                                } else if fill_is_primary {
                                    gpui_kit::rgb(palette.on_accent).into()
                                } else {
                                    gpui_kit::rgb(palette.text).into()
                                })
                                .when(!fill_disabled, |el| {
                                    el.cursor_pointer()
                                        .hover(move |s| s.opacity(0.9))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.run_pdf_ocr_batch(only_ocr_targets.clone(), true, cx);
                                        }))
                                })
                                .child(crate::ui::icon(IconName::Sparkles).w(px(12.)).h(px(12.)))
                                .child(rp::kb_fill_missing_label(only_ocr_count)),
                        )
                    })
                    .when(selected_count > 0 && !ocr_active, |bar| {
                        bar.child(
                            div()
                                .id("pdf-mgr-run-selected")
                                .h(px(26.))
                                .px_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(gpui_kit::rgb(palette.border))
                                .bg(gpui_kit::rgb(palette.surface))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(t.body))
                                .font_medium()
                                .text_color(if accurate_disabled {
                                    fg_alpha(palette.muted, 0.6)
                                } else {
                                    gpui_kit::rgb(palette.text).into()
                                })
                                .when(!accurate_disabled, |el| {
                                    el.cursor_pointer()
                                        .hover(move |s| s.bg(hover_bg))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.run_pdf_ocr_batch(
                                                selected_paths_for_run.clone(),
                                                false,
                                                cx,
                                            );
                                        }))
                                })
                                .child(crate::ui::icon(IconName::Sparkles).w(px(12.)).h(px(12.)))
                                .child(rp::kb_refine_selected(selected_count)),
                        )
                    })
                    .when(selected_count > 0, |bar| {
                        bar.child(
                            div()
                                .id("pdf-mgr-export-selected")
                                .h(px(26.))
                                .px_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(gpui_kit::rgb(palette.border))
                                .bg(gpui_kit::rgb(palette.surface))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(t.body))
                                .font_medium()
                                .text_color(if export_disabled {
                                    fg_alpha(palette.muted, 0.55)
                                } else {
                                    gpui_kit::rgb(palette.text).into()
                                })
                                .when(export_disabled, |el| {
                                    el.tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            rp::KB_EXPORT_UNAVAILABLE_TIP.get(),
                                        )
                                        .build(window, cx)
                                    })
                                })
                                .when(!export_disabled, |el| {
                                    el.cursor_pointer()
                                        .hover(move |s| s.bg(hover_bg))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.export_selected_pdf_markdowns(cx);
                                        }))
                                })
                                .child(crate::ui::icon(IconName::FileDown).w(px(12.)).h(px(12.)))
                                .child(rp::kb_export_selected_label(selected_exportable_count)),
                        )
                    })
                    .when(ocr_active, |bar| {
                        bar.child(
                            div()
                                .id("pdf-mgr-cancel-all")
                                .h(px(26.))
                                .px_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(mix_color(palette.border, palette.danger, 0.35))
                                .bg(mix_color(palette.surface, palette.danger, 0.08))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(t.body))
                                .font_medium()
                                .text_color(gpui_kit::rgb(palette.danger))
                                .cursor_pointer()
                                .hover(move |s| s.bg(mix_color(palette.surface, palette.danger, 0.16)))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.cancel_all_pdf_ocr(cx);
                                }))
                                .child(crate::ui::icon(IconName::Close).w(px(12.)).h(px(12.)))
                                .child(rp::KB_CANCEL_BTN.get()),
                        )
                    }),
            );

        div()
            .id("right-panel-pdf-manager")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(gpui_kit::rgb(palette.surface))
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
