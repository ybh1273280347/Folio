//! Markdown panes share one GPUI block editor for live editing and keep the
//! source editor only for the explicit source-mode toggle.
use crate::math::{MathReady, MathRenderer, formula_id};
use crate::model::{AnnotationItem, AppSettings, HIGHLIGHT_COLORS};
use crate::storage;
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use gpui_kit::{
    assets::IconName,
    base::{Scrollbar, ScrollbarMode, StyledExt, text::MarkdownExtensions},
    component::{
        input::{Editor, EditorState, Input, InputEvent, InputState, Position},
        menu::ContextMenuExt,
    },
    prelude::FluentBuilder,
    *,
};
use markdown_editor::{
    BlockEditor, BlockEditorEvent, BlockEditorState, BlockId, BlockKind, BlockPosition,
    BlockSelection, ColumnAlign, EditorCommand, MarkChange, MarkKind, TableEdit,
};
use std::{cell::RefCell, ops::Range, path::PathBuf, rc::Rc};

pub enum MarkdownEvent {
    Change(String),
    Focus,
    Zoom(u8),
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
        InsertTable
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

fn filter_slash_commands(query: &str) -> Vec<&'static SlashCommand> {
    let q = query.trim().to_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|cmd| {
            q.is_empty()
                || cmd.title.to_lowercase().contains(&q)
                || cmd.id.contains(&q)
                || cmd.keywords.iter().any(|k| k.contains(&q))
        })
        .collect()
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

struct OutlineHeading {
    range: Range<usize>,
    level: u8,
    title: String,
}

pub struct MarkdownPane {
    export: Rc<RefCell<Option<crate::markdown_pdf::ExportRequest>>>,
    export_width: Pixels,
    text: String,
    path: PathBuf,
    headings: Vec<OutlineHeading>,
    source_editor: Entity<EditorState>,
    visual_editor: Entity<BlockEditorState>,
    source: bool,
    outline: bool,
    reading: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    settings: AppSettings,
    math: Entity<MathRenderer>,
    formula_editor: Option<(Entity<EditorState>, bool)>,
    pending_image: Option<(PathBuf, PathBuf)>,
    image_error: Option<String>,
    // Slash menu state
    slash_index: usize,
    dismissed_slash_prefix: Option<String>,
    // Floating selection toolbar sub-popovers
    show_turn_into: bool,
    show_color_picker: bool,
    active_highlight_color: String,
    link_editing: bool,
    link_input: Entity<InputState>,
    // Inline search state
    search_open: bool,
    search_input: Entity<InputState>,
    search_occurrence: usize,
    // Left gutter block handle & menu
    mouse_pos: Option<Point<Pixels>>,
    hovered_block_id: Option<BlockId>,
    block_menu_for: Option<(BlockId, Point<Pixels>)>,
    // Annotations (Chrome-style highlights & comments)
    annotations: Vec<AnnotationItem>,
    active_annotation_id: Option<u64>,
    show_annotations_drawer: bool,
    note_input: Entity<InputState>,
    _source_subscription: Subscription,
    _visual_subscription: Subscription,
    _math_subscription: Subscription,
}

impl EventEmitter<MarkdownEvent> for MarkdownPane {}

