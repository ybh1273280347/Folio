//! Markdown panes share one GPUI block editor for live editing and keep the
//! source editor only for the explicit source-mode toggle.
use super::{
    extensions::{CALLOUTS, markdown_extensions},
    pdf,
};
use crate::theme::color as rgb;
use crate::math::{MathDisplaySize, MathReady, MathRenderer, formula_id};
use crate::mermaid::{MermaidReady, MermaidRenderer};
use crate::annotations::{self, AnnotationItem, HIGHLIGHT_COLORS};
use crate::model::AppSettings;
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use gpui_kit::{
    assets::IconName,
    base::{Scrollbar, ScrollbarMode, StyledExt, text::MarkdownExtensions},
    component::{
        Sizable,
        input::{
            BufferPoint, Editor, EditorState, Input, InputEvent, InputState, Position,
            RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, RopeExt, Textarea,
            TextareaState,
        },
        menu::ContextMenuExt,
        resizable::{h_resizable, resizable_panel},
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use markdown_editor::{
    BlockEditor, BlockEditorEvent, BlockEditorModel, BlockEditorState, BlockId, BlockKind,
    BlockPosition, BlockSelection, CalloutStyle, ColumnAlign, EditorCommand, MarkChange, MarkKind,
    TableEdit,
};
use crate::rag::SearchHit;
use std::{cell::RefCell, cmp::Reverse, ops::Range, path::PathBuf, rc::Rc};

mod emoji;
mod outline;
mod view;

const PREVIEW_BYTES: usize = 8 * 1024;

pub enum MarkdownEvent {
    RenameTitle(String),
    Change(String),
    DeleteAnnotation(u64),
    Focus,
    Zoom(u8),
    OpenLink(String),
    AskAi(String),
    SendToCanvas {
        quote: String,
        occurrence: usize,
        heading_path: Vec<String>,
        line_hint: Option<usize>,
    },
    FilesChanged,
    UpdateTextColorPresets(Vec<String>),
}

enum PendingVisualJump {
    Heading(String),
    Annotation(u64),
    KbHit(SearchHit),
    Quote {
        quote: String,
        occurrence: usize,
        fallback_heading: Option<String>,
    },
}

enum PendingSourceJump {
    Heading(usize),
    KbHit(SearchHit),
}

actions!(
    folio_markdown,
    [
        ToggleSource,
        Bold,
        Italic,
        Underline,
        Strikethrough,
        InlineCode,
        Highlight,
        LinkAction,
        HeadingAction,
        BulletList,
        TaskList,
        InsertImage,
        InsertTable,
        FindInDocument,
        ReplaceInDocument
    ]
);

#[derive(Clone, Copy)]
struct SlashCommand {
    id: &'static str,
    title: &'static str,
    hint: &'static str,
    keywords: &'static [&'static str],
    icon: IconName,
}

const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        id: "text",
        title: "正文段落",
        hint: "T",
        keywords: &["text", "p", "paragraph", "zw", "zhengwen", "正文"],
        icon: IconName::Type,
    },
    SlashCommand {
        id: "h1",
        title: "一级标题",
        hint: "#",
        keywords: &["h1", "heading1", "title", "bt1", "一级标题", "标题"],
        icon: IconName::Heading1,
    },
    SlashCommand {
        id: "h2",
        title: "二级标题",
        hint: "##",
        keywords: &["h2", "heading2", "bt2", "二级标题", "标题"],
        icon: IconName::Heading2,
    },
    SlashCommand {
        id: "h3",
        title: "三级标题",
        hint: "###",
        keywords: &["h3", "heading3", "bt3", "三级标题", "标题"],
        icon: IconName::Heading3,
    },
    SlashCommand {
        id: "h4",
        title: "四级标题",
        hint: "####",
        keywords: &["h4", "heading4", "bt4", "四级标题", "标题"],
        icon: IconName::Heading4,
    },
    SlashCommand {
        id: "bullet",
        title: "无序列表",
        hint: "-",
        keywords: &["ul", "list", "bullet", "lb", "无序列表", "列表"],
        icon: IconName::List,
    },
    SlashCommand {
        id: "ordered",
        title: "有序列表",
        hint: "1.",
        keywords: &["ol", "ordered", "number", "yxlb", "有序列表"],
        icon: IconName::ListOrdered,
    },
    SlashCommand {
        id: "todo",
        title: "任务待办",
        hint: "[]",
        keywords: &["todo", "task", "check", "rw", "任务", "待办"],
        icon: IconName::ListTodo,
    },
    SlashCommand {
        id: "quote",
        title: "引用块",
        hint: ">",
        keywords: &["quote", "blockquote", "yy", "引用"],
        icon: IconName::TextQuote,
    },
    SlashCommand {
        id: "details",
        title: "可折叠区域",
        hint: "<details>",
        keywords: &["details", "fold", "collapse", "zhedie", "折叠", "可折叠区域"],
        icon: IconName::TextQuote,
    },
    SlashCommand {
        id: "code",
        title: "代码块",
        hint: "```",
        keywords: &["code", "codeblock", "dm", "代码"],
        icon: IconName::CodeXml,
    },
    SlashCommand {
        id: "table",
        title: "表格 (3×3)",
        hint: "▦",
        keywords: &["table", "grid", "bg", "表格"],
        icon: IconName::Table2,
    },
    SlashCommand {
        id: "math",
        title: "数学公式块",
        hint: "$$",
        keywords: &["math", "latex", "formula", "gs", "公式", "数学"],
        icon: IconName::Sigma,
    },
    SlashCommand {
        id: "divider",
        title: "分割线",
        hint: "---",
        keywords: &["hr", "divider", "line", "fgx", "分割线"],
        icon: IconName::Minus,
    },
    SlashCommand {
        id: "image",
        title: "插入本地图片",
        hint: "IMG",
        keywords: &["img", "image", "pic", "tp", "图片"],
        icon: IconName::Image,
    },
    SlashCommand {
        id: "emoji",
        title: "表情符号",
        hint: ":",
        keywords: &["emoji", "emote", "smile", "bq", "表情", "符号"],
        icon: IconName::Sparkles,
    },
];

fn extract_slash_query(prefix: &str) -> Option<(usize, String)> {
    let slash_pos = prefix.rfind('/')?;
    if slash_pos > 0 {
        let prev = prefix[..slash_pos].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let query = &prefix[slash_pos + 1..];
    if query.len() > 24 || query.chars().any(char::is_whitespace) {
        return None;
    }
    Some((query.len() + 1, query.to_lowercase()))
}

fn slash_command_title(cmd: &SlashCommand) -> String {
    if let Some(name) = cmd.id.strip_prefix("emoji:") {
        return match crate::i18n::locale() {
            crate::i18n::Locale::ZhCn => cmd.title.to_owned(),
            crate::i18n::Locale::En => name.replace('_', " "),
        };
    }
    match cmd.id {
        "text" => crate::i18n::markdown::SLASH_TEXT.get().to_owned(),
        "h1" => crate::i18n::markdown::SLASH_H1.get().to_owned(),
        "h2" => crate::i18n::markdown::SLASH_H2.get().to_owned(),
        "h3" => crate::i18n::markdown::SLASH_H3.get().to_owned(),
        "h4" => crate::i18n::markdown::SLASH_H4.get().to_owned(),
        "bullet" => crate::i18n::markdown::SLASH_BULLET.get().to_owned(),
        "ordered" => crate::i18n::markdown::SLASH_ORDERED.get().to_owned(),
        "todo" => crate::i18n::markdown::SLASH_TODO.get().to_owned(),
        "quote" => crate::i18n::markdown::SLASH_QUOTE.get().to_owned(),
        "details" => crate::i18n::markdown::SLASH_DETAILS.get().to_owned(),
        "code" => crate::i18n::markdown::SLASH_CODE.get().to_owned(),
        "table" => crate::i18n::markdown::SLASH_TABLE.get().to_owned(),
        "math" => crate::i18n::markdown::SLASH_MATH.get().to_owned(),
        "divider" => crate::i18n::markdown::SLASH_DIVIDER.get().to_owned(),
        "image" => crate::i18n::markdown::SLASH_IMAGE.get().to_owned(),
        "emoji" => crate::i18n::markdown::SLASH_EMOJI.get().to_owned(),
        _ => cmd.title.to_owned(),
    }
}

fn filter_slash_commands(query: &str) -> Vec<&'static SlashCommand> {
    let q = query.trim().to_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|cmd| {
            q.is_empty()
                || cmd.title.to_lowercase().contains(&q)
                || slash_command_title(cmd).to_lowercase().contains(&q)
                || cmd.id.contains(&q)
                || cmd.keywords.iter().any(|k| k.contains(&q))
        })
        .collect()
}

/// Both menus share token replacement, keyboard navigation and caret anchoring.
/// The leading colon is kept in the display query to distinguish emoji search.
fn quick_insert_items(prefix: &str) -> Option<(usize, String, Vec<&'static SlashCommand>)> {
    if let Some((len, query)) = emoji::query(prefix) {
        let items = emoji::ITEMS.iter().filter(|item| {
            query.is_empty() || item.title.contains(&query)
                || item.keywords.iter().any(|word| word.contains(&query))
        }).collect();
        return Some((len, format!(":{query}"), items));
    }
    let (len, query) = extract_slash_query(prefix)?;
    let items = filter_slash_commands(&query);
    Some((len, query, items))
}

fn hsla_for_color_name(name: &str) -> Hsla {
    match name {
        "green" => rgb(0x86efac).into(),
        "blue" => rgb(0x7dd3fc).into(),
        "pink" => rgb(0xf9a8d4).into(),
        "orange" => rgb(0xfdba74).into(),
        _ => rgb(0xfde047).into(),
    }
}

fn hsla_to_hex(color: Hsla) -> String {
    let rgba = gpui::Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b)
    )
}

struct OutlineHeading {
    range: Range<usize>,
    line: usize,
    level: u8,
    title: String,
    display_title: String,
}

#[derive(Clone)]
struct OutlineResizeDrag;

impl Render for OutlineResizeDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct MarkdownPane {
    export: Rc<RefCell<Option<pdf::ExportRequest>>>,
    export_width: Pixels,
    text: String,
    /// Whitespace-free characters of `text`, kept current for the corner badge.
    char_count: usize,
    path: PathBuf,
    title_input: Entity<InputState>,
    headings: Vec<OutlineHeading>,
    source_editor: Entity<EditorState>,
    // Each view keeps its editing state until the other view changes the buffer.
    source_needs_sync: bool,
    visual_needs_sync: bool,
    pending_source_jump: Option<PendingSourceJump>,
    source_heading_highlight: RangeDecorationCollection,
    source_heading_highlight_timer: Task<()>,
    visual_editor: Entity<BlockEditorState>,
    extensions: MarkdownExtensions,
    // The preview is never the saved document; editing resumes after the full model arrives.
    loading: Option<Task<()>>,
    source: bool,
    outline: bool,
    outline_width: f32,
    outline_drag: Option<(f32, f32)>,
    outline_initialized: bool,
    outline_user_toggled: bool,
    outline_anim_gen: u64,
    outline_closing: bool,
    outline_closing_task: Task<()>,
    outline_scroll: ScrollHandle,
    outline_collapsed: std::collections::BTreeSet<usize>,
    active_heading: Option<usize>,
    heading_click_lock_until: Option<std::time::Instant>,
    pane_outer_width: Rc<std::cell::Cell<f32>>,
    last_scroll_y: Rc<std::cell::Cell<Pixels>>,
    reading: bool,
    backdrop_visible: bool,
    zen_mode: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    settings: AppSettings,
    math: Entity<MathRenderer>,
    mermaid: Entity<MermaidRenderer>,
    formula_editor: Option<(Entity<EditorState>, bool)>,
    pending_image: Option<(PathBuf, PathBuf)>,
    image_error: Option<String>,
    // Slash menu state
    slash_index: usize,
    slash_scroll: ScrollHandle,
    dismissed_slash_prefix: Option<String>,
    // Floating selection toolbar sub-popovers
    show_turn_into: bool,
    show_color_picker: bool,
    active_highlight_color: String,
    color_preset_editing: bool,
    color_hex_editing: Option<Option<usize>>,
    color_hex_input: Entity<InputState>,
    color_hex_error: Option<String>,
    link_editing: bool,
    link_input: Entity<InputState>,
    hovered_link: Option<SharedString>,
    pending_visual_jump: Option<PendingVisualJump>,
    // Inline search state
    search_open: bool,
    search_replace_open: bool,
    search_case_sensitive: bool,
    search_whole_word: bool,
    search_input: Entity<InputState>,
    replace_input: Entity<InputState>,
    search_occurrence: usize,
    search_match_index: usize,
    search_match_total: usize,
    // Left gutter block handle & menu
    mouse_pos: Option<Point<Pixels>>,
    hovered_block_id: Option<BlockId>,
    block_menu_for: Option<(BlockId, Point<Pixels>)>,
    // Annotations (Chrome-style highlights & comments)
    annotations: Vec<AnnotationItem>,
    active_annotation_id: Option<u64>,
    selected_annotation_id: Option<u64>,
    editing_drawer_annotation_id: Option<u64>,
    annotation_list_focus: FocusHandle,
    annotation_list_scroll: ScrollHandle,
    show_annotations_drawer: bool,
    note_input: Entity<TextareaState>,
    drawer_note_input: Entity<TextareaState>,
    zoom_hud: crate::ui::ZoomHud,
    translate: crate::translate::TranslatePopover,
    _source_subscription: Subscription,
    _visual_subscription: Subscription,
    _math_subscription: Subscription,
    _mermaid_subscription: Subscription,
}

impl EventEmitter<MarkdownEvent> for MarkdownPane {}

impl MarkdownPane {
    pub fn path(&self) -> &std::path::Path { &self.path }

    /// Rename the document identity without replacing its editor or undo history.
    pub fn set_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.path = path;
        let doc_dir = self
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let image_cache = crate::image::MarkdownImageCache::default();
        self.visual_editor.update(cx, |editor, cx| {
            editor.set_image_source(
                move |uri| {
                    crate::image::resolve_markdown_image_source_cached(&doc_dir, uri, &image_cache)
                },
                cx,
            );
        });
        let title = self.path.file_stem().unwrap().to_string_lossy().into_owned();
        self.title_input.update(cx, |input, cx| input.set_value(title, window, cx));
        for annotation in &mut self.annotations {
            annotation.file = self.path.file_name().unwrap().to_string_lossy().into_owned();
        }
        cx.notify();
    }
    /// The workspace hosts this capture independently of the selected tab.
    pub fn export_element(&self) -> Option<AnyElement> {
        self.export.borrow().as_ref()?;
        if self.loading.is_some() {
            return None;
        }
        let request = self.export.clone();
        let editor = self.visual_editor.clone();
        let math = self.math.clone();
        let width = self.export_width;
        let mermaid = self.mermaid.clone();
        Some(
            div()
                .id("markdown-pdf-export")
                .absolute()
                .top_0()
                .left_0()
                .text_size(px(self.settings.markdown_font_size as f32))
                .font_family(crate::ui::sans_font(&self.settings.markdown_font))
                .child(
                    canvas(
                        move |_, window, cx| {
                            let content = BlockEditor::document(&editor, window, cx);
                            let background: Hsla = gpui_kit::rgb(crate::theme::palette().surface).into();
                            let mut element =
                                div().w(width).p_6()
                                    .bg(background)
                                    .child(content).into_any_element();
                            let size = element.layout_as_root(
                                size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                                window,
                                cx,
                            );
                            let bounds = Bounds::new(Point::default(), size);
                            window.with_document_mask(bounds, |window| {
                                element.prepaint_at(Point::default(), window, cx);
                            });
                            // Keep the page background tied to these paint operations;
                            // the worker thread cannot read the UI thread's theme palette.
                            (element, size, background)
                        },
                        move |_, (mut element, size, background), window, cx| {
                            let bounds = Bounds::new(Point::default(), size);
                            let ops = window.with_document_mask(bounds, |window| {
                                window.capture_vectors(|window| element.paint(window, cx))
                            });
                            let cancelled = request
                                .borrow()
                                .as_ref()
                                .expect("one export")
                                .cancelled
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if !cancelled
                                && (!math.read(cx).is_idle()
                                    || !mermaid.read(cx).is_idle()
                                    || ops.iter().any(|op| matches!(op, VectorPaint::PendingImage)))
                            {
                                return;
                            }
                            let faces = if cancelled {
                                Ok(Default::default())
                            } else {
                                pdf::fonts(&ops, cx)
                            };
                            let request = request.borrow_mut().take().expect("one export request");
                            std::thread::spawn(move || {
                                pdf::publish(request, ops, size, background, faces)
                            });
                        },
                    )
                    .size(px(0.)),
                )
                .into_any_element(),
        )
    }

    pub fn export_pdf(
        &mut self,
        request: pdf::ExportRequest,
        cx: &mut Context<Self>,
    ) {
        if self.source {
            self.load_visual(cx);
        }
        *self.export.borrow_mut() = Some(request);
        self.export_width = if self.settings.line_width_limited {
            self.scroll.bounds().size.width.min(px(850.))
        } else {
            self.scroll.bounds().size.width
        };
        cx.notify();
    }

    pub fn new(
        text: String,
        path: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        reading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::markdown::UNTITLED_PAGE_PLACEHOLDER.get())
                .default_value(
                    path.file_stem()
                        .expect("Markdown filename")
                        .to_string_lossy()
                        .into_owned(),
                )
        });
        cx.subscribe_in(&title_input, window, |_pane: &mut Self, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                cx.emit(MarkdownEvent::RenameTitle(input.read(cx).value().trim().to_owned()));
            }
        }).detach();
        let scroll = ScrollHandle::new();
        let mermaid = cx.new(MermaidRenderer::new);
        let extensions = markdown_extensions(&math, MathDisplaySize::Markdown, cx);
        let callout_styles: Vec<_> = CALLOUTS
            .iter()
            .map(|&(name, title, color)| CalloutStyle {
                name: name.into(),
                title: title.into(),
                color: gpui_kit::rgb(color).into(),
            })
            .collect();
        let initial_doc_dir = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let initial_image_cache = crate::image::MarkdownImageCache::default();
        let visual_editor = cx.new(|cx| {
            let mut editor = BlockEditorState::new(window, cx)
                .with_content_folding()
                .with_clipboard_policy(markdown_editor::ClipboardPolicy::Host)
                .with_image_source(move |uri| {
                    crate::image::resolve_markdown_image_source_cached(
                        &initial_doc_dir,
                        uri,
                        &initial_image_cache,
                    )
                });
            editor
                .set_markdown_extensions(extensions.clone(), cx)
                .expect("current Markdown parses with the configured GPUI extensions");
            editor.set_callout_styles(callout_styles, cx);
            editor.set_scroll_handle(scroll.clone());
            let body_size = px(settings.markdown_font_size as f32);
            editor.set_code_styles(
                crate::ui::inline_code_style(&settings, body_size),
                crate::ui::block_code_style(&settings, body_size),
                cx,
            );
            let renderer = mermaid.downgrade();
            editor.set_code_block_preview("mermaid".into(), move |id, revision, code, interactive, window, cx| {
                renderer.update(cx, |renderer, cx| renderer.element(id, revision, code, interactive, window, cx))
                    .expect("Mermaid renderer lives with its Markdown pane")
            }, cx);
            editor
        });
        let weak_pane = cx.weak_entity();
        visual_editor.update(cx, |editor, _| {
            editor.set_slash_key_handler(move |editor, key, window, cx| {
                // Completion belongs to prose; code and IME composition keep
                // their own Enter/Tab semantics instead of accepting an emoji.
                if editor.is_readonly() || editor.is_composing()
                    || editor.block_kind_at_caret() == Some(BlockKind::CodeBlock)
                    || editor.selection_window_bounds().is_some()
                {
                    return false;
                }
                let Some(prefix) = editor.caret_prefix().map(str::to_owned) else {
                    return false;
                };
                let Some((token_len, _, items)) = quick_insert_items(&prefix) else {
                    return false;
                };
                if items.is_empty() {
                    return false;
                }
                let Some(pane) = weak_pane.upgrade() else {
                    return false;
                };
                let (source, dismissed, slash_index) = {
                    let p = pane.read(cx);
                    (p.source, p.dismissed_slash_prefix.clone(), p.slash_index)
                };
                if source || dismissed.as_deref() == Some(prefix.as_str()) {
                    return false;
                }
                let count = items.len();
                match key {
                    "up" => {
                        pane.update(cx, |pane, cx| {
                            pane.slash_index = (pane.slash_index + count - 1) % count;
                            pane.scroll_slash_item_into_view(pane.slash_index);
                            cx.notify();
                        });
                        true
                    }
                    "down" => {
                        pane.update(cx, |pane, cx| {
                            pane.slash_index = (pane.slash_index + 1) % count;
                            pane.scroll_slash_item_into_view(pane.slash_index);
                            cx.notify();
                        });
                        true
                    }
                    "enter" | "tab" => {
                        let cmd_id = items[slash_index.min(count - 1)].id;
                        let weak_for_defer = weak_pane.clone();
                        window.defer(cx, move |window, cx| {
                            if let Some(pane) = weak_for_defer.upgrade() {
                                pane.update(cx, |pane, cx| {
                                    pane.execute_slash_command(cmd_id, token_len, window, cx);
                                });
                            }
                        });
                        true
                    }
                    _ => false,
                }
            });
        });
        let source_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(settings.show_line_numbers)
                .soft_wrap(true)
                .background_wrap_for_large_values(PREVIEW_BYTES)
                .searchable(true)
        });
        let source_heading_highlight = source_editor.update(cx, |editor, cx| {
            editor.create_range_decorations_collection(Vec::new(), cx)
        });
        let link_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::markdown::LINK_INPUT_PLACEHOLDER.get()));
        cx.subscribe_in(
            &link_input,
            window,
            |pane: &mut Self, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) && pane.link_editing {
                    let url = state.read(cx).value().trim().to_string();
                    pane.visual_editor.update(cx, |editor, cx| {
                        editor.set_link((!url.is_empty()).then_some(url.as_str()), cx);
                        editor.focus(window, cx);
                    });
                    pane.link_editing = false;
                    cx.notify();
                }
            },
        )
        .detach();
        let color_hex_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::markdown::COLOR_HEX_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &color_hex_input,
            window,
            |pane: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    if pane.color_hex_error.take().is_some() {
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { .. } => {
                    if pane.color_hex_editing.is_some() {
                        pane.commit_color_preset_input(cx);
                    }
                }
                _ => {}
            },
        )
        .detach();
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::markdown::SEARCH_DOC_PLACEHOLDER.get()));
        cx.subscribe_in(
            &search_input,
            window,
            |pane: &mut Self, state, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let q = state.read(cx).value().to_string();
                    pane.run_search(&q, 0, cx);
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => {
                    let q = state.read(cx).value().to_string();
                    let delta = if *shift { -1 } else { 1 };
                    pane.step_search(&q, delta, cx);
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        let replace_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::markdown::REPLACE_WITH_PLACEHOLDER.get()));
        cx.subscribe_in(
            &replace_input,
            window,
            |pane: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    pane.replace_current(cx);
                }
            },
        )
        .detach();
        let note_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .submit_on_enter(true)
                .placeholder(crate::i18n::markdown::ANNOTATION_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &note_input,
            window,
            |pane: &mut Self, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    if let Some(id) = pane.active_annotation_id {
                        let comment = state.read(cx).value().trim().to_string();
                        if let Some(item) = pane.annotations.iter_mut().find(|a| a.id == id) {
                            item.comment = comment;
                        }
                        let _ = annotations::save_annotations(&pane.path, &pane.annotations);
                        pane.active_annotation_id = None;
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        let drawer_note_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .submit_on_enter(true)
                .placeholder(crate::i18n::markdown::ANNOTATION_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &drawer_note_input,
            window,
            |pane: &mut Self, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    if let Some(id) = pane.editing_drawer_annotation_id {
                        let comment = state.read(cx).value().trim().to_string();
                        if let Some(item) = pane.annotations.iter_mut().find(|a| a.id == id) {
                            item.comment = comment;
                        }
                        let _ = annotations::save_annotations(&pane.path, &pane.annotations);
                        pane.editing_drawer_annotation_id = None;
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        let source_subscription = cx.subscribe_in(
            &source_editor,
            window,
            |pane: &mut Self, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) && pane.source {
                    // A source edit supersedes any model currently being prepared.
                    pane.loading = None;
                    pane.mermaid.update(cx, |renderer, _| renderer.retain_blocks([]));
                    pane.text = state.read(cx).value().to_string();
                    pane.char_count = crate::model::nonspace_char_count(&pane.text);
                    pane.visual_needs_sync = true;
                    pane.parse_outline();
                    cx.emit(MarkdownEvent::Change(pane.text.clone()));
                    cx.notify();
                } else if matches!(event, InputEvent::Focus) {
                    cx.emit(MarkdownEvent::Focus);
                }
            },
        );
        let visual_subscription = cx.subscribe_in(
            &visual_editor,
            window,
            |pane, editor, event: &BlockEditorEvent, window, cx| match event {
                BlockEditorEvent::Changed { .. } => {
                    pane.text = editor.read(cx).source();
                    pane.char_count = crate::model::nonspace_char_count(&pane.text);
                    pane.source_needs_sync = true;
                    pane.retain_mermaid_blocks(cx);
                    pane.parse_outline();
                    pane.slash_index = 0;
                    pane.slash_scroll.set_offset(point(px(0.), px(0.)));
                    if let Some(dismissed) = &pane.dismissed_slash_prefix
                        && editor.read(cx).caret_prefix() != Some(dismissed.as_str())
                    {
                        pane.dismissed_slash_prefix = None;
                    }
                    cx.emit(MarkdownEvent::Change(pane.text.clone()));
                    cx.notify();
                }
                BlockEditorEvent::SelectionChanged => {
                    pane.show_turn_into = false;
                    pane.close_color_picker();
                    pane.block_menu_for = None;
                    if editor.read(cx).selection_window_bounds().is_none() {
                        pane.link_editing = false;
                        // If caret landed on an existing <mark> highlight that has a saved annotation, surface it!
                        if let Some((_, _, quote)) = editor.read(cx).highlight_at_caret() {
                            if let Some(existing) =
                                pane.annotations.iter().find(|a| a.quote == quote).cloned()
                            {
                                if pane.active_annotation_id != Some(existing.id) {
                                    pane.active_annotation_id = Some(existing.id);
                                    let note_input = pane.note_input.clone();
                                    let comment = existing.comment;
                                    window.defer(cx, move |window, cx| {
                                        note_input.update(cx, |input, cx| {
                                            input.set_value(comment, window, cx);
                                        });
                                    });
                                }
                            }
                        } else {
                            pane.active_annotation_id = None;
                        }
                    }
                    cx.notify();
                }
                BlockEditorEvent::BlockDragEnded => cx.notify(),
                BlockEditorEvent::EditLinkRequested { url } => {
                    pane.link_editing = true;
                    let initial = url.as_ref().map(|u| u.to_string()).unwrap_or_default();
                    let link_input = pane.link_input.clone();
                    window.defer(cx, move |window, cx| {
                        link_input.update(cx, |input, cx| {
                            input.set_value(initial, window, cx);
                            input.focus(window, cx);
                        });
                    });
                    cx.notify();
                }
                BlockEditorEvent::LinkClicked { url } => {
                    pane.handle_link_click(url.as_ref(), window, cx);
                }
                BlockEditorEvent::LinkHovered { url } => {
                    if pane.hovered_link != *url {
                        pane.hovered_link = url.clone();
                        cx.notify();
                    }
                }
                BlockEditorEvent::PasteRequested { request, item } => {
                    let text_fallback = item.text().map(|s| s.to_string());
                    match crate::image::clipboard::extract_images_from_clipboard(item) {
                        Ok(sources) if sources.is_empty() => {
                            editor.update(cx, |editor, cx| {
                                editor.apply_paste(request, text_fallback.as_deref(), cx);
                            });
                        }
                        Ok(_) => {
                            let consumed = editor.update(cx, |editor, cx| {
                                editor.apply_paste(request, None, cx)
                            });
                            if consumed {
                                pane.paste_clipboard_images_visual(item, cx);
                            }
                        }
                        Err(err) => {
                            editor.update(cx, |editor, cx| {
                                editor.apply_paste(request, None, cx);
                            });
                            pane.image_error = Some(err);
                            cx.notify();
                        }
                    }
                }
                BlockEditorEvent::Focus => cx.emit(MarkdownEvent::Focus),
                _ => {}
            },
        );
        let math_subscription =
            cx.subscribe_in(&math, window, |pane, _, event: &MathReady, window, cx| {
                match event {
                    MathReady::Ready => pane.visual_editor.update(cx, |_, cx| cx.notify()),
                    MathReady::Edit { id, latex, display } => {
                        let selected = pane.visual_editor.update(cx, |editor, cx| {
                            editor.select_custom_node(|node| formula_id(node) == Some(*id), cx)
                        });
                        if selected {
                            let editor = cx.new(|cx| {
                                EditorState::new(window, cx)
                                    .line_number(false)
                                    .soft_wrap(true)
                                    .default_value(latex.clone())
                            });
                            let editor_focus = editor.clone();
                            window.defer(cx, move |window, cx| {
                                editor_focus.update(cx, |editor, cx| editor.focus(window, cx));
                            });
                            pane.formula_editor = Some((editor, *display));
                        }
                    }
                }
                cx.notify();
            });
        let mermaid_subscription = cx.subscribe(&mermaid, |pane, _, _: &MermaidReady, cx| {
            pane.visual_editor.update(cx, |_, cx| cx.notify());
            cx.notify();
        });
        let annotations = annotations::load_annotations(&path);
        let char_count = crate::model::nonspace_char_count(&text);
        let mut pane = Self {
            export: Rc::new(RefCell::new(None)),
            export_width: px(0.),
            text,
            char_count,
            path,
            title_input,
            headings: Vec::new(),
            source_editor,
            source_needs_sync: true,
            visual_needs_sync: false,
            pending_source_jump: None,
            source_heading_highlight,
            source_heading_highlight_timer: Task::ready(()),
            visual_editor,
            extensions,
            loading: None,
            source: false,
            outline: false,
            outline_width: outline::PANEL_WIDTH,
            outline_drag: None,
            outline_initialized: false,
            outline_user_toggled: false,
            outline_anim_gen: 0,
            outline_closing: false,
            outline_closing_task: Task::ready(()),
            outline_scroll: ScrollHandle::new(),
            outline_collapsed: std::collections::BTreeSet::new(),
            active_heading: None,
            heading_click_lock_until: None,
            pane_outer_width: Rc::new(std::cell::Cell::new(0.0)),
            last_scroll_y: Rc::new(std::cell::Cell::new(px(0.))),
            reading,
            backdrop_visible: false,
            zen_mode: false,
            focus: cx.focus_handle(),
            scroll,
            settings,
            math,
            formula_editor: None,
            pending_image: None,
            image_error: None,
            mermaid,
            slash_index: 0,
            slash_scroll: ScrollHandle::new(),
            dismissed_slash_prefix: None,
            show_turn_into: false,
            show_color_picker: false,
            active_highlight_color: "yellow".to_owned(),
            color_preset_editing: false,
            color_hex_editing: None,
            color_hex_input,
            color_hex_error: None,
            link_editing: false,
            link_input,
            hovered_link: None,
            pending_visual_jump: None,
            search_open: false,
            search_replace_open: false,
            search_case_sensitive: false,
            search_whole_word: false,
            search_input,
            replace_input,
            search_occurrence: 0,
            search_match_index: 0,
            search_match_total: 0,
            mouse_pos: None,
            hovered_block_id: None,
            block_menu_for: None,
            annotations,
            active_annotation_id: None,
            selected_annotation_id: None,
            editing_drawer_annotation_id: None,
            annotation_list_focus: cx.focus_handle(),
            annotation_list_scroll: ScrollHandle::new(),
            show_annotations_drawer: false,
            note_input,
            drawer_note_input,
            zoom_hud: crate::ui::ZoomHud::default(),
            translate: crate::translate::TranslatePopover::default(),
            _source_subscription: source_subscription,
            _visual_subscription: visual_subscription,
            _math_subscription: math_subscription,
            _mermaid_subscription: mermaid_subscription,
        };
        pane.load_visual(cx);
        pane
    }

    fn load_visual(&mut self, cx: &mut Context<Self>) {
        self.loading = None;
        self.visual_needs_sync = false;
        // Bound first-paint work in bytes, even for OCR paragraphs with no blank lines.
        // The full parse still sees the original document, preserving fences and references.
        let long = self.text.len() > PREVIEW_BYTES;
        let preview_end = if long {
            let end = self.text.floor_char_boundary(PREVIEW_BYTES);
            self.text[..end].rfind('\n').unwrap_or(end)
        } else {
            self.text.len()
        };
        self.visual_editor.update(cx, |editor, cx| {
            editor.set_readonly(long, cx);
            editor
                .set_source(&self.text[..preview_end], cx)
                .expect("Markdown preview parses with the configured GPUI extensions");
        });
        self.retain_mermaid_blocks(cx);
        if !long {
            self.parse_outline();
            self.apply_pending_visual_jump(cx);
            return;
        }
        self.headings.clear();
        let text = self.text.clone();
        let extensions = self.extensions.clone();
        let prepared = cx.background_executor().spawn(async move {
            let model = BlockEditorModel::from_markdown(&text, extensions)
                .expect("full Markdown parses with the configured GPUI extensions");
            (model, Self::outline_headings(&text))
        });
        self.loading = Some(cx.spawn(async move |this, cx| {
            let (model, headings) = prepared.await;
            let _ = this.update(cx, |pane, cx| {
                pane.visual_editor.update(cx, |editor, cx| {
                    editor.set_model(model, cx);
                    editor.set_readonly(false, cx);
                });
                pane.retain_mermaid_blocks(cx);
                pane.headings = headings;
                pane.reconcile_outline_state(cx);
                pane.loading = None;
                pane.apply_pending_visual_jump(cx);
                cx.notify();
            });
        }));
    }

    fn retain_mermaid_blocks(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<_> = self.visual_editor.read(cx).model().document().leaves()
            .filter(|block| block.code_language().is_some_and(|language| language.eq_ignore_ascii_case("mermaid")))
            .map(|block| block.id())
            .collect();
        self.mermaid.update(cx, |renderer, _| renderer.retain_blocks(ids));
    }

    fn parse_outline(&mut self) {
        self.headings = Self::outline_headings(&self.text);
        let len = self.headings.len();
        self.outline_collapsed.retain(|&ix| ix < len);
        if self.active_heading.is_some_and(|ix| ix >= len) {
            self.active_heading = (len > 0).then_some(0);
        } else if self.active_heading.is_none() && len > 0 {
            self.active_heading = Some(0);
        }
    }

    fn reconcile_outline_state(&mut self, cx: &App) {
        let len = self.headings.len();
        self.outline_collapsed.retain(|&ix| ix < len);
        self.active_heading = self
            .visual_editor
            .read(cx)
            .active_heading_index(px(96.))
            .or((len > 0).then_some(0));
        let width = self.pane_outer_width.get();
        if !self.source
            && !self.outline_user_toggled
            && !self.headings.is_empty()
            && width >= outline::AUTO_OPEN_WIDTH
        {
            self.outline = true;
            self.outline_closing = false;
        }
    }

    fn outline_headings(text: &str) -> Vec<OutlineHeading> {
        let arena = Arena::new();
        let root = parse_document(&arena, text, &Options::default());
        let lines: Vec<_> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        root.descendants()
            .filter_map(|node| {
                let data = node.data.borrow();
                let NodeValue::Heading(heading) = &data.value else {
                    return None;
                };
                let position = data.sourcepos;
                let start = lines
                    .get(position.start.line.saturating_sub(1))
                    .copied()
                    .unwrap_or(0)
                    + position.start.column.saturating_sub(1);
                let end = (lines
                    .get(position.end.line.saturating_sub(1))
                    .copied()
                    .unwrap_or(0)
                    + position.end.column)
                    .min(text.len());
                let title: String = node
                    .descendants()
                    .filter_map(|child| match &child.data.borrow().value {
                        NodeValue::Text(text) => Some(text.to_string()),
                        NodeValue::Code(code) => Some(code.literal.clone()),
                        NodeValue::SoftBreak => Some(" ".to_owned()),
                        _ => None,
                    })
                    .collect();
                let display_title = outline::clean_outline_title(&title);
                Some(OutlineHeading {
                    range: start..end,
                    line: position.start.line.saturating_sub(1),
                    level: heading.level,
                    title,
                    display_title,
                })
            })
            .collect()
    }

    pub fn is_outline_open(&self) -> bool {
        self.outline && !self.outline_closing
    }

    pub fn is_source(&self) -> bool {
        self.source
    }

    pub fn source_text(&self) -> String {
        self.text.clone()
    }

    pub fn annotation_count(&self) -> usize {
        self.annotations.len()
    }

    pub fn toggle_annotations_drawer(&mut self, cx: &mut Context<Self>) {
        self.show_annotations_drawer = !self.show_annotations_drawer;
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
    }

    pub fn set_backdrop_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.backdrop_visible = visible;
        cx.notify();
    }

    pub fn set_zen_mode(&mut self, zen: bool, cx: &mut Context<Self>) {
        if self.zen_mode != zen {
            self.zen_mode = zen;
            cx.notify();
        }
    }

    pub fn is_search_open(&self) -> bool {
        self.search_open && !self.source
    }

    pub fn outline_docked_width(&self) -> f32 {
        if self.is_outline_open() && !self.is_outline_overlay() {
            self.docked_outline_panel_width()
        } else {
            0.0
        }
    }

    fn docked_outline_panel_width(&self) -> f32 {
        if self.source {
            self.outline_width
        } else {
            outline::PANEL_WIDTH
        }
    }

    fn is_outline_rendered(&self) -> bool {
        self.outline || self.outline_closing
    }

    fn is_outline_overlay(&self) -> bool {
        let w = self.pane_outer_width.get();
        !self.source && w > 0.0 && w < outline::OVERLAY_WIDTH
    }

    pub fn toggle_outline(&mut self, cx: &mut Context<Self>) {
        self.outline_user_toggled = true;
        self.outline_initialized = true;
        if self.is_outline_open() {
            self.close_outline(cx);
        } else {
            self.outline = true;
            self.outline_closing = false;
            self.outline_closing_task = Task::ready(());
            self.outline_anim_gen = self.outline_anim_gen.wrapping_add(1);
            self.sync_active_heading(cx);
            cx.notify();
        }
    }

    fn close_outline(&mut self, cx: &mut Context<Self>) {
        if !self.outline || self.outline_closing {
            return;
        }
        self.outline_closing = true;
        self.outline_anim_gen = self.outline_anim_gen.wrapping_add(1);
        self.outline_closing_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(outline::ANIMATION_MS + 10))
                .await;
            let _ = this.update(cx, |pane, cx| {
                if pane.outline_closing {
                    pane.outline = false;
                    pane.outline_closing = false;
                    cx.notify();
                }
            });
        });
        cx.notify();
    }

    fn handle_pane_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.source || width <= 0.0 {
            return;
        }
        let prev = self.pane_outer_width.replace(width);
        let mut changed = false;

        if !self.outline_initialized {
            self.outline_initialized = true;
            if !self.outline_user_toggled
                && !self.headings.is_empty()
                && width >= outline::AUTO_OPEN_WIDTH
            {
                self.outline = true;
                self.outline_closing = false;
                changed = true;
            }
        } else {
            let was_overlay = prev > 0.0 && prev < outline::OVERLAY_WIDTH;
            let is_overlay = width < outline::OVERLAY_WIDTH;
            if was_overlay != is_overlay {
                changed = true;
            }
            if !self.outline_user_toggled {
                if is_overlay && self.is_outline_open() {
                    self.outline = false;
                    self.outline_closing = false;
                    changed = true;
                } else if !self.is_outline_open()
                    && !self.headings.is_empty()
                    && width >= outline::AUTO_OPEN_WIDTH
                    && prev < outline::AUTO_OPEN_WIDTH
                {
                    self.outline = true;
                    self.outline_closing = false;
                    self.outline_anim_gen = self.outline_anim_gen.wrapping_add(1);
                    changed = true;
                }
            }
        }

        if changed {
            cx.notify();
        }
    }

    fn sync_active_heading(&mut self, cx: &mut Context<Self>) {
        if self.source || self.headings.is_empty() {
            return;
        }
        if self
            .heading_click_lock_until
            .is_some_and(|until| std::time::Instant::now() < until)
        {
            return;
        }
        let next = self.visual_editor.read(cx).active_heading_index(px(96.));
        if next != self.active_heading {
            self.active_heading = next;
            cx.notify();
        }
    }

    fn on_outline_heading_click(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_heading = Some(index);
        self.heading_click_lock_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(260));
        self.navigate_heading(index, window, cx);
        if self.is_outline_overlay() {
            self.close_outline(cx);
        }
    }

    fn handle_link_click(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = raw.trim();
        if url.is_empty() {
            return;
        }
        if let Some(fragment) = url.strip_prefix('#') {
            self.navigate_to_anchor(fragment, window, cx);
            return;
        }
        cx.emit(MarkdownEvent::OpenLink(url.to_owned()));
    }

    pub fn navigate_to_anchor(&mut self, anchor: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading.is_some() {
            self.pending_visual_jump = Some(PendingVisualJump::Heading(anchor.to_owned()));
            return;
        }
        self.reveal_anchor(anchor, Some(window), cx);
    }

    pub(crate) fn jump_to_kb_hit(&mut self, hit: &SearchHit, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            if self.source_editor.read(cx).is_loading_wrapped_value() {
                self.pending_source_jump = Some(PendingSourceJump::KbHit(hit.clone()));
                return;
            }
            self.pending_source_jump = None;
            if let Some(range) = hit.source_keyword(&self.text) {
                self.reveal_source_hit(range, window, cx);
                return;
            }
        } else {
            if self.loading.is_some() && hit.keyword.is_some() && hit.line_start.is_some() {
                // The first-paint model can omit the hit and is replaced after the full parse.
                self.pending_visual_jump = Some(PendingVisualJump::KbHit(hit.clone()));
                return;
            }
            if self.select_kb_keyword(hit, cx) {
                return;
            }
        }

        let heading = crate::rag::pipeline::clean_heading_text(
            hit.heading.rsplit(" > ").next().unwrap(),
        );
        let snippet = (hit.keyword.is_none() || hit.line_start.is_none()).then_some(hit.snippet.as_str());
        self.jump_to_search_hit(hit.line_start, Some(&heading), snippet, window, cx);
    }

    fn select_kb_keyword(&mut self, hit: &SearchHit, cx: &mut Context<Self>) -> bool {
        let Some(range) = hit.source_keyword(&self.text) else {
            return false;
        };
        let section = self.headings.partition_point(|heading| heading.range.start <= range.start);
        let start_heading = section.checked_sub(1);
        let start = start_heading.map_or(0, |index| self.headings[index].range.start);
        let keyword = self.text[range.clone()].to_ascii_lowercase();
        let occurrence = self.text[start..range.start]
            .to_ascii_lowercase()
            .matches(&keyword)
            .count();
        self.visual_editor.update(cx, |editor, cx| {
            let start_block = start_heading.and_then(|index| editor.heading_block_at(index));
            let end_block = editor.heading_block_at(section);
            // A snippet can include text from several leaves. Compare the context on each
            // side of the token and use its section-local ordinal when contexts tie.
            let selection = editor.model().document().leaves()
                .skip_while(|block| start_block.is_some_and(|start| block.id() != start))
                .take_while(|block| end_block != Some(block.id()))
                .filter_map(|block| Some((block.id(), block.text()?)))
                .flat_map(|(block, text)| {
                    hit.keyword_occurrences(text)
                        .into_iter()
                        .map(move |(range, score)| (block, range, score))
                })
                .enumerate()
                .max_by_key(|(index, (_, _, score))| (*score, Reverse(index.abs_diff(occurrence))))
                .map(|(_, (block, range, _))| BlockSelection::Text {
                    anchor: BlockPosition::new(block, range.start),
                    head: BlockPosition::new(block, range.end),
                });
            selection.is_some_and(|selection| editor.navigate_to_selection(selection, cx).is_ok())
        })
    }

    fn reveal_source_hit(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.source_editor.update(cx, |editor, cx| {
            let point = editor.text().offset_to_point(range.start);
            editor.set_cursor_position(editor.text().offset_to_position(range.start), window, cx);
            if let Some(line_height) = editor.line_height() {
                let row = editor
                    .display_map()
                    .buffer_pos_to_display_pos(BufferPoint::new(point.row, point.column))
                    .row;
                let mut offset = editor.scroll_offset();
                offset.y = editor.input_bounds().size.height * 0.25 - line_height * row;
                editor.set_scroll_offset(offset, cx);
            }
        });
        self.source_heading_highlight.set(
            vec![RangeDecoration::new(range)
                .with_style(RangeDecorationStyle::Fill)
                .with_color(gpui_kit::base::Theme::global(cx).tokens.colors.selection)],
            cx,
        );
        self.source_heading_highlight_timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1400)).await;
            let _ = this.update(cx, |pane, cx| pane.source_heading_highlight.clear(cx));
        });
        cx.notify();
    }

    pub fn jump_to_search_hit(
        &mut self,
        line_1based: Option<usize>,
        heading_title: Option<&str>,
        snippet_phrase: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source {
            if let Some(line_num) = line_1based
                && !self.source_editor.read(cx).is_loading_wrapped_value()
            {
                if let Some(range) = crate::workspace::line_byte_range(&self.text, line_num.saturating_sub(1)) {
                    self.reveal_source_hit(range, window, cx);
                }
                return;
            }
            if let Some(heading) = heading_title
                && let Some(idx) = self.find_heading_index(heading)
            {
                self.navigate_heading(idx, window, cx);
            }
            return;
        }

        if self.loading.is_none()
            && let Some(phrase) = snippet_phrase
                .map(|s| s.trim_matches('…').trim())
                .filter(|s| !s.is_empty())
        {
            let probe: String = phrase.chars().take(24).collect();
            let matched = self.visual_editor.update(cx, |editor, cx| {
                if editor.select_quote_occurrence(&probe, 0, cx) {
                    editor.focus(window, cx);
                    true
                } else {
                    false
                }
            });
            if matched {
                cx.notify();
                return;
            }
        }
        if let Some(heading) = heading_title {
            self.navigate_to_anchor(heading, window, cx);
        }
    }

    fn reveal_anchor(&mut self, anchor: &str, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(index) = self.find_heading_index(anchor) else {
            return;
        };
        if let Some(window) = window {
            self.navigate_heading(index, window, cx);
        } else {
            self.select_visual_heading(index, None, cx);
            cx.notify();
        }
    }

    fn reveal_annotation(&mut self, id: u64, cx: &mut Context<Self>) {
        // The preview is replaced by the full model, including its selection.
        if self.loading.is_some() {
            self.pending_visual_jump = Some(PendingVisualJump::Annotation(id));
        } else if let Some(item) = self.annotations.iter().find(|item| item.id == id) {
            self.visual_editor.update(cx, |editor, cx| {
                editor.select_quote_occurrence(&item.quote, item.occurrence, cx);
            });
        }
    }

    fn apply_pending_visual_jump(&mut self, cx: &mut Context<Self>) {
        match self.pending_visual_jump.take() {
            Some(PendingVisualJump::Heading(anchor)) => self.reveal_anchor(&anchor, None, cx),
            Some(PendingVisualJump::Annotation(id)) => self.reveal_annotation(id, cx),
            Some(PendingVisualJump::KbHit(hit)) => {
                if !self.select_kb_keyword(&hit, cx) {
                    let heading = crate::rag::pipeline::clean_heading_text(
                        hit.heading.rsplit(" > ").next().unwrap(),
                    );
                    self.reveal_anchor(&heading, None, cx);
                }
            }
            Some(PendingVisualJump::Quote {
                quote,
                occurrence,
                fallback_heading,
            }) => {
                let matched = self
                    .visual_editor
                    .update(cx, |editor, cx| editor.select_quote_occurrence(&quote, occurrence, cx));
                if !matched && let Some(heading) = fallback_heading {
                    self.reveal_anchor(&heading, None, cx);
                }
            }
            None => {}
        }
    }

    pub fn jump_to_canvas_quote(
        &mut self,
        quote: &str,
        occurrence: usize,
        fallback_heading: Option<&str>,
        line_hint: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.source {
            self.jump_to_search_hit(line_hint, fallback_heading, Some(quote), window, cx);
            return true;
        }
        if self.loading.is_some() {
            self.pending_visual_jump = Some(PendingVisualJump::Quote {
                quote: quote.to_owned(),
                occurrence,
                fallback_heading: fallback_heading.map(str::to_owned),
            });
            return true;
        }
        let matched = self.visual_editor.update(cx, |editor, cx| {
            if editor.select_quote_occurrence(quote, occurrence, cx) {
                editor.focus(window, cx);
                true
            } else {
                false
            }
        });
        if matched {
            cx.notify();
            return true;
        }
        if let Some(heading) = fallback_heading {
            self.navigate_to_anchor(heading, window, cx);
        }
        false
    }

    fn active_heading_context(&self) -> (Vec<String>, Option<usize>) {
        let Some(active_ix) = self.active_heading.filter(|ix| *ix < self.headings.len()) else {
            return (Vec::new(), None);
        };
        let line_hint = Some(self.headings[active_ix].line + 1);
        let mut chain = Vec::new();
        let mut current_level = self.headings[active_ix].level;
        chain.push(self.headings[active_ix].display_title.clone());
        for heading in self.headings[..active_ix].iter().rev() {
            if heading.level < current_level {
                chain.push(heading.display_title.clone());
                current_level = heading.level;
            }
        }
        chain.reverse();
        (chain, line_hint)
    }

    fn find_heading_index(&self, anchor: &str) -> Option<usize> {
        match_heading_index(&self.headings, anchor)
    }

    fn navigate_heading(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            if self.source_editor.read(cx).is_loading_wrapped_value() {
                self.pending_source_jump = Some(PendingSourceJump::Heading(index));
                return;
            }
            let Some(heading) = self.headings.get(index) else {
                return;
            };
            let range = heading.range.clone();
            let line = heading.line;
            self.source_editor.update(cx, |editor, cx| {
                editor.set_cursor_position(Position::new(line as u32, 0), window, cx);
                if let Some(line_height) = editor.line_height() {
                    // Display rows account for wrapping before the target line.
                    let row = editor.display_map().buffer_line_to_display_row(line);
                    let mut offset = editor.scroll_offset();
                    offset.y = editor.input_bounds().size.height * 0.25 - line_height * row;
                    editor.set_scroll_offset(offset, cx);
                }
            });
            self.source_heading_highlight.set(vec![RangeDecoration::new(range)
                .with_style(RangeDecorationStyle::Fill)
                .with_color(gpui_kit::base::Theme::global(cx).tokens.colors.selection)], cx);
            self.source_heading_highlight_timer = cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1200))
                    .await;
                let _ = this.update(cx, |pane, cx| pane.source_heading_highlight.clear(cx));
            });
        } else {
            self.select_visual_heading(index, Some(window), cx);
        }
        cx.notify();
    }

    fn select_visual_heading(
        &mut self,
        index: usize,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(block) = self.visual_editor.read(cx).heading_block_at(index) else {
            return;
        };
        let visual_editor = self.visual_editor.clone();
        if let Some(window) = window {
            window.defer(cx, move |window, cx| {
                visual_editor.update(cx, |editor, cx| {
                    if editor.navigate_to_heading(block, cx).is_ok() {
                        editor.focus(window, cx);
                    }
                });
            });
        } else {
            visual_editor.update(cx, |editor, cx| {
                let _ = editor.navigate_to_heading(block, cx);
            });
        }
    }

    fn active_slash_state(&self, cx: &App) -> Option<(usize, String, Vec<&'static SlashCommand>)> {
        if self.source {
            return None;
        }
        let editor = self.visual_editor.read(cx);
        if editor.is_readonly() || editor.is_composing()
            || editor.block_kind_at_caret() == Some(BlockKind::CodeBlock)
            || editor.selection_window_bounds().is_some()
        {
            return None;
        }
        let prefix = editor.caret_prefix()?;
        if self.dismissed_slash_prefix.as_deref() == Some(prefix) {
            return None;
        }
        let (token_len, query, items) = quick_insert_items(prefix)?;
        if items.is_empty() {
            return None;
        }
        Some((token_len, query, items))
    }

    fn scroll_slash_item_into_view(&self, index: usize) {
        let item_h = 38.0;
        let viewport_h = 320.0;
        let top_y = index as f32 * item_h;
        let bottom_y = top_y + item_h;
        let current_scroll = f32::from(-self.slash_scroll.offset().y).max(0.0);

        if top_y < current_scroll {
            let target = top_y.max(0.0);
            self.slash_scroll.set_offset(point(px(0.), px(-target)));
        } else if bottom_y > current_scroll + viewport_h {
            let target = bottom_y - viewport_h + 4.0;
            self.slash_scroll.set_offset(point(px(0.), px(-target)));
        }
    }

    fn execute_slash_command(
        &mut self,
        cmd_id: &str,
        token_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let replacement = if cmd_id == "emoji" {
            Some(":")
        } else {
            emoji::ITEMS.iter().find(|item| item.id == cmd_id).map(|item| item.hint)
        };
        if let Some(replacement) = replacement {
            // Replace the search token in one edit: Undo restores it, and UTF-8
            // byte lengths stay aligned even for Chinese queries and joined emoji.
            self.visual_editor.update(cx, |editor, cx| {
                if let Ok(mut range) = editor.capture_suffix(token_len) {
                    let _ = editor.replace_captured(&mut range, replacement, cx);
                }
                editor.focus(window, cx);
            });
            cx.notify();
            return;
        }
        self.visual_editor.update(cx, |editor, cx| {
            if let Ok(mut range) = editor.capture_suffix(token_len) {
                let _ = editor.replace_captured(&mut range, "", cx);
            }
        });
        match cmd_id {
            "text" => self.run_command(EditorCommand::SetBlockKind(BlockKind::Paragraph), cx),
            "h1" => self.run_command(
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 1 }),
                cx,
            ),
            "h2" => self.run_command(
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
                cx,
            ),
            "h3" => self.run_command(
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 3 }),
                cx,
            ),
            "h4" => self.run_command(
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 4 }),
                cx,
            ),
            "bullet" => self.run_command(EditorCommand::ToggleList { ordered: false }, cx),
            "ordered" => self.run_command(EditorCommand::ToggleList { ordered: true }, cx),
            "todo" => self.run_command(EditorCommand::ToggleTask, cx),
            "quote" => self.run_command(EditorCommand::ToggleBlockquote, cx),
            "code" => self.run_command(EditorCommand::SetCodeBlock { lang: None }, cx),
            "table" => self.run_command(EditorCommand::InsertTable { rows: 3, cols: 3 }, cx),
            "divider" => self.run_command(
                EditorCommand::InsertBlock(BlockKind::HorizontalRule),
                cx,
            ),
            "details" => {
                self.visual_editor.update(cx, |editor, cx| {
                    editor.replace_selected_markdown(crate::i18n::markdown::DETAILS_TEMPLATE.get(), cx);
                    editor.focus(window, cx);
                });
            }
            "math" => {
                self.visual_editor.update(cx, |editor, cx| {
                    editor.replace_selected_markdown("$$\n\\int_a^b f(x)\\,dx\n$$", cx);
                    editor.focus(window, cx);
                });
            }
            "image" => self.insert_image(window, cx),
            _ => {}
        }
        let visual_editor = self.visual_editor.clone();
        window.defer(cx, move |window, cx| {
            visual_editor.update(cx, |editor, cx| editor.focus(window, cx));
        });
        cx.notify();
    }

    pub fn apply_settings(
        &mut self,
        settings: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.show_line_numbers != settings.show_line_numbers {
            self.source_editor.update(cx, |editor, cx| {
                editor.set_line_number(settings.show_line_numbers, window, cx);
            });
        }
        let body_size = px(settings.markdown_font_size as f32);
        let previous_size = px(self.settings.markdown_font_size as f32);
        let inline = crate::ui::inline_code_style(settings, body_size);
        let block = crate::ui::block_code_style(settings, body_size);
        if inline != crate::ui::inline_code_style(&self.settings, previous_size)
            || block != crate::ui::block_code_style(&self.settings, previous_size)
        {
            self.visual_editor.update(cx, |editor, cx| {
                editor.set_code_styles(inline, block, cx);
            });
        }
        self.settings.clone_from(settings);
        cx.notify();
    }

    pub fn sync(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.text == text {
            return;
        }
        self.text = text.to_owned();
        self.char_count = crate::model::nonspace_char_count(text);
        self.source_needs_sync = true;
        self.pending_source_jump = None;
        self.load_visual(cx);
        if self.source {
            self.load_source(window, cx);
        }
        cx.notify();
    }

    pub fn refresh_locale(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.title_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::UNTITLED_PAGE_PLACEHOLDER.get(), window, cx);
        });
        self.link_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::LINK_INPUT_PLACEHOLDER.get(), window, cx);
        });
        self.color_hex_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::COLOR_HEX_PLACEHOLDER.get(), window, cx);
        });
        self.search_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::SEARCH_DOC_PLACEHOLDER.get(), window, cx);
        });
        self.replace_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::REPLACE_WITH_PLACEHOLDER.get(), window, cx);
        });
        self.note_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::ANNOTATION_PLACEHOLDER.get(), window, cx);
        });
        self.drawer_note_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::markdown::ANNOTATION_PLACEHOLDER.get(), window, cx);
        });
    }

    pub fn sync_annotations(&mut self, items: &[AnnotationItem], cx: &mut Context<Self>) {
        // Close inputs whose record changed so stale text cannot overwrite a tool edit.
        let unchanged = |id: &u64| items.iter().find(|item| item.id == *id)
            .is_some_and(|item| self.annotations.iter().any(|old| old == item));
        self.active_annotation_id = self.active_annotation_id.filter(unchanged);
        self.editing_drawer_annotation_id = self.editing_drawer_annotation_id.filter(unchanged);
        self.selected_annotation_id = self.selected_annotation_id.filter(|id| items.iter().any(|item| item.id == *id));
        self.annotations = items.to_vec();
        cx.notify();
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            let source_editor = self.source_editor.clone();
            window.defer(cx, move |window, cx| {
                source_editor.update(cx, |editor, cx| editor.focus(window, cx));
            });
        } else {
            self.defer_focus_visual(window, cx);
        }
        cx.emit(MarkdownEvent::Focus);
    }

    fn defer_focus_visual(&self, window: &mut Window, cx: &mut Context<Self>) {
        let visual_editor = self.visual_editor.clone();
        window.defer(cx, move |window, cx| {
            visual_editor.update(cx, |editor, cx| editor.focus(window, cx));
        });
    }

    pub(super) fn show_empty_templates(&self) -> bool {
        !self.source && self.loading.is_none() && self.text.trim().is_empty()
    }

    pub(super) fn apply_empty_template(
        &mut self,
        template_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let markdown = match template_id {
            "todo" => crate::i18n::markdown::todo_template_markdown(),
            _ => return,
        };
        let synced = self.visual_editor.update(cx, |editor, cx| {
            editor.set_readonly(false, cx);
            editor.set_source(&markdown, cx)?;
            Ok::<String, SharedString>(editor.source())
        });
        if let Ok(next_text) = synced {
            self.text = next_text;
            self.char_count = crate::model::nonspace_char_count(&self.text);
            self.source_needs_sync = true;
            self.retain_mermaid_blocks(cx);
            self.parse_outline();
            cx.emit(MarkdownEvent::Change(self.text.clone()));
            self.defer_focus_visual(window, cx);
            cx.notify();
        }
    }

    fn run_search(&mut self, query: &str, index: isize, cx: &mut Context<Self>) {
        if query.is_empty() {
            self.search_occurrence = 0;
            self.search_match_index = 0;
            self.search_match_total = 0;
            return;
        }
        let case_sensitive = self.search_case_sensitive;
        let whole_word = self.search_whole_word;
        let (matched_index, total) = self.visual_editor.update(cx, |editor, cx| {
            editor.search_occurrence_with(query, index, case_sensitive, whole_word, cx)
        });
        self.search_match_index = matched_index;
        self.search_match_total = total;
        self.search_occurrence = matched_index.saturating_sub(1);
    }

    fn step_search(&mut self, query: &str, delta: isize, cx: &mut Context<Self>) {
        if query.is_empty() {
            self.run_search(query, 0, cx);
            return;
        }
        let next = self.search_occurrence as isize + delta;
        self.run_search(query, next, cx);
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            return;
        }
        let selected = self
            .visual_editor
            .read(cx)
            .selected_plain_text()
            .unwrap_or_default();
        self.search_open = true;
        if !selected.is_empty() {
            self.run_search(&selected, 0, cx);
        } else {
            let existing = self.search_input.read(cx).value().to_string();
            self.run_search(&existing, self.search_occurrence as isize, cx);
        }
        let search_input = self.search_input.clone();
        window.defer(cx, move |window, cx| {
            search_input.update(cx, |input, cx| {
                if !selected.is_empty() {
                    input.set_value(selected, window, cx);
                }
                input.focus(window, cx);
            });
        });
        cx.notify();
    }

    fn open_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            return;
        }
        let selected = self
            .visual_editor
            .read(cx)
            .selected_plain_text()
            .unwrap_or_default();
        self.search_open = true;
        self.search_replace_open = true;
        if !selected.is_empty() {
            self.run_search(&selected, 0, cx);
        } else {
            let existing = self.search_input.read(cx).value().to_string();
            self.run_search(&existing, self.search_occurrence as isize, cx);
        }
        let has_query = !selected.is_empty() || !self.search_input.read(cx).value().is_empty();
        let search_input = self.search_input.clone();
        let replace_input = self.replace_input.clone();
        window.defer(cx, move |window, cx| {
            if !selected.is_empty() {
                search_input.update(cx, |input, cx| input.set_value(selected, window, cx));
            }
            if has_query {
                replace_input.update(cx, |input, cx| input.focus(window, cx));
            } else {
                search_input.update(cx, |input, cx| input.focus(window, cx));
            }
        });
        cx.notify();
    }

    fn toggle_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_replace_open = !self.search_replace_open;
        if self.search_replace_open {
            let replace_input = self.replace_input.clone();
            window.defer(cx, move |window, cx| {
                replace_input.update(cx, |input, cx| input.focus(window, cx));
            });
        }
        cx.notify();
    }

    fn replace_current(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        let replacement = self.replace_input.read(cx).value().to_string();
        let index = self.search_occurrence as isize;
        let case_sensitive = self.search_case_sensitive;
        let whole_word = self.search_whole_word;
        let (matched_index, total) = self.visual_editor.update(cx, |editor, cx| {
            editor.replace_search_occurrence(
                &query,
                &replacement,
                index,
                case_sensitive,
                whole_word,
                cx,
            )
        });
        self.search_match_index = matched_index;
        self.search_match_total = total;
        self.search_occurrence = matched_index.saturating_sub(1);
        cx.notify();
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        let replacement = self.replace_input.read(cx).value().to_string();
        let case_sensitive = self.search_case_sensitive;
        let whole_word = self.search_whole_word;
        self.visual_editor.update(cx, |editor, cx| {
            editor.replace_all_search_occurrences(
                &query,
                &replacement,
                case_sensitive,
                whole_word,
                cx,
            )
        });
        self.search_occurrence = 0;
        self.search_match_index = 0;
        self.search_match_total = 0;
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self.search_replace_open = false;
        self.defer_focus_visual(window, cx);
        cx.notify();
    }

    fn load_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.source_editor.update(cx, |editor, cx| {
            if self.text.len() > PREVIEW_BYTES {
                editor.set_value_wrapped_in_background(self.text.clone(), window, cx);
            } else {
                editor.set_value(self.text.clone(), window, cx);
            }
        });
        self.source_needs_sync = false;
        cx.notify();
    }

    pub fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.source = !self.source;
        if self.source {
            self.hovered_link = None;
            if self.source_needs_sync {
                self.load_source(window, cx);
            }
            self.source_editor.update(cx, |editor, cx| editor.focus(window, cx));
        } else {
            self.source_editor.update(cx, |editor, _| editor.cancel_background_wrap());
            self.pending_source_jump = None;
            if self.visual_needs_sync {
                self.load_visual(cx);
            }
            self.defer_focus_visual(window, cx);
        }
        cx.emit(MarkdownEvent::Focus);
        cx.notify();
    }

    fn run_command(&mut self, command: EditorCommand, cx: &mut Context<Self>) {
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(command, cx);
        });
    }

    fn close_color_picker(&mut self) {
        self.show_color_picker = false;
        self.color_preset_editing = false;
        self.color_hex_editing = None;
        self.color_hex_error = None;
    }

    fn apply_text_color(&mut self, hex: &str, cx: &mut Context<Self>) {
        let Some(rgb_val) = crate::model::parse_rgb_hex(hex) else {
            return;
        };
        let hsla: Hsla = gpui_kit::rgb(rgb_val).into();
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().with_color(hsla)),
                cx,
            );
        });
        self.close_color_picker();
        cx.notify();
    }

    fn clear_text_color(&mut self, cx: &mut Context<Self>) {
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().clear(MarkKind::Color)),
                cx,
            );
        });
        self.close_color_picker();
        cx.notify();
    }

    fn save_text_color_presets(&mut self, presets: Vec<String>, cx: &mut Context<Self>) {
        let sanitized = crate::model::sanitize_text_color_presets(presets);
        self.settings.text_color_presets = sanitized.clone();
        cx.emit(MarkdownEvent::UpdateTextColorPresets(sanitized));
        cx.notify();
    }

    fn open_color_hex_editor(
        &mut self,
        target: Option<usize>,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.color_hex_editing = Some(target);
        self.color_hex_error = None;
        let color_hex_input = self.color_hex_input.clone();
        window.defer(cx, move |window, cx| {
            color_hex_input.update(cx, |input, cx| {
                input.set_value(initial, window, cx);
                input.focus(window, cx);
            });
        });
        cx.notify();
    }

    fn commit_color_preset_input(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.color_hex_editing else {
            return;
        };
        let raw = self.color_hex_input.read(cx).value().to_string();
        let Some(normalized) = crate::model::normalize_hex_color(&raw) else {
            self.color_hex_error = Some(crate::i18n::markdown::COLOR_HEX_INVALID.get().to_owned());
            cx.notify();
            return;
        };
        let mut presets = self.settings.text_color_presets.clone();
        match target {
            Some(ix) if ix < presets.len() => {
                presets[ix] = normalized;
            }
            _ => {
                if !presets.contains(&normalized)
                    && presets.len() < crate::model::MAX_TEXT_COLOR_PRESETS
                {
                    presets.push(normalized);
                }
            }
        }
        self.color_hex_editing = None;
        self.color_hex_error = None;
        self.save_text_color_presets(presets, cx);
    }

    fn delete_color_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut presets = self.settings.text_color_presets.clone();
        if index < presets.len() {
            presets.remove(index);
            self.color_hex_editing = None;
            self.color_hex_error = None;
            self.save_text_color_presets(presets, cx);
        }
    }

    fn reset_color_presets(&mut self, cx: &mut Context<Self>) {
        self.color_hex_editing = None;
        self.color_hex_error = None;
        self.save_text_color_presets(crate::model::default_text_color_presets(), cx);
    }

    fn apply_highlight_color(&mut self, color_name: &str, cx: &mut Context<Self>) {
        self.active_highlight_color = color_name.to_owned();
        let hsla = hsla_for_color_name(color_name);
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().with_highlight(hsla)),
                cx,
            );
        });
        self.close_color_picker();
        cx.notify();
    }

    fn clear_highlight(&mut self, cx: &mut Context<Self>) {
        self.visual_editor.update(cx, |editor, cx| {
            // A caret inside a highlight targets the whole marked span; an
            // explicit selection continues to target only the selected text.
            if let Some((block, range, _)) = editor.highlight_at_caret() {
                editor.set_selection(BlockSelection::Text {
                    anchor: BlockPosition::new(block, range.start),
                    head: BlockPosition::new(block, range.end),
                }, cx).expect("highlight range belongs to the current document");
            }
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().clear(MarkKind::Highlight)),
                cx,
            );
        });
        self.close_color_picker();
        cx.notify();
    }

    fn create_or_focus_annotation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((quote, occurrence)) = self
            .visual_editor
            .read(cx)
            .selected_quote_and_occurrence()
        else {
            return;
        };
        let color = self.active_highlight_color.clone();
        let hsla = hsla_for_color_name(&color);
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().with_highlight(hsla)),
                cx,
            );
        });
        let id = if let Some(existing) = self
            .annotations
            .iter()
            .find(|a| a.quote == quote && a.occurrence == occurrence)
        {
            existing.id
        } else {
            let next_id = self
                .annotations
                .iter()
                .map(|a| a.id)
                .max()
                .unwrap_or(0)
                + 1;
            self.annotations.push(AnnotationItem {
                id: next_id,
                file: String::new(),
                page: 0,
                quote,
                occurrence,
                color,
                comment: String::new(),
                region: None,
                rects: Vec::new(),
            });
            let _ = annotations::save_annotations(&self.path, &self.annotations);
            next_id
        };
        let comment = self
            .annotations
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.comment.clone())
            .unwrap_or_default();
        self.active_annotation_id = Some(id);
        let note_input = self.note_input.clone();
        window.defer(cx, move |window, cx| {
            note_input.update(cx, |input, cx| {
                input.set_value(comment, window, cx);
                input.focus(window, cx);
            });
        });
        cx.notify();
    }

    fn insert_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(crate::i18n::markdown::DIALOG_INSERT_IMAGE.get())
            .add_filter(crate::i18n::markdown::IMAGE_FILTER_LABEL.get(), &["png", "jpg", "jpeg", "webp", "gif", "svg"]);
        let directory = self
            .path
            .parent()
            .expect("Markdown file has parent")
            .to_path_buf();
        let pane = cx.entity();
        cx.spawn_in(window, async move |_, cx| {
            let Some(handle) = picker.pick_file().await else {
                return;
            };
            let source = handle.path().to_path_buf();
            let filename = source
                .file_name()
                .expect("selected image has name")
                .to_owned();
            let destination = directory.join(&filename);
            let _ = pane.update_in(cx, |pane, _, cx| {
                if source != destination && destination.exists() {
                    pane.pending_image = Some((source, destination));
                    cx.notify();
                    return;
                }
                if source != destination
                    && let Err(error) = std::fs::copy(source, &destination)
                {
                    pane.image_error = Some(error.to_string());
                    cx.notify();
                    return;
                }
                pane.insert_image_reference(&filename.to_string_lossy(), cx);
            });
        })
        .detach();
    }

    fn insert_image_reference(&mut self, filename: &str, cx: &mut Context<Self>) {
        let insertion = crate::i18n::markdown::image_markdown_insertion(filename);
        self.insert_visual_markdown(&insertion, cx);
    }

    fn insert_visual_markdown(&mut self, markdown: &str, cx: &mut Context<Self>) {
        self.visual_editor.update(cx, |editor, cx| {
            let Ok(mut range) = editor.capture_selection() else {
                return;
            };
            let revision = editor.revision();
            // Import the reference as an image node where the selection accepts Markdown.
            editor.replace_selected_markdown(markdown, cx);
            if editor.revision() == revision {
                // Code and table cells keep the existing text insertion behavior.
                let _ = editor.replace_captured(&mut range, markdown, cx);
            }
        });
    }

    fn save_clipboard_images_markdown(
        &mut self,
        item: &ClipboardItem,
        cx: &mut Context<Self>,
    ) -> Result<Option<String>, String> {
        let dir = crate::image::markdown_paste_image_dir(&self.path);
        let saved = crate::image::save_clipboard_images_to_dir(item, &dir)?;
        if saved.is_empty() {
            return Ok(None);
        }
        let markdown = saved
            .iter()
            .map(|saved_path| {
                let rel = crate::image::markdown_relative_image_ref(&self.path, saved_path);
                crate::i18n::markdown::image_markdown_insertion(&rel)
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.emit(MarkdownEvent::FilesChanged);
        Ok(Some(markdown))
    }

    fn paste_clipboard_images_visual(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) {
        match self.save_clipboard_images_markdown(item, cx) {
            Ok(Some(markdown)) => self.insert_visual_markdown(&markdown, cx),
            Ok(None) => {}
            Err(err) => {
                self.image_error = Some(err);
                cx.notify();
            }
        }
    }

    pub(crate) fn handle_source_paste(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match crate::image::clipboard::extract_images_from_clipboard(item) {
            Ok(sources) if sources.is_empty() => false,
            Ok(_) => match self.save_clipboard_images_markdown(item, cx) {
                Ok(Some(markdown)) => {
                    self.source_editor.update(cx, |editor, cx| {
                        editor.insert(&markdown, window, cx);
                    });
                    true
                }
                Ok(None) => false,
                Err(err) => {
                    self.image_error = Some(err);
                    cx.notify();
                    true
                }
            },
            Err(err) => {
                self.image_error = Some(err);
                cx.notify();
                true
            }
        }
    }

    fn confirm_image_import(&mut self, cx: &mut Context<Self>) {
        let (source, destination) = self.pending_image.take().expect("prompt has image paths");
        if let Err(error) = std::fs::copy(source, &destination) {
            self.image_error = Some(error.to_string());
        } else {
            let filename = destination
                .file_name()
                .expect("import destination has filename")
                .to_string_lossy();
            self.insert_image_reference(&filename, cx);
        }
        cx.notify();
    }

    fn render_floating_selection_toolbar(
        &self,
        pane_origin: Point<Pixels>,
        pane_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.source {
            return None;
        }
        let editor = self.visual_editor.read(cx);
        let sel_bounds = editor.selection_window_bounds()?;
        let marks = editor.model().active_marks();
        let block_kind = editor.top_block_at_or_active(None).map(|(_, k, _)| k);
        let active_align = editor.model().column_align_at_caret();
        let in_table = active_align.is_some();

        let block_badge = match block_kind {
            Some(BlockKind::Heading { level: 1 }) => "H1",
            Some(BlockKind::Heading { level: 2 }) => "H2",
            Some(BlockKind::Heading { level: 3 }) => "H3",
            Some(BlockKind::Heading { level: 4 }) => "H4",
            Some(BlockKind::Heading { .. }) => "H",
            Some(BlockKind::CodeBlock { .. }) => "<>",
            Some(BlockKind::Blockquote) => "❝",
            _ => "T",
        };

        let rel_x = (sel_bounds.origin.x - pane_origin.x - px(40.))
            .max(px(12.))
            .min((pane_size.width - px(560.)).max(px(12.)));
        let raw_y = sel_bounds.origin.y - pane_origin.y - px(48.);
        let rel_y = if raw_y < px(8.) {
            sel_bounds.bottom() - pane_origin.y + px(8.)
        } else {
            raw_y
        };

        let tool_btn = |id: &'static str,
                        active: bool,
                        content: AnyElement,
                        on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>| {
            div()
                .id(id)
                .w(px(28.))
                .h(px(28.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(active, |s| {
                    s.bg(rgb(0xe0ecff))
                        .text_color(rgb(0x1d4ed8))
                        .hover(|h| h.bg(rgb(0xd0e6ff)))
                        .active(|a| a.bg(rgb(0xb9d8fb)))
                })
                .when(!active, |s| {
                    s.text_color(rgb(0x1f2328))
                        .hover(|h| h.bg(rgb(0xf2f3f5)))
                        .active(|a| a.bg(rgb(crate::ui::PRESSED_BG)))
                })
                .child(content)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(on_click)
        };

        let sep = || div().w(px(1.)).h(px(16.)).mx_0p5().bg(rgb(0xe5e6eb));

        let active_hl_rgb = HIGHLIGHT_COLORS
            .iter()
            .find(|(name, _, _)| *name == self.active_highlight_color)
            .map(|(_, _, hex)| *hex)
            .unwrap_or(0xfacc15);

        let pill = div()
            .id("md-floating-toolbar")
            .h(px(38.))
            .px_2()
            .rounded(px(19.))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xe2e4e8))
            .shadow_md()
            .flex()
            .items_center()
            .gap_0p5()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // 1. Block type switcher (T)
            .child(
                div()
                    .id("ft-block-type")
                    .px_2()
                    .h(px(28.))
                    .rounded(px(14.))
                    .bg(rgb(0xf2f3f5))
                    .hover(|s| s.bg(rgb(0xe5e6eb)))
                    .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .font_family("Georgia")
                    .font_semibold()
                    .text_sm()
                    .text_color(rgb(0x1f2328))
                    .child(block_badge)
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.show_turn_into = !pane.show_turn_into;
                        pane.close_color_picker();
                        cx.notify();
                    })),
            )
            .child(sep())
            // 2. B / S / I / U / <>
            .child(tool_btn(
                "ft-bold",
                marks.has(MarkKind::Bold),
                crate::ui::icon(IconName::Bold).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::ToggleMark(MarkKind::Bold), cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-strike",
                marks.has(MarkKind::Strikethrough),
                crate::ui::icon(IconName::Strikethrough).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::ToggleMark(MarkKind::Strikethrough), cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-italic",
                marks.has(MarkKind::Italic),
                crate::ui::icon(IconName::Italic).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::ToggleMark(MarkKind::Italic), cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-underline",
                marks.has(MarkKind::Underline),
                crate::ui::icon(IconName::Underline).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::ToggleMark(MarkKind::Underline), cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-code",
                marks.has(MarkKind::Code),
                crate::ui::icon(IconName::Code).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::ToggleMark(MarkKind::Code), cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(sep())
            // 3. Align Left / Center / Right
            .child(tool_btn(
                "ft-align-left",
                !in_table || matches!(active_align, Some(ColumnAlign::Left | ColumnAlign::None)),
                crate::ui::icon(IconName::TextAlignStart).into_any_element(),
                Box::new(cx.listener(move |pane, _, window, cx| {
                    if in_table {
                        pane.run_command(
                            EditorCommand::EditTable(TableEdit::SetColumnAlign(ColumnAlign::Left)),
                            cx,
                        );
                    }
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-align-center",
                in_table && active_align == Some(ColumnAlign::Center),
                crate::ui::icon(IconName::TextAlignCenter).into_any_element(),
                Box::new(cx.listener(move |pane, _, window, cx| {
                    if in_table {
                        pane.run_command(
                            EditorCommand::EditTable(TableEdit::SetColumnAlign(
                                ColumnAlign::Center,
                            )),
                            cx,
                        );
                    }
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-align-right",
                in_table && active_align == Some(ColumnAlign::Right),
                crate::ui::icon(IconName::TextAlignEnd).into_any_element(),
                Box::new(cx.listener(move |pane, _, window, cx| {
                    if in_table {
                        pane.run_command(
                            EditorCommand::EditTable(TableEdit::SetColumnAlign(ColumnAlign::Right)),
                            cx,
                        );
                    }
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(sep())
            // 4. 'A' Text Color & Highlight Picker
            .child(
                div()
                    .id("ft-highlight-a")
                    .w(px(28.))
                    .h(px(26.))
                    .rounded(px(5.))
                    .bg(if self.show_color_picker || marks.color().is_some() || marks.highlight().is_some() {
                        rgb(0xe8f3ff)
                    } else {
                        rgb(0xf2f3f5)
                    })
                    .hover(|s| s.bg(rgb(0xd0e6ff)))
                    .active(|s| s.bg(rgb(0xb9d8fb)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(marks.color().unwrap_or_else(|| rgb(0x165dff).into()))
                            .child("A"),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(3.))
                            .rounded(px(1.5))
                            .bg(marks.highlight().unwrap_or_else(|| rgb(active_hl_rgb).into())),
                    )
                    .on_click(cx.listener(|pane, _, _, cx| {
                        if pane.show_color_picker {
                            pane.close_color_picker();
                        } else {
                            pane.show_color_picker = true;
                        }
                        pane.show_turn_into = false;
                        cx.notify();
                    })),
            )
            .child(sep())
            // 5. Indent / Outdent
            .child(tool_btn(
                "ft-indent",
                false,
                crate::ui::icon(IconName::ListIndentIncrease).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::Indent, cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(tool_btn(
                "ft-outdent",
                false,
                crate::ui::icon(IconName::ListIndentDecrease).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.run_command(EditorCommand::Outdent, cx);
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            .child(sep())
            // 6. Link
            .child(tool_btn(
                "ft-link",
                marks.has(MarkKind::Link) || self.link_editing,
                crate::ui::icon(IconName::Link).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.link_editing = !pane.link_editing;
                    if pane.link_editing {
                        let current = pane
                            .visual_editor
                            .read(cx)
                            .link_at_caret()
                            .map(|u| u.to_string())
                            .unwrap_or_default();
                        let link_input = pane.link_input.clone();
                        window.defer(cx, move |window, cx| {
                            link_input.update(cx, |input, cx| {
                                input.set_value(current, window, cx);
                                input.focus(window, cx);
                            });
                        });
                    }
                    cx.notify();
                })),
            ))
            .child(sep())
            // 7. Search selected text
            .child(tool_btn(
                "ft-search",
                self.search_open,
                crate::ui::icon(IconName::Search).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.open_search(window, cx);
                })),
            ))
            // 8. Add Comment / Annotation (💬+)
            .child(tool_btn(
                "ft-comment",
                self.active_annotation_id.is_some(),
                crate::ui::icon(IconName::MessageSquarePlus).into_any_element(),
                Box::new(cx.listener(|pane, _, window, cx| {
                    pane.create_or_focus_annotation(window, cx);
                })),
            ))
            // 9. Translate selection (译)
            .child(tool_btn(
                "ft-translate",
                self.translate.is_open(),
                div().text_xs().font_semibold().child(crate::i18n::markdown::TRANSLATE_SHORT_ICON.get()).into_any_element(),
                Box::new(cx.listener(move |pane, _, window, cx| {
                    if let Some(text) = pane.visual_editor.read(cx).selected_plain_text() {
                        let anchor = point(rel_x, rel_y + px(44.));
                        crate::translate::TranslatePopover::open_and_translate(
                            pane,
                            text,
                            Some(anchor),
                            cx,
                            |this| &mut this.translate,
                        );
                    }
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            // 10. Ask AI (问 AI)
            .child(tool_btn(
                "ft-ask-ai",
                false,
                crate::ui::icon(IconName::Sparkles).into_any_element(),
                Box::new(cx.listener(move |pane, _, window, cx| {
                    if let Some(text) = pane.visual_editor.read(cx).selected_plain_text()
                        && !text.trim().is_empty()
                    {
                        cx.emit(MarkdownEvent::AskAi(text));
                    }
                    pane.defer_focus_visual(window, cx);
                })),
            ))
            // 11. Send to Canvas (添加到画布)
            .child(tool_btn(
                "ft-send-to-canvas",
                false,
                crate::ui::icon(IconName::LayoutGrid).into_any_element(),
                Box::new(cx.listener(move |pane, _, window, cx| {
                    let visual = pane.visual_editor.read(cx);
                    let quote_pair = visual
                        .selected_quote_and_occurrence()
                        .or_else(|| {
                            visual
                                .selected_plain_text()
                                .filter(|s| !s.trim().is_empty())
                                .map(|s| (s, 0))
                        });
                    if let Some((quote, occurrence)) = quote_pair {
                        let (heading_path, line_hint) = pane.active_heading_context();
                        cx.emit(MarkdownEvent::SendToCanvas {
                            quote,
                            occurrence,
                            heading_path,
                            line_hint,
                        });
                    }
                    pane.defer_focus_visual(window, cx);
                })),
            ));

        let mut stack = div()
            .absolute()
            .left(rel_x)
            .top(rel_y)
            .flex()
            .flex_col()
            .gap_1p5()
            .child(pill);

        if self.show_turn_into {
            let turn_items: &[(&str, &str, IconName)] = &[
                ("text", crate::i18n::markdown::BLOCK_TEXT.get(), IconName::Type),
                ("h1", crate::i18n::markdown::BLOCK_H1.get(), IconName::Heading1),
                ("h2", crate::i18n::markdown::BLOCK_H2.get(), IconName::Heading2),
                ("h3", crate::i18n::markdown::BLOCK_H3.get(), IconName::Heading3),
                ("h4", crate::i18n::markdown::BLOCK_H4.get(), IconName::Heading4),
                ("bullet", crate::i18n::markdown::BLOCK_BULLET.get(), IconName::List),
                ("ordered", crate::i18n::markdown::BLOCK_ORDERED.get(), IconName::ListOrdered),
                ("todo", crate::i18n::markdown::BLOCK_TODO.get(), IconName::ListTodo),
                ("quote", crate::i18n::markdown::BLOCK_QUOTE.get(), IconName::TextQuote),
                ("code", crate::i18n::markdown::BLOCK_CODE.get(), IconName::CodeXml),
            ];
            let rows = turn_items.iter().map(|(id, label, ic)| {
                let cmd_id = *id;
                div()
                    .id(SharedString::from(format!("turn-into-{cmd_id}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(32.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(0x37352f))
                    .hover(|s| s.bg(rgb(0xefefed)))
                    .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .rounded(px(4.))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e7e3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::ui::icon(*ic).w(px(13.)).h(px(13.))),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.show_turn_into = false;
                        pane.execute_slash_command(cmd_id, 0, window, cx);
                    }))
            });
            stack = stack.child(
                div()
                    .w(px(186.))
                    .p_1p5()
                    .rounded(px(8.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe8e7e3))
                    .shadow_md()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(rows),
            );
        }

        if self.show_color_picker {
            let current_text_hex = marks.color().map(hsla_to_hex);
            let presets = &self.settings.text_color_presets;
            let preset_count = presets.len();
            let at_limit = preset_count >= crate::model::MAX_TEXT_COLOR_PRESETS;

            let text_swatches = presets.iter().enumerate().map(|(ix, hex)| {
                let hex_owned = hex.clone();
                let rgb_val = crate::model::parse_rgb_hex(hex).unwrap_or(0x1f2328);
                let selected = current_text_hex.as_deref() == Some(hex.as_str());
                let editing_this = self.color_hex_editing == Some(Some(ix));
                let ring = if editing_this {
                    rgb(0x165dff)
                } else if selected {
                    rgb(0x1f2328)
                } else {
                    rgb(0xffffff)
                };
                div()
                    .id(SharedString::from(format!("md-text-color-swatch-{ix}")))
                    .relative()
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(11.))
                    .bg(gpui_kit::rgb(rgb_val))
                    .border_2()
                    .border_color(ring)
                    .cursor_pointer()
                    .when(self.color_preset_editing, |el| {
                        let delete_ix = ix;
                        el.child(
                            div()
                                .id(SharedString::from(format!("md-text-color-del-{ix}")))
                                .absolute()
                                .top(px(-4.))
                                .right(px(-4.))
                                .w(px(13.))
                                .h(px(13.))
                                .rounded(px(7.))
                                .bg(rgb(0x1f2328))
                                .border_1()
                                .border_color(rgb(0xffffff))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child(
                                    crate::ui::icon(IconName::Close)
                                        .w(px(8.))
                                        .h(px(8.))
                                        .text_color(rgb(0xffffff)),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    cx.stop_propagation();
                                    pane.delete_color_preset(delete_ix, cx);
                                })),
                        )
                    })
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        if pane.color_preset_editing {
                            pane.open_color_hex_editor(Some(ix), hex_owned.clone(), window, cx);
                        } else {
                            pane.apply_text_color(&hex_owned, cx);
                            pane.defer_focus_visual(window, cx);
                        }
                    }))
            });

            let hl_swatches = HIGHLIGHT_COLORS.iter().map(|(name, _, hex)| {
                let cname = (*name).to_owned();
                let selected = self.active_highlight_color == *name;
                div()
                    .id(SharedString::from(format!("md-hl-swatch-{name}")))
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(11.))
                    .bg(gpui_kit::rgb(*hex))
                    .border_2()
                    .border_color(if selected {
                        rgb(0x1f2328)
                    } else {
                        rgb(0xffffff)
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.apply_highlight_color(&cname, cx);
                        pane.defer_focus_visual(window, cx);
                    }))
            });

            let preview_rgb =
                crate::model::parse_rgb_hex(self.color_hex_input.read(cx).value().as_ref());

            stack = stack.child(
                div()
                    .w(px(284.))
                    .px_3()
                    .py_2p5()
                    .rounded(px(10.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
                    .shadow_md()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Section 1: Text Foreground Color
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(rgb(0x57606a))
                                            .child(crate::i18n::markdown::TEXT_COLOR_LABEL.get()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x8c959f))
                                            .child(format!(
                                                "{}/{}",
                                                preset_count,
                                                crate::model::MAX_TEXT_COLOR_PRESETS
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .when(self.color_preset_editing, |el| {
                                        el.child(
                                            div()
                                                .id("md-tc-reset")
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded(px(4.))
                                                .text_xs()
                                                .text_color(rgb(0x57606a))
                                                .hover(|s| s.bg(rgb(0xf2f3f5)))
                                                .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                                                .cursor_pointer()
                                                .child(crate::i18n::markdown::COLOR_PRESET_RESET.get())
                                                .on_click(cx.listener(|pane, _, _, cx| {
                                                    pane.reset_color_presets(cx);
                                                })),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("md-tc-edit-toggle")
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .text_xs()
                                            .text_color(if self.color_preset_editing {
                                                rgb(0x165dff)
                                            } else {
                                                rgb(0x57606a)
                                            })
                                            .hover(|s| s.bg(rgb(0xf2f3f5)))
                                            .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                                            .cursor_pointer()
                                            .child(if self.color_preset_editing {
                                                crate::i18n::markdown::COLOR_PRESETS_DONE.get()
                                            } else {
                                                crate::i18n::markdown::COLOR_PRESETS_EDIT.get()
                                            })
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.color_preset_editing = !pane.color_preset_editing;
                                                if !pane.color_preset_editing {
                                                    pane.color_hex_editing = None;
                                                    pane.color_hex_error = None;
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("md-tc-clear")
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .text_xs()
                                            .text_color(rgb(0x57606a))
                                            .hover(|s| s.bg(rgb(0xf2f3f5)))
                                            .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                                            .cursor_pointer()
                                            .child(crate::i18n::common::CLEAR.get())
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.clear_text_color(cx);
                                                pane.defer_focus_visual(window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .children(text_swatches)
                            .when(!at_limit, |el| {
                                el.child(
                                    div()
                                        .id("md-tc-add")
                                        .w(px(22.))
                                        .h(px(22.))
                                        .rounded(px(11.))
                                        .border_1()
                                        .border_color(rgb(0xd0d7de))
                                        .bg(rgb(0xf8f9fa))
                                        .hover(|s| s.bg(rgb(0xeeeeec)))
                                        .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .child(
                                            crate::ui::icon(IconName::Plus)
                                                .w(px(12.))
                                                .h(px(12.))
                                                .text_color(rgb(0x57606a)),
                                        )
                                        .tooltip(|window, cx| {
                                            Tooltip::new(crate::i18n::markdown::COLOR_PRESET_ADD.get())
                                                .build(window, cx)
                                        })
                                        .on_click(cx.listener(|pane, _, window, cx| {
                                            pane.open_color_hex_editor(
                                                None,
                                                "#".to_owned(),
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                            }),
                    )
                    .when(self.color_hex_editing.is_some(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .pt_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .w(px(18.))
                                                .h(px(18.))
                                                .rounded(px(9.))
                                                .bg(gpui_kit::rgb(preview_rgb.unwrap_or(0xf2f3f5)))
                                                .border_1()
                                                .border_color(if preview_rgb.is_some() {
                                                    rgb(0xd0d7de)
                                                } else {
                                                    rgb(0xdc2626)
                                                }),
                                        )
                                        .child(Input::new(&self.color_hex_input).flex_1().min_w_0())
                                        .child(
                                            div()
                                                .id("md-tc-hex-save")
                                                .w(px(24.))
                                                .h(px(24.))
                                                .rounded(px(5.))
                                                .bg(rgb(0x165dff))
                                                .hover(|s| s.bg(rgb(0x0e4ad9)))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .child(
                                                    crate::ui::icon(IconName::Check)
                                                        .w(px(13.))
                                                        .h(px(13.))
                                                        .text_color(rgb(0xffffff)),
                                                )
                                                .on_click(cx.listener(|pane, _, _, cx| {
                                                    pane.commit_color_preset_input(cx);
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("md-tc-hex-cancel")
                                                .w(px(24.))
                                                .h(px(24.))
                                                .rounded(px(5.))
                                                .hover(|s| s.bg(rgb(0xf2f3f5)))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .child(
                                                    crate::ui::icon(IconName::Close)
                                                        .w(px(13.))
                                                        .h(px(13.))
                                                        .text_color(rgb(0x57606a)),
                                                )
                                                .on_click(cx.listener(|pane, _, _, cx| {
                                                    pane.color_hex_editing = None;
                                                    pane.color_hex_error = None;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .when_some(self.color_hex_error.clone(), |col, err| {
                                    col.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xdc2626))
                                            .child(err),
                                    )
                                }),
                        )
                    })
                    .child(div().h(px(1.)).bg(rgb(0xeeeeec)))
                    // Section 2: Highlight Background
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(rgb(0x57606a))
                                    .child(crate::i18n::markdown::HIGHLIGHTER_LABEL.get()),
                            )
                            .child(
                                div()
                                    .id("md-hl-clear")
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .text_xs()
                                    .text_color(rgb(0x57606a))
                                    .hover(|s| s.bg(rgb(0xf2f3f5)))
                                    .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                                    .cursor_pointer()
                                    .child(crate::i18n::common::CLEAR.get())
                                    .on_click(cx.listener(|pane, _, window, cx| {
                                        pane.clear_highlight(cx);
                                        pane.defer_focus_visual(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .children(hl_swatches),
                    ),
            );
        }

        if self.link_editing {
            stack = stack.child(
                div()
                    .w(px(320.))
                    .p_2()
                    .rounded(px(8.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
                    .shadow_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&self.link_input).flex_1().min_w_0())
                    .child(
                        crate::ui::button_primary("apply-md-link", crate::i18n::common::CONFIRM.get()).on_click(cx.listener(
                            |pane, _, window, cx| {
                                let url = pane.link_input.read(cx).value().trim().to_string();
                                pane.visual_editor.update(cx, |editor, cx| {
                                    editor.set_link((!url.is_empty()).then_some(url.as_str()), cx);
                                });
                                pane.defer_focus_visual(window, cx);
                                pane.link_editing = false;
                                cx.notify();
                            },
                        )),
                    ),
            );
        }

        Some(deferred(stack).into_any_element())
    }

    fn render_slash_menu(
        &self,
        pane_origin: Point<Pixels>,
        pane_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (token_len, query, items) = self.active_slash_state(cx)?;
        let caret = self.visual_editor.read(cx).caret_window_bounds()?;
        let active_ix = self.slash_index.min(items.len().saturating_sub(1));

        let menu_w = px(288.);
        let rel_x = (caret.origin.x - pane_origin.x)
            .max(px(16.))
            .min((pane_size.width - menu_w - px(16.)).max(px(16.)));
        let below_y = caret.bottom() - pane_origin.y + px(6.);
        let header_h = if query.is_empty() { 12. } else { 42. };
        let menu_h = px((items.len().min(8) as f32) * 38. + header_h);
        let rel_y = if below_y + menu_h > pane_size.height && caret.origin.y - pane_origin.y > menu_h
        {
            caret.origin.y - pane_origin.y - menu_h - px(6.)
        } else {
            below_y
        };

        let rows = items.iter().enumerate().map(|(ix, cmd)| {
            let cmd_id = cmd.id;
            let selected = ix == active_ix;
            div()
                .id(SharedString::from(format!("slash-item-{cmd_id}")))
                .flex()
                .items_center()
                .justify_between()
                .px_2p5()
                .h(px(36.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(selected, |s| {
                    s.bg(rgb(0xf2f2f0))
                        .active(|a| a.bg(rgb(0xeae9e6)))
                })
                .when(!selected, |s| {
                    s.hover(|h| h.bg(rgb(0xf7f7f5)))
                        .active(|a| a.bg(rgb(0xeae9e6)))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2p5()
                        .child(
                            div()
                                .w(px(20.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(if selected {
                                    rgb(0x37352f)
                                } else {
                                    rgb(0x5c5b57)
                                })
                                .child(if cmd.id.starts_with("emoji:") {
                                    div()
                                        .font_family("Segoe UI Emoji")
                                        .text_size(px(16.))
                                        .child(cmd.hint)
                                        .into_any_element()
                                } else {
                                    crate::ui::icon(cmd.icon)
                                        .w(px(15.))
                                        .h(px(15.))
                                        .into_any_element()
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(13.5))
                                .text_color(rgb(0x37352f))
                                .child(slash_command_title(cmd)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_family("Consolas")
                        .min_w(px(24.))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if selected {
                            rgb(0xe6e6e3)
                        } else {
                            rgb(0xf5f5f4)
                        })
                        .text_color(if selected {
                            rgb(0x8c8b87)
                        } else {
                            rgb(0xa8a7a3)
                        })
                        .child(cmd.id.strip_prefix("emoji:").unwrap_or(cmd.hint)),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |pane, _, window, cx| {
                    pane.execute_slash_command(cmd_id, token_len, window, cx);
                }))
        });

        let header_label = if let Some(query) = query.strip_prefix(':') {
            if query.is_empty() {
                Some(crate::i18n::markdown::EMOJI_HEADER_EMPTY.get().to_owned())
            } else {
                Some(crate::i18n::markdown::emoji_header_query(query))
            }
        } else if query.is_empty() {
            None
        } else {
            Some(crate::i18n::markdown::slash_header_query(&query))
        };

        Some(
            deferred(
                div()
                    .id("md-slash-menu")
                    .absolute()
                    .left(rel_x)
                    .top(rel_y)
                    .w(menu_w)
                    .p_1p5()
                    .rounded(px(12.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xeae9e5))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when_some(header_label, |el, label| {
                        el.child(
                            div()
                                .px_2p5()
                                .pt_1()
                                .pb_1p5()
                                .border_b_1()
                                .border_color(rgb(0xf1f0eb))
                                .text_size(px(11.))
                                .text_color(rgb(0x9b9a97))
                                .child(label),
                        )
                    })
                    .child(
                        div()
                            .id("slash-menu-items")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .track_scroll(&self.slash_scroll)
                            .on_scroll(cx.listener(|_, _: &ScrollEvent, _, cx| {
                                cx.stop_propagation();
                            }))
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .children(rows),
                    ),
            )
            .into_any_element(),
        )
    }

    fn render_left_block_handle(
        &self,
        pane_origin: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.visual_editor.read(cx);
        if self.source || editor.is_block_dragging() {
            return None;
        }
        let (top_id, kind, bounds) = editor.top_block_at_or_active(self.mouse_pos)?;
        let badge = match kind {
            BlockKind::Heading { level: 1 } => "H1",
            BlockKind::Heading { level: 2 } => "H2",
            BlockKind::Heading { level: 3 } => "H3",
            BlockKind::Heading { level: 4 } => "H4",
            BlockKind::Heading { .. } => "H",
            BlockKind::List { ordered: true, .. } => "1.",
            BlockKind::List { ordered: false, .. } => "•",
            BlockKind::Blockquote => "❝",
            BlockKind::CodeBlock { .. } => "<>",
            BlockKind::Table => "▦",
            _ => "T",
        };
        let gutter_offset = if kind == BlockKind::Table {
            px(52.)
        } else if kind == BlockKind::Blockquote {
            px(72.)
        } else {
            px(46.)
        };
        let rel_x = (bounds.origin.x - pane_origin.x - gutter_offset).max(px(2.));
        let rel_y = bounds.origin.y - pane_origin.y + px(1.);

        let handle = div()
            .id(("md-block-gutter-handle", top_id.as_u64()))
            .absolute()
            .left(rel_x)
            .top(rel_y)
            .h(px(24.))
            .px_1()
            .rounded(px(5.))
            .flex()
            .items_center()
            .gap_0p5()
            .cursor(CursorStyle::OpenHand)
            .text_color(rgb(0x2b6cb0))
            .hover(|s| s.bg(rgb(0xeef4fb)))
            .active(|s| s.bg(rgb(0xdde9f6)))
            .child(
                div()
                    .font_family("Georgia")
                    .font_semibold()
                    .text_xs()
                    .child(badge),
            )
            .child(crate::ui::icon(IconName::GripVertical))
            .on_mouse_down(MouseButton::Left, cx.listener(move |pane, event: &MouseDownEvent, _, cx| {
                // Only arm the drag here. A press below the drag threshold is still a menu click.
                pane.visual_editor.update(cx, |editor, _| {
                    editor.arm_top_block_drag(top_id, event.position);
                });
                cx.stop_propagation();
            }))
            .on_click(cx.listener(move |pane, ev: &ClickEvent, _, cx| {
                if pane.visual_editor.read(cx).is_block_dragging() {
                    return;
                }
                if pane.block_menu_for.is_some_and(|(id, _)| id == top_id) {
                    pane.block_menu_for = None;
                } else {
                    pane.block_menu_for = Some((top_id, ev.mouse_position().unwrap_or_default()));
                }
                cx.notify();
            }));

        if let Some((menu_block, _)) = self.block_menu_for
            && menu_block == top_id
        {
            let items: &[(&str, EditorCommand, IconName)] = &[
                (
                    crate::i18n::markdown::TURN_INTO_TEXT.get(),
                    EditorCommand::SetBlockKind(BlockKind::Paragraph),
                    IconName::Type,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_H1.get(),
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 1 }),
                    IconName::Heading1,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_H2.get(),
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
                    IconName::Heading2,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_H3.get(),
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 3 }),
                    IconName::Heading3,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_BULLET.get(),
                    EditorCommand::ToggleList { ordered: false },
                    IconName::List,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_TODO.get(),
                    EditorCommand::ToggleTask,
                    IconName::ListTodo,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_QUOTE.get(),
                    EditorCommand::ToggleBlockquote,
                    IconName::TextQuote,
                ),
                (
                    crate::i18n::markdown::TURN_INTO_CODE.get(),
                    EditorCommand::SetCodeBlock { lang: None },
                    IconName::CodeXml,
                ),
            ];
            let rows = items.iter().enumerate().map(|(ix, (label, cmd, ic))| {
                let command = cmd.clone();
                div()
                    .id(("block-menu-item", ix))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(32.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(0x37352f))
                    .hover(|s| s.bg(rgb(0xefefed)))
                    .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .rounded(px(4.))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e7e3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::ui::icon(*ic).w(px(13.)).h(px(13.))),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.block_menu_for = None;
                        pane.visual_editor.update(cx, |editor, cx| {
                            editor.run_on_top_block(top_id, command.clone(), cx);
                        });
                        pane.defer_focus_visual(window, cx);
                        cx.notify();
                    }))
            });
            let menu = div()
                .id("md-block-action-menu")
                .absolute()
                .left(rel_x)
                .top(rel_y + px(26.))
                .w(px(186.))
                .p_1p5()
                .rounded(px(8.))
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0xe8e7e3))
                .shadow_md()
                .flex()
                .flex_col()
                .gap_0p5()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows)
                .child(div().h(px(1.)).my_1().bg(rgb(0xf1f0eb)))
                .child(
                    div()
                        .id("block-menu-delete")
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .h(px(32.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0xd93025))
                        .hover(|s| s.bg(rgb(0xfff1f0)))
                        .active(|s| s.bg(rgb(0xffdcd8)))
                        .child(
                            div()
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(4.))
                                .bg(rgb(0xfff1f0))
                                .border_1()
                                .border_color(rgb(0xffccc7))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(crate::ui::icon(IconName::Trash).w(px(13.)).h(px(13.))),
                        )
                        .child(crate::i18n::markdown::DELETE_BLOCK.get())
                        .on_click(cx.listener(move |pane, _, window, cx| {
                            pane.block_menu_for = None;
                            pane.visual_editor.update(cx, |editor, cx| {
                                editor.delete_top_block(top_id, cx);
                            });
                            pane.defer_focus_visual(window, cx);
                            cx.notify();
                        })),
                );
            return Some(
                div()
                    .child(handle)
                    .child(deferred(menu))
                    .into_any_element(),
            );
        }

        Some(handle.into_any_element())
    }

    fn render_active_annotation_popover(
        &self,
        pane_origin: Point<Pixels>,
        pane_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.active_annotation_id?;
        let item = self.annotations.iter().find(|a| a.id == id)?.clone();
        let anchor = self
            .visual_editor
            .read(cx)
            .selection_window_bounds()
            .or_else(|| self.visual_editor.read(cx).caret_window_bounds())?;

        let quote_font_size = 12.0;
        let comment_font_size = 12.5;
        let popover_w = 308.0;

        let rel_x = (anchor.origin.x - pane_origin.x)
            .max(px(16.))
            .min((pane_size.width - px(popover_w + 16.)).max(px(16.)));
        let rel_y = (anchor.bottom() - pane_origin.y + px(8.)).min(pane_size.height - px(140.));

        let swatches = HIGHLIGHT_COLORS.iter().map(|(name, _, hex)| {
            let cname = (*name).to_owned();
            let selected = item.color == *name;
            div()
                .id(SharedString::from(format!("ann-color-{name}")))
                .w(px(16.))
                .h(px(16.))
                .rounded(px(8.))
                .bg(gpui_kit::rgb(*hex))
                .border_2()
                .border_color(if selected {
                    rgb(0x1f2328)
                } else {
                    rgb(0xffffff)
                })
                .cursor_pointer()
                .on_click(cx.listener(move |pane, _, _, cx| {
                    if let Some(a) = pane.annotations.iter_mut().find(|a| a.id == id) {
                        a.color = cname.clone();
                    }
                    let _ = annotations::save_annotations(&pane.path, &pane.annotations);
                    pane.apply_highlight_color(&cname, cx);
                }))
        });

        Some(
            deferred(
                div()
                    .id("md-annotation-popover")
                    .absolute()
                    .left(rel_x)
                    .top(rel_y)
                    .w(px(popover_w))
                    .p_3()
                    .rounded(px(10.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
                    .shadow_md()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .children(swatches),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        crate::ui::control("delete-md-annotation", IconName::Trash)
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(MarkdownEvent::DeleteAnnotation(id));
                                            })),
                                    )
                                    .child(
                                        crate::ui::control("close-md-annotation", IconName::Close)
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.active_annotation_id = None;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(quote_font_size))
                            .text_color(rgb(0x787774))
                            .text_ellipsis()
                            .child(format!("“{}”", item.quote)),
                    )
                    .child(Textarea::new(&self.note_input).w_full().text_size(px(comment_font_size)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(
                                crate::ui::button_primary("save-md-annotation", crate::i18n::markdown::SAVE_ANNOTATION.get())
                                    .font_family(crate::ui::sans_font(&self.settings.interface_font))
                                    .text_size(px(12.))
                                    .min_h(px(26.))
                                    .px(px(10.))
                                    .py(px(3.))
                                    .on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        let comment =
                                            pane.note_input.read(cx).value().trim().to_string();
                                        if let Some(a) =
                                            pane.annotations.iter_mut().find(|a| a.id == id)
                                        {
                                            a.comment = comment;
                                        }
                                        let _ = annotations::save_annotations(
                                            &pane.path,
                                            &pane.annotations,
                                        );
                                        pane.active_annotation_id = None;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }
}

fn action(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    crate::ui::button(id, label)
}


fn match_heading_index(headings: &[OutlineHeading], anchor: &str) -> Option<usize> {
    let decoded = crate::files::percent_decode(anchor);
    let target = decoded.trim();
    if target.is_empty() {
        return None;
    }
    if let Some(index) = headings
        .iter()
        .position(|heading| heading.title.trim().eq_ignore_ascii_case(target))
    {
        return Some(index);
    }
    let target_slug = heading_slug(target);
    if !target_slug.is_empty()
        && let Some(index) = headings
            .iter()
            .position(|heading| heading_slug(&heading.title) == target_slug)
    {
        return Some(index);
    }
    let collapsed = collapse_hyphens(&target_slug);
    if !collapsed.is_empty() {
        return headings
            .iter()
            .position(|heading| collapse_hyphens(&heading_slug(&heading.title)) == collapsed);
    }
    None
}

fn heading_slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

fn collapse_hyphens(slug: &str) -> String {
    let mut out = String::with_capacity(slug.len());
    let mut prev_hyphen = true;
    for ch in slug.chars() {
        if ch == '-' {
            if !prev_hyphen {
                out.push('-');
                prev_hyphen = true;
            }
        } else {
            out.push(ch);
            prev_hyphen = false;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{OutlineHeading, match_heading_index};

    #[test]
    fn heading_anchor_matches_exact_slug_and_percent_encoded() {
        let headings = vec![
            OutlineHeading {
                range: 0..10,
                line: 0,
                level: 1,
                title: "1. 背景介绍".to_owned(),
                display_title: "1. 背景介绍".to_owned(),
            },
            OutlineHeading {
                range: 12..30,
                line: 2,
                level: 2,
                title: "Quick Start Guide".to_owned(),
                display_title: "Quick Start Guide".to_owned(),
            },
        ];
        assert_eq!(match_heading_index(&headings, "1.%20背景介绍"), Some(0));
        assert_eq!(match_heading_index(&headings, "1-背景介绍"), Some(0));
        assert_eq!(match_heading_index(&headings, "quick-start-guide"), Some(1));
        assert_eq!(match_heading_index(&headings, "Quick Start Guide"), Some(1));
        assert_eq!(match_heading_index(&headings, "missing"), None);
    }

    #[test]
    fn reopens_a_saved_resized_image_as_editable_content() {
        use super::{BlockEditorModel, MarkdownExtensions};
        use markdown_editor::BlockKind;

        let saved = r#"# 递推式

<img src="images/复杂度表达式/图.png" alt="Image" width="802px" height="486.52518px" />

下一段"#;
        let model = BlockEditorModel::from_markdown(saved, MarkdownExtensions::default()).unwrap();
        let document = model.document();
        let mut leaves = document.leaves();
        leaves.next().unwrap();
        let image = leaves.next().unwrap();
        assert_eq!(image.kind(), BlockKind::Paragraph);
        assert_eq!(image.text(), Some("\u{FFFC}"));
        assert_eq!(leaves.next().unwrap().text(), Some("下一段"));
        assert_eq!(model.source(), saved);
    }

    #[test]
    fn text_color_round_trips_and_toggles_in_editor_model() {
        use super::{
            BlockEditorModel, BlockPosition, BlockSelection, EditorCommand, MarkChange, MarkKind,
            MarkdownExtensions, hsla_to_hex,
        };
        let src = r##"a <span style="color: #dc2626">red</span> b"##;
        let model = BlockEditorModel::from_markdown(src, MarkdownExtensions::default()).unwrap();
        assert_eq!(model.source(), src);

        let named = BlockEditorModel::from_markdown(
            r##"<span style="color: red">x</span>"##,
            MarkdownExtensions::default(),
        )
        .unwrap();
        assert_eq!(named.source(), r##"<span style="color: #ef4444">x</span>"##);

        let mut editable =
            BlockEditorModel::from_markdown("hello world", MarkdownExtensions::default()).unwrap();
        let leaf = editable.document().leaves().next().unwrap().id();
        editable
            .set_selection(BlockSelection::Text {
                anchor: BlockPosition::new(leaf, 0),
                head: BlockPosition::new(leaf, 5),
            })
            .unwrap();
        let blue: gpui_kit::Hsla = gpui_kit::rgb(0x2563eb).into();
        editable
            .run(EditorCommand::ApplyMark(MarkChange::new().with_color(blue)))
            .unwrap();
        assert_eq!(
            editable.source(),
            r##"<span style="color: #2563eb">hello</span> world"##
        );
        assert_eq!(editable.active_marks().color().map(hsla_to_hex).as_deref(), Some("#2563eb"));

        editable
            .run(EditorCommand::ApplyMark(MarkChange::new().clear(MarkKind::Color)))
            .unwrap();
        assert_eq!(editable.source(), "hello world");
        assert!(!editable.active_marks().has(MarkKind::Color));
    }
}