impl MarkdownPane {
    /// The workspace hosts this capture independently of the selected tab.
    pub fn export_element(&self) -> Option<AnyElement> {
        self.export.borrow().as_ref()?;
        let request = self.export.clone();
        let editor = self.visual_editor.clone();
        let math = self.math.clone();
        let width = self.export_width;
        Some(
            div()
                .id("markdown-pdf-export")
                .absolute()
                .top_0()
                .left_0()
                .text_size(px(self.settings.markdown_font_size as f32))
                .font_family(if self.settings.interface_font.is_empty() {
                    SharedString::from("Segoe UI")
                } else {
                    self.settings.interface_font.clone().into()
                })
                .child(
                    canvas(
                        move |_, window, cx| {
                            let content = BlockEditor::document(&editor, window, cx);
                            let mut element =
                                div().w(width).p_6().child(content).into_any_element();
                            let size = element.layout_as_root(
                                size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                                window,
                                cx,
                            );
                            let bounds = Bounds::new(Point::default(), size);
                            window.with_document_mask(bounds, |window| {
                                element.prepaint_at(Point::default(), window, cx);
                            });
                            (element, size)
                        },
                        move |_, (mut element, size), window, cx| {
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
                                    || ops.iter().any(|op| matches!(op, VectorPaint::PendingImage)))
                            {
                                return;
                            }
                            let faces = if cancelled {
                                Ok(Default::default())
                            } else {
                                crate::markdown_pdf::fonts(&ops, cx)
                            };
                            let request = request.borrow_mut().take().expect("one export request");
                            std::thread::spawn(move || {
                                crate::markdown_pdf::publish(request, ops, size, faces)
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
        request: crate::markdown_pdf::ExportRequest,
        cx: &mut Context<Self>,
    ) {
        if self.source {
            self.visual_editor.update(cx, |editor, cx| {
                editor
                    .set_source(&self.text, cx)
                    .expect("current Markdown parses")
            });
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
        let scroll = ScrollHandle::new();
        math.update(cx, |renderer, cx| renderer.set_appearance(&settings, cx));
        let extensions = MarkdownExtensions::default()
            .plugin(math.read(cx).plugin(false, math.clone()))
            .plugin(math.read(cx).plugin(true, math.clone()));
        let visual_editor = cx.new(|cx| {
            let mut editor = BlockEditorState::new(window, cx);
            editor
                .set_markdown_extensions(extensions, cx)
                .expect("current Markdown parses with the configured GPUI extensions");
            editor
                .set_source(&text, cx)
                .expect("current Markdown parses with the configured GPUI extensions");
            editor.set_scroll_handle(scroll.clone());
            editor
        });
        let weak_pane = cx.weak_entity();
        visual_editor.update(cx, |editor, _| {
            editor.set_slash_key_handler(move |editor, key, window, cx| {
                if editor.selection_window_bounds().is_some() {
                    return false;
                }
                let Some(prefix) = editor.caret_prefix().map(str::to_owned) else {
                    return false;
                };
                let Some((token_len, query)) = extract_slash_query(&prefix) else {
                    return false;
                };
                let items = filter_slash_commands(&query);
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
                            cx.notify();
                        });
                        true
                    }
                    "down" => {
                        pane.update(cx, |pane, cx| {
                            pane.slash_index = (pane.slash_index + 1) % count;
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
                .line_number(false)
                .soft_wrap(true)
                .searchable(true)
                .default_value(text.clone())
        });
        let link_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入链接 URL (回车确认)"));
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
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("在文档中查找…"));
        cx.subscribe_in(
            &search_input,
            window,
            |pane: &mut Self, state, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    pane.search_occurrence = 0;
                    let q = state.read(cx).value().trim().to_string();
                    if !q.is_empty() {
                        pane.visual_editor.update(cx, |editor, cx| {
                            editor.select_quote_occurrence(&q, 0, cx);
                        });
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => {
                    let q = state.read(cx).value().trim().to_string();
                    if !q.is_empty() {
                        if *shift {
                            pane.search_occurrence = pane.search_occurrence.saturating_sub(1);
                        } else {
                            pane.search_occurrence += 1;
                        }
                        let next = pane.search_occurrence;
                        let matched = pane.visual_editor.update(cx, |editor, cx| {
                            editor.select_quote_occurrence(&q, next, cx)
                        });
                        if !matched && next > 0 {
                            pane.search_occurrence = 0;
                            pane.visual_editor.update(cx, |editor, cx| {
                                editor.select_quote_occurrence(&q, 0, cx);
                            });
                        }
                        cx.notify();
                    }
                }
                _ => {}
            },
        )
        .detach();
        let note_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入批注内容 (回车保存)…"));
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
                        let _ = storage::save_annotations(&pane.path, &pane.annotations);
                        pane.active_annotation_id = None;
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
                    pane.text = state.read(cx).value().to_string();
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
                    pane.parse_outline();
                    pane.slash_index = 0;
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
                    pane.show_color_picker = false;
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
        let annotations = storage::load_annotations(&path);
        let mut pane = Self {
            export: Rc::new(RefCell::new(None)),
            export_width: px(0.),
            text,
            path,
            headings: Vec::new(),
            source_editor,
            visual_editor,
            source: false,
            outline: false,
            reading,
            focus: cx.focus_handle(),
            scroll,
            settings,
            math,
            formula_editor: None,
            pending_image: None,
            image_error: None,
            slash_index: 0,
            dismissed_slash_prefix: None,
            show_turn_into: false,
            show_color_picker: false,
            active_highlight_color: "yellow".to_owned(),
            link_editing: false,
            link_input,
            search_open: false,
            search_input,
            search_occurrence: 0,
            mouse_pos: None,
            hovered_block_id: None,
            block_menu_for: None,
            annotations,
            active_annotation_id: None,
            show_annotations_drawer: false,
            note_input,
            _source_subscription: source_subscription,
            _visual_subscription: visual_subscription,
            _math_subscription: math_subscription,
        };
        pane.parse_outline();
        pane
    }

    fn parse_outline(&mut self) {
        let arena = Arena::new();
        let root = parse_document(&arena, &self.text, &Options::default());
        let lines: Vec<_> = std::iter::once(0)
            .chain(self.text.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        self.headings = root
            .descendants()
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
                    .min(self.text.len());
                let title = node
                    .descendants()
                    .filter_map(|child| match &child.data.borrow().value {
                        NodeValue::Text(text) => Some(text.to_string()),
                        NodeValue::Code(code) => Some(code.literal.clone()),
                        _ => None,
                    })
                    .collect();
                Some(OutlineHeading {
                    range: start..end,
                    level: heading.level,
                    title,
                })
            })
            .collect();
    }

    pub fn toggle_outline(&mut self, cx: &mut Context<Self>) {
        self.outline = !self.outline;
        cx.notify();
    }

    fn navigate_heading(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            let Some(heading) = self.headings.get(index) else {
                return;
            };
            let offset = heading.range.start.min(self.text.len());
            let line = self.text.as_bytes()[..offset]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
            self.source_editor.update(cx, |editor, cx| {
                editor.set_cursor_position(Position::new(line as u32, 0), window, cx)
            });
        } else {
            let headings = self
                .visual_editor
                .read(cx)
                .model()
                .document()
                .leaves()
                .filter(|block| matches!(block.kind(), BlockKind::Heading { .. }))
                .map(|block| block.id())
                .collect::<Vec<_>>();
            let Some(&block) = headings.get(index) else {
                return;
            };
            let visual_editor = self.visual_editor.clone();
            window.defer(cx, move |window, cx| {
                visual_editor.update(cx, |editor, cx| {
                    let _ = editor
                        .set_selection(BlockSelection::caret(BlockPosition::new(block, 0)), cx);
                    editor.reveal_caret(cx);
                    editor.focus(window, cx);
                });
            });
        }
        cx.notify();
    }

    fn active_slash_state(&self, cx: &App) -> Option<(usize, String, Vec<&'static SlashCommand>)> {
        if self.source {
            return None;
        }
        let editor = self.visual_editor.read(cx);
        if editor.selection_window_bounds().is_some() {
            return None;
        }
        let prefix = editor.caret_prefix()?;
        if self.dismissed_slash_prefix.as_deref() == Some(prefix) {
            return None;
        }
        let (token_len, query) = extract_slash_query(prefix)?;
        let items = filter_slash_commands(&query);
        if items.is_empty() {
            return None;
        }
        Some((token_len, query, items))
    }

    fn handle_slash_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((token_len, _, items)) = self.active_slash_state(cx) else {
            return false;
        };
        let count = items.len();
        match key {
            "up" => {
                self.slash_index = (self.slash_index + count - 1) % count;
                cx.notify();
                true
            }
            "down" => {
                self.slash_index = (self.slash_index + 1) % count;
                cx.notify();
                true
            }
            "enter" | "tab" => {
                let cmd = items[self.slash_index.min(count - 1)];
                self.execute_slash_command(cmd.id, token_len, window, cx);
                true
            }
            _ => false,
        }
    }

    fn execute_slash_command(
        &mut self,
        cmd_id: &str,
        token_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub fn sync(
        &mut self,
        text: &str,
        settings: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings != *settings {
            self.settings = settings.clone();
            self.math
                .update(cx, |renderer, cx| renderer.set_appearance(settings, cx));
            cx.notify();
        }
        if self.text == text {
            return;
        }
        self.text = text.to_owned();
        self.parse_outline();
        self.visual_editor.update(cx, |editor, cx| {
            editor
                .set_source(text, cx)
                .expect("external Markdown parses with the configured GPUI extensions");
        });
        self.source_editor.update(cx, |editor, cx| {
            editor.set_value(text.to_owned(), window, cx)
        });
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

    pub fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.source = !self.source;
        if self.source {
            let text = self.text.clone();
            let source_editor = self.source_editor.clone();
            window.defer(cx, move |window, cx| {
                source_editor.update(cx, |editor, cx| {
                    editor.set_value(text, window, cx);
                    editor.set_highlighter("markdown", cx);
                    editor.focus(window, cx);
                });
            });
        } else {
            self.visual_editor.update(cx, |editor, cx| {
                editor
                    .set_source(&self.text, cx)
                    .expect("source mode Markdown parses with the configured GPUI extensions");
            });
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

    fn apply_highlight_color(&mut self, color_name: &str, cx: &mut Context<Self>) {
        self.active_highlight_color = color_name.to_owned();
        let hsla = hsla_for_color_name(color_name);
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().with_highlight(hsla)),
                cx,
            );
        });
        self.show_color_picker = false;
        cx.notify();
    }

    fn clear_highlight(&mut self, cx: &mut Context<Self>) {
        self.visual_editor.update(cx, |editor, cx| {
            let _ = editor.run(
                EditorCommand::ApplyMark(MarkChange::new().clear(MarkKind::Highlight)),
                cx,
            );
        });
        self.show_color_picker = false;
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
            let _ = storage::save_annotations(&self.path, &self.annotations);
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
            .set_title("插入本地图片")
            .add_filter("图片", &["png", "jpg", "jpeg", "webp", "gif", "svg"]);
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
        let insertion = format!("![图片](<{filename}>)");
        self.visual_editor.update(cx, |editor, cx| {
            if let Ok(mut range) = editor.capture_selection() {
                let _ = editor.replace_captured(&mut range, &insertion, cx);
            }
        });
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
            .min((pane_size.width - px(520.)).max(px(12.)));
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
                    s.bg(rgb(0xe0ecff)).text_color(rgb(0x1d4ed8))
                })
                .when(!active, |s| {
                    s.text_color(rgb(0x1f2328)).hover(|h| h.bg(rgb(0xf2f3f5)))
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
                        pane.show_color_picker = false;
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
            // 4. 'A' Highlight / Fluorescent Picker
            .child(
                div()
                    .id("ft-highlight-a")
                    .w(px(28.))
                    .h(px(26.))
                    .rounded(px(5.))
                    .bg(rgb(0xe8f3ff))
                    .hover(|s| s.bg(rgb(0xd0e6ff)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(rgb(0x165dff))
                            .child("A"),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(3.))
                            .rounded(px(1.5))
                            .bg(rgb(active_hl_rgb)),
                    )
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.show_color_picker = !pane.show_color_picker;
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
                    let selected = pane
                        .visual_editor
                        .read(cx)
                        .selected_plain_text()
                        .unwrap_or_default();
                    pane.search_open = true;
                    pane.search_occurrence = 0;
                    let search_input = pane.search_input.clone();
                    window.defer(cx, move |window, cx| {
                        search_input.update(cx, |input, cx| {
                            input.set_value(selected, window, cx);
                            input.focus(window, cx);
                        });
                    });
                    cx.notify();
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
                ("text", "正文", IconName::Type),
                ("h1", "一级标题 H1", IconName::Heading1),
                ("h2", "二级标题 H2", IconName::Heading2),
                ("h3", "三级标题 H3", IconName::Heading3),
                ("h4", "四级标题 H4", IconName::Heading4),
                ("bullet", "无序列表", IconName::List),
                ("ordered", "有序列表", IconName::ListOrdered),
                ("todo", "任务列表", IconName::ListTodo),
                ("quote", "引用块", IconName::TextQuote),
                ("code", "代码块", IconName::CodeXml),
            ];
            let rows = turn_items.iter().map(|(id, label, ic)| {
                let cmd_id = *id;
                div()
                    .id(SharedString::from(format!("turn-into-{cmd_id}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2p5()
                    .py_1()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_xs()
                    .hover(|s| s.bg(rgb(0xf2f3f5)))
                    .child(crate::ui::icon(*ic))
                    .child(*label)
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.show_turn_into = false;
                        pane.execute_slash_command(cmd_id, 0, window, cx);
                    }))
            });
            stack = stack.child(
                div()
                    .w(px(160.))
                    .p_1p5()
                    .rounded(px(8.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(rows),
            );
        }

        if self.show_color_picker {
            let swatches = HIGHLIGHT_COLORS.iter().map(|(name, _, hex)| {
                let cname = (*name).to_owned();
                let selected = self.active_highlight_color == *name;
                div()
                    .id(SharedString::from(format!("md-hl-swatch-{name}")))
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(11.))
                    .bg(rgb(*hex))
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
            stack = stack.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(px(10.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().text_xs().text_color(rgb(0x787774)).child("荧光笔"))
                    .children(swatches)
                    .child(
                        div()
                            .id("md-hl-clear")
                            .px_2()
                            .py_0p5()
                            .rounded(px(4.))
                            .text_xs()
                            .text_color(rgb(0x57606a))
                            .hover(|s| s.bg(rgb(0xf2f3f5)))
                            .cursor_pointer()
                            .child("清除")
                            .on_click(cx.listener(|pane, _, window, cx| {
                                pane.clear_highlight(cx);
                                pane.defer_focus_visual(window, cx);
                            })),
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
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&self.link_input).flex_1().min_w_0())
                    .child(
                        crate::ui::button("apply-md-link", "确定").on_click(cx.listener(
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

        let rel_x = (caret.origin.x - pane_origin.x)
            .max(px(16.))
            .min((pane_size.width - px(250.)).max(px(16.)));
        let below_y = caret.bottom() - pane_origin.y + px(6.);
        let menu_h = px((items.len().min(9) as f32) * 32. + 36.);
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
                .h(px(30.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |s| {
                    s.bg(rgb(0xe8f3ff)).text_color(rgb(0x165dff))
                })
                .when(!selected, |s| {
                    s.text_color(rgb(0x1f2328)).hover(|h| h.bg(rgb(0xf2f3f5)))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(crate::ui::icon(cmd.icon))
                        .child(div().text_xs().font_medium().child(cmd.title)),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family("Consolas")
                        .text_color(rgb(0x86909c))
                        .child(cmd.hint),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |pane, _, window, cx| {
                    pane.execute_slash_command(cmd_id, token_len, window, cx);
                }))
        });

        let header_label = if query.is_empty() {
            "基础与排版块 (↑↓选择 · Enter确认)".to_owned()
        } else {
            format!("过滤: /{query}")
        };

        Some(
            deferred(
                div()
                    .id("md-slash-menu")
                    .absolute()
                    .left(rel_x)
                    .top(rel_y)
                    .w(px(236.))
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .p_1p5()
                    .rounded(px(10.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(0x86909c))
                            .child(header_label),
                    )
                    .children(rows),
            )
            .into_any_element(),
        )
    }

    fn render_left_block_handle(
        &self,
        pane_origin: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.source {
            return None;
        }
        let (top_id, kind, bounds) = self
            .visual_editor
            .read(cx)
            .top_block_at_or_active(self.mouse_pos)?;
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
        let rel_x = (bounds.origin.x - pane_origin.x - px(46.)).max(px(4.));
        let rel_y = bounds.origin.y - pane_origin.y + px(1.);

        let handle = div()
            .id("md-block-gutter-handle")
            .absolute()
            .left(rel_x)
            .top(rel_y)
            .h(px(24.))
            .px_1()
            .rounded(px(5.))
            .flex()
            .items_center()
            .gap_0p5()
            .cursor_pointer()
            .text_color(rgb(0x2b6cb0))
            .hover(|s| s.bg(rgb(0xeef4fb)))
            .child(
                div()
                    .font_family("Georgia")
                    .font_semibold()
                    .text_xs()
                    .child(badge),
            )
            .child(crate::ui::icon(IconName::GripVertical))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |pane, ev: &ClickEvent, _, cx| {
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
                    "转为正文",
                    EditorCommand::SetBlockKind(BlockKind::Paragraph),
                    IconName::Type,
                ),
                (
                    "转为一级标题 H1",
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 1 }),
                    IconName::Heading1,
                ),
                (
                    "转为二级标题 H2",
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
                    IconName::Heading2,
                ),
                (
                    "转为三级标题 H3",
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 3 }),
                    IconName::Heading3,
                ),
                (
                    "转为无序列表",
                    EditorCommand::ToggleList { ordered: false },
                    IconName::List,
                ),
                (
                    "转为任务列表",
                    EditorCommand::ToggleTask,
                    IconName::ListTodo,
                ),
                (
                    "转为引用块",
                    EditorCommand::ToggleBlockquote,
                    IconName::TextQuote,
                ),
                (
                    "转为代码块",
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
                    .px_2p5()
                    .py_1()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_xs()
                    .hover(|s| s.bg(rgb(0xf2f3f5)))
                    .child(crate::ui::icon(*ic))
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
                .w(px(168.))
                .p_1p5()
                .rounded(px(8.))
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0xe2e4e8))
                .flex()
                .flex_col()
                .gap_0p5()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows)
                .child(div().h(px(1.)).my_1().bg(rgb(0xebedf0)))
                .child(
                    div()
                        .id("block-menu-delete")
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2p5()
                        .py_1()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0xd93025))
                        .hover(|s| s.bg(rgb(0xfff1f0)))
                        .child(crate::ui::icon(IconName::Trash))
                        .child("删除该块")
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

        let rel_x = (anchor.origin.x - pane_origin.x)
            .max(px(16.))
            .min((pane_size.width - px(310.)).max(px(16.)));
        let rel_y = (anchor.bottom() - pane_origin.y + px(8.)).min(pane_size.height - px(140.));

        let swatches = HIGHLIGHT_COLORS.iter().map(|(name, _, hex)| {
            let cname = (*name).to_owned();
            let selected = item.color == *name;
            div()
                .id(SharedString::from(format!("ann-color-{name}")))
                .w(px(16.))
                .h(px(16.))
                .rounded(px(8.))
                .bg(rgb(*hex))
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
                    let _ = storage::save_annotations(&pane.path, &pane.annotations);
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
                    .w(px(296.))
                    .p_3()
                    .rounded(px(10.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe2e4e8))
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
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                pane.annotations.retain(|a| a.id != id);
                                                let _ = storage::save_annotations(
                                                    &pane.path,
                                                    &pane.annotations,
                                                );
                                                pane.active_annotation_id = None;
                                                pane.clear_highlight(cx);
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
                            .text_xs()
                            .text_color(rgb(0x787774))
                            .text_ellipsis()
                            .child(format!("“{}”", item.quote)),
                    )
                    .child(Input::new(&self.note_input))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(
                                crate::ui::button("save-md-annotation", "保存批注").on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        let comment =
                                            pane.note_input.read(cx).value().trim().to_string();
                                        if let Some(a) =
                                            pane.annotations.iter_mut().find(|a| a.id == id)
                                        {
                                            a.comment = comment;
                                        }
                                        let _ = storage::save_annotations(
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
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xefefed)))
        .child(crate::ui::label(label.into()))
}

impl Render for MarkdownPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_bounds = self.scroll.bounds();
        let pane_origin = pane_bounds.origin;
        let pane_size = pane_bounds.size;

        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .pr(px(38.))
            .border_b_1()
            .border_color(rgb(0xe8e7e3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .child(crate::ui::icon(IconName::FileText))
                    .child(
                        div().min_w_0().text_ellipsis().child(
                            self.path
                                .file_name()
                                .expect("Markdown has filename")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .child(
                        crate::ui::control("md-outline", IconName::ListTree)
                            .on_click(cx.listener(|pane, _, _, cx| pane.toggle_outline(cx))),
                    )
                    .child(
                        action(
                            "md-font-size",
                            format!("{}px", self.settings.markdown_font_size),
                        )
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.settings.markdown_font_size = 14;
                            cx.emit(MarkdownEvent::Zoom(14));
                            cx.notify();
                        })),
                    )
                    .child(action("md-source", "源码").on_click(
                        cx.listener(|pane, _, window, cx| pane.toggle_source(window, cx)),
                    )),
            );

        let mut content = div().relative().flex().flex_1().min_h_0();
        if self.outline {
            let headings = self.headings.iter().enumerate().map(|(index, heading)| {
                div()
                    .id(("md-heading", index))
                    .pl(px(12. + (heading.level.saturating_sub(1) as f32) * 12.))
                    .pr_2()
                    .py_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xefefed)))
                    .child(div().text_ellipsis().child(heading.title.clone()))
                    .on_click(
                        cx.listener(move |pane, _, window, cx| {
                            pane.navigate_heading(index, window, cx)
                        }),
                    )
            });
            content = content.child(
                div()
                    .id("markdown-outline")
                    .w(px(210.))
                    .flex_shrink_0()
                    .h_full()
                    .overflow_y_scroll()
                    .bg(rgb(0xfafaf9))
                    .border_r_1()
                    .border_color(rgb(0xe8e7e3))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(crate::ui::label("标题导航".into()))
                    .children(headings),
            );
        }

        if self.source {
            content = content.child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .child(
                        Editor::new(&self.source_editor)
                            .appearance(false)
                            .bordered(false)
                            .size_full()
                            .font_family(if self.settings.editor_font.is_empty() {
                                SharedString::from("Consolas")
                            } else {
                                self.settings.editor_font.clone().into()
                            })
                            .text_size(px(self.settings.editor_font_size as f32)),
                    )
                    .when(!self.reading, |area| {
                        area.child(
                            div()
                                .absolute()
                                .top_2()
                                .right_4()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    crate::ui::control("md-float-outline-src", IconName::ListTree)
                                        .on_click(cx.listener(|pane, _, _, cx| {
                                            pane.toggle_outline(cx)
                                        })),
                                )
                                .child(
                                    crate::ui::control("md-float-source-src", IconName::CodeXml)
                                        .on_click(cx.listener(|pane, _, window, cx| {
                                            pane.toggle_source(window, cx)
                                        })),
                                ),
                        )
                    }),
            );
        } else {
            let floating_toolbar =
                self.render_floating_selection_toolbar(pane_origin, pane_size, cx);
            let slash_menu = self.render_slash_menu(pane_origin, pane_size, cx);
            let block_handle = self.render_left_block_handle(pane_origin, cx);
            let annotation_popover =
                self.render_active_annotation_popover(pane_origin, pane_size, cx);

            content = content.child(
                div()
                    .id("markdown-content")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .justify_center()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .text_size(px(self.settings.markdown_font_size as f32))
                    .font_family(if self.settings.interface_font.is_empty() {
                        SharedString::from("Segoe UI")
                    } else {
                        self.settings.interface_font.clone().into()
                    })
                    .on_mouse_move(cx.listener(|pane, ev: &MouseMoveEvent, _, cx| {
                        pane.mouse_pos = Some(ev.position);
                        let next_id = pane
                            .visual_editor
                            .read(cx)
                            .top_block_at_or_active(Some(ev.position))
                            .map(|(id, _, _)| id);
                        if next_id != pane.hovered_block_id {
                            pane.hovered_block_id = next_id;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .when(self.settings.line_width_limited, |content| {
                                content.max_w(px(850.))
                            })
                            .pl(px(54.))
                            .pr_6()
                            .py_6()
                            .child(BlockEditor::new(&self.visual_editor)),
                    )
                    .children(block_handle)
                    .children(floating_toolbar)
                    .children(slash_menu)
                    .children(annotation_popover)
                    .when(!self.reading, |area| {
                        area.child(
                            div()
                                .id("md-floating-controls")
                                .absolute()
                                .top_2p5()
                                .right_4()
                                .flex()
                                .items_center()
                                .gap_1()
                                .when(!self.annotations.is_empty(), |row| {
                                    let count = self.annotations.len();
                                    row.child(
                                        div()
                                            .id("md-annotations-pill")
                                            .px_2p5()
                                            .py_1()
                                            .rounded_full()
                                            .bg(rgb(0xfef9c3))
                                            .border_1()
                                            .border_color(rgb(0xfde047))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(0x854d0e))
                                            .child(crate::ui::icon(IconName::MessageSquareText))
                                            .child(format!("{count}"))
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.show_annotations_drawer =
                                                    !pane.show_annotations_drawer;
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    crate::ui::control("md-float-outline", IconName::ListTree)
                                        .on_click(cx.listener(|pane, _, _, cx| {
                                            pane.toggle_outline(cx)
                                        })),
                                )
                                .child(
                                    crate::ui::control("md-float-source", IconName::CodeXml)
                                        .on_click(cx.listener(|pane, _, window, cx| {
                                            pane.toggle_source(window, cx)
                                        })),
                                ),
                        )
                    })
                    .when(self.reading && !self.annotations.is_empty(), |area| {
                        let count = self.annotations.len();
                        area.child(
                            div()
                                .id("md-annotations-pill-reading")
                                .absolute()
                                .top_3()
                                .right_4()
                                .px_2p5()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(0xfef9c3))
                                .border_1()
                                .border_color(rgb(0xfde047))
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .text_xs()
                                .text_color(rgb(0x854d0e))
                                .child(crate::ui::icon(IconName::MessageSquareText))
                                .child(format!("{count}"))
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.show_annotations_drawer = !pane.show_annotations_drawer;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(self.search_open, |area| {
                        area.child(
                            div()
                                .id("md-search-bar")
                                .absolute()
                                .top_3()
                                .right(px(140.))
                                .w(px(260.))
                                .px_2p5()
                                .py_1()
                                .rounded(px(8.))
                                .bg(rgb(0xffffff))
                                .border_1()
                                .border_color(rgb(0xe2e4e8))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(Input::new(&self.search_input).flex_1().min_w_0())
                                .child(
                                    crate::ui::control("close-md-search", IconName::Close)
                                        .on_click(cx.listener(|pane, _, _, cx| {
                                            pane.search_open = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll)
                                .mode(ScrollbarMode::Always)
                                .styles(|styles| {
                                    styles
                                        .thumb(|style| {
                                            style
                                                .bg(rgb(0x999996))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                        .thumb_hover(|style| {
                                            style
                                                .bg(rgb(0x777774))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                        .thumb_active(|style| {
                                            style
                                                .bg(rgb(0x5d5d5a))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                }),
                        ),
                    ),
            );

            if self.show_annotations_drawer && !self.annotations.is_empty() {
                let cards = self.annotations.iter().map(|item| {
                    let id = item.id;
                    let quote = item.quote.clone();
                    let occ = item.occurrence;
                    let comment = item.comment.clone();
                    let badge_rgb = HIGHLIGHT_COLORS
                        .iter()
                        .find(|(n, _, _)| *n == item.color)
                        .map(|(_, _, hex)| *hex)
                        .unwrap_or(0xfacc15);
                    div()
                        .id(SharedString::from(format!("md-ann-card-{id}")))
                        .p_2p5()
                        .rounded(px(8.))
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0xe8e7e3))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .cursor_pointer()
                        .hover(|s| s.border_color(rgb(0x94a3b8)))
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
                                        .min_w_0()
                                        .child(
                                            div()
                                                .w(px(8.))
                                                .h(px(8.))
                                                .rounded(px(4.))
                                                .flex_shrink_0()
                                                .bg(rgb(badge_rgb)),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_ellipsis()
                                                .child(quote.clone()),
                                        ),
                                )
                                .child(
                                    crate::ui::control(
                                        SharedString::from(format!("del-ann-{id}")),
                                        IconName::Trash,
                                    )
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        cx.stop_propagation();
                                        pane.annotations.retain(|a| a.id != id);
                                        let _ = storage::save_annotations(
                                            &pane.path,
                                            &pane.annotations,
                                        );
                                        cx.notify();
                                    })),
                                ),
                        )
                        .when(!comment.is_empty(), |card| {
                            card.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x57606a))
                                    .child(comment.clone()),
                            )
                        })
                        .on_click(cx.listener(move |pane, _, window, cx| {
                            let visual_editor = pane.visual_editor.clone();
                            let note_input = pane.note_input.clone();
                            let quote_cl = quote.clone();
                            let comment_cl = comment.clone();
                            pane.active_annotation_id = Some(id);
                            window.defer(cx, move |window, cx| {
                                visual_editor.update(cx, |editor, cx| {
                                    editor.select_quote_occurrence(&quote_cl, occ, cx);
                                });
                                note_input.update(cx, |input, cx| {
                                    input.set_value(comment_cl, window, cx);
                                    input.focus(window, cx);
                                });
                            });
                            cx.notify();
                        }))
                });
                content = content.child(
                    div()
                        .id("md-annotations-drawer")
                        .w(px(240.))
                        .flex_shrink_0()
                        .h_full()
                        .overflow_y_scroll()
                        .bg(rgb(0xfafaf9))
                        .border_l_1()
                        .border_color(rgb(0xe8e7e3))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_xs().font_semibold().child("文档批注"))
                                .child(
                                    crate::ui::control("close-ann-drawer", IconName::Close)
                                        .on_click(cx.listener(|pane, _, _, cx| {
                                            pane.show_annotations_drawer = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .children(cards),
                );
            }
        }

        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .when(self.reading, |pane| pane.child(toolbar))
            .child(content)
            .on_key_down(cx.listener(|pane, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    if let Some(prefix) = pane.visual_editor.read(cx).caret_prefix()
                        && extract_slash_query(prefix).is_some()
                    {
                        pane.dismissed_slash_prefix = Some(prefix.to_owned());
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if pane.show_turn_into
                        || pane.show_color_picker
                        || pane.link_editing
                        || pane.block_menu_for.is_some()
                        || pane.active_annotation_id.is_some()
                        || pane.search_open
                    {
                        pane.show_turn_into = false;
                        pane.show_color_picker = false;
                        pane.link_editing = false;
                        pane.block_menu_for = None;
                        pane.active_annotation_id = None;
                        pane.search_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, _, _, cx| {
                    pane.show_turn_into = false;
                    pane.show_color_picker = false;
                    pane.block_menu_for = None;
                    cx.emit(MarkdownEvent::Focus);
                }),
            )
            .on_action(
                cx.listener(|pane, _: &ToggleSource, window, cx| pane.toggle_source(window, cx)),
            )
            .on_action(cx.listener(|pane, _: &Bold, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Bold), cx)
            }))
            .on_action(cx.listener(|pane, _: &Italic, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Italic), cx)
            }))
            .on_action(cx.listener(|pane, _: &Underline, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Underline), cx)
            }))
            .on_action(cx.listener(|pane, _: &Strikethrough, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Strikethrough), cx)
            }))
            .on_action(cx.listener(|pane, _: &InlineCode, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Code), cx)
            }))
            .on_action(cx.listener(|pane, _: &Highlight, _, cx| {
                let c = pane.active_highlight_color.clone();
                pane.apply_highlight_color(&c, cx);
            }))
            .on_action(cx.listener(|pane, _: &LinkAction, _, cx| {
                pane.visual_editor
                    .update(cx, |editor, cx| editor.request_link_edit(cx));
            }))
            .on_action(cx.listener(|pane, _: &HeadingAction, _, cx| {
                pane.run_command(
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
                    cx,
                )
            }))
            .on_action(cx.listener(|pane, _: &BulletList, _, cx| {
                pane.run_command(EditorCommand::ToggleList { ordered: false }, cx)
            }))
            .on_action(cx.listener(|pane, _: &TaskList, _, cx| {
                pane.run_command(EditorCommand::ToggleTask, cx)
            }))
            .on_action(
                cx.listener(|pane, _: &InsertImage, window, cx| pane.insert_image(window, cx)),
            )
            .on_action(cx.listener(|pane, _: &InsertTable, _, cx| {
                pane.run_command(EditorCommand::InsertTable { rows: 3, cols: 3 }, cx)
            }))
            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                if event.modifiers.control {
                    let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                        1
                    } else {
                        -1
                    };
                    pane.settings.markdown_font_size =
                        (pane.settings.markdown_font_size as i16 + delta).clamp(11, 26) as u8;
                    pane.math.update(cx, |renderer, cx| {
                        renderer.set_appearance(&pane.settings, cx)
                    });
                    cx.stop_propagation();
                    cx.notify();
                    cx.emit(MarkdownEvent::Zoom(pane.settings.markdown_font_size));
                }
            }))
            .context_menu(|menu, _, _| {
                menu.menu("加粗", Box::new(Bold))
                    .menu("斜体", Box::new(Italic))
                    .menu("下划线", Box::new(Underline))
                    .menu("删除线", Box::new(Strikethrough))
                    .menu("荧光高亮", Box::new(Highlight))
                    .separator()
                    .menu("标题", Box::new(HeadingAction))
                    .menu("无序列表", Box::new(BulletList))
                    .menu("任务列表", Box::new(TaskList))
                    .separator()
                    .menu("插入图片…", Box::new(InsertImage))
                    .menu("插入表格", Box::new(InsertTable))
            })
            .when_some(self.formula_editor.clone(), |pane, (editor, display)| {
                pane.child(crate::ui::modal_overlay(
                    "编辑公式",
                    "输入 LaTeX 正文，无需添加分隔符。".to_owned(),
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div().h(px(180.)).w_full().child(
                                Editor::new(&editor)
                                    .appearance(false)
                                    .bordered(true)
                                    .size_full()
                                    .font_family("Consolas")
                                    .text_size(px(18.)),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(crate::ui::button("cancel-formula", "取消").on_click(
                                    cx.listener(|pane, _, window, cx| {
                                        pane.formula_editor = None;
                                        pane.defer_focus_visual(window, cx);
                                        cx.notify();
                                    }),
                                ))
                                .child(crate::ui::button("apply-formula", "应用").on_click(
                                    cx.listener(move |pane, _, window, cx| {
                                        let latex = editor.read(cx).value().to_string();
                                        let source = if display {
                                            format!("$$\n{latex}\n$$")
                                        } else {
                                            format!("${latex}$")
                                        };
                                        pane.visual_editor.update(cx, |editor, cx| {
                                            editor.replace_selected_markdown(&source, cx);
                                        });
                                        pane.defer_focus_visual(window, cx);
                                        pane.formula_editor = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ))
            })
            .when_some(self.pending_image.clone(), |pane, (_, destination)| {
                pane.child(crate::ui::modal_overlay(
                    "替换同名图片？",
                    destination.to_string_lossy().into_owned(),
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::ui::button("cancel-image-overwrite", "取消").on_click(
                                cx.listener(|pane, _, _, cx| {
                                    pane.pending_image = None;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            crate::ui::button("confirm-image-overwrite", "替换").on_click(
                                cx.listener(|pane, _, _, cx| pane.confirm_image_import(cx)),
                            ),
                        ),
                ))
            })
            .when_some(self.image_error.clone(), |pane, error| {
                pane.child(crate::ui::modal_overlay(
                    "图片导入失败",
                    error,
                    div().flex().justify_end().child(
                        crate::ui::button("dismiss-image-error", "确定").on_click(cx.listener(
                            |pane, _, _, cx| {
                                pane.image_error = None;
                                cx.notify();
                            },
                        )),
                    ),
                ))
            })
    }
}
