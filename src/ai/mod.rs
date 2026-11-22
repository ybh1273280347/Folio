//! Native GPUI conversation view; the manager owns project tabs and the shared SDK host.
mod confirmation;
mod diff;
mod rpc;
mod manager;
mod quote;
mod session;
mod render;
use session::{Session, SessionChanges};
pub use manager::AiPane;
pub(crate) use rpc::project_key;
pub(crate) use rpc::{WorkspaceToolAction, WorkspaceToolRequest};
pub use quote::SelectionQuote;
use quote::{append_quotes, split_injected_prompt};

use crate::math::{MathDisplaySize, MathReady, MathRenderer};
use base64::Engine as _;
use gpui_kit::component::Sizable;
use crate::mermaid::{MermaidReady, MermaidRenderer};
use crate::model::{AppSettings, ToolExecutionView};
use crate::terminal::{DraggedFolder, DraggedPaths};
use crate::theme::{Palette, color as theme_color, palette as active_palette};
use confirmation::{ApprovalState, ConfirmationContent, TagApproval};
use diff::{DiffLine, EditPatch, FileDiff};
use gpui::{
    ClipboardEntry, EntityInputHandler, ExternalPaths, ImageFormat, MouseButton, ObjectFit,
    ScrollEvent, ScrollHandle, Subscription,
};
use gpui_kit::{
    assets::IconName,
    base::{
        Dialog, DialogPopup, Scrollbar, ScrollbarMode, StyledExt, TextSelection,
        text::{MarkdownExtensions, TextView, TextViewStyle},
    },
    component::{
        button::{Button, ButtonCustomVariant, ButtonVariants},
        diff::{Diff, DiffChangeIndicator, DiffHunkSeparator, DiffState},
        input::{
            Enter, Escape, IndentInline, InlineToken, Input, InputEvent, InputState, InputToken,
            MoveDown, MoveUp, Textarea, TextareaState,
        },
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use rpc::{RpcClient, RpcImage, RpcInbound};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

pub enum AiPaneEvent {
    WorkspaceTool(WorkspaceToolRequest),
    UndoTags(Vec<crate::tags::agent::TagConfirmation>),
    OpenFile(PathBuf),
    BackToHub,
    ToggleMaximize,
}
impl EventEmitter<AiPaneEvent> for AiPane {}
impl EventEmitter<AiPaneEvent> for ChatPane {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedFile {
    pub display: String,
    pub path: PathBuf,
}

#[derive(Clone)]
struct AnswerQuotePopover {
    quote: SelectionQuote,
    position: Point<Pixels>,
    scroll_offset: Point<Pixels>,
}

#[derive(Clone, Debug)]
struct ModelInfo {
    provider: String,
    id: String,
    name: String,
    reasoning: bool,
}

#[derive(Clone, Debug)]
struct ContextUsage {
    tokens: Option<u64>,
    context_window: u64,
    percent: Option<u64>,
}

#[derive(Clone, Debug)]
struct PiCommandInfo {
    name: String,
    description: String,
    source: String,
}

#[derive(Clone)]
struct PastedImage {
    id: u64,
    image: Arc<gpui::Image>,
    mime_type: String,
    data_b64: String,
    label: String,
}

#[derive(Clone, Debug)]
struct ToolBlock {
    id: String,
    name: String,
    args: Value,
    summary: String,
    output: String,
    is_error: bool,
    done: bool,
    duration_ms: Option<u64>,
    diffs: Vec<FileDiff>,
    approval: Option<TagApproval>,
}

#[derive(Clone, Debug)]
enum AssistantPart {
    Thinking {
        text: String,
        duration_ms: Option<u64>,
        streaming: bool,
    },
    Text(String),
    Tool(ToolBlock),
}

#[derive(Clone)]
enum ChatMessage {
    User {
        entry_id: Option<String>,
        display_text: String,
        // Pi stores user-message timestamps as Unix milliseconds.
        timestamp_ms: Option<i64>,
        attached_file: Option<String>,
        mounted_skills: Vec<String>,
        quotes: Vec<SelectionQuote>,
        images: Vec<Arc<gpui::Image>>,
    },
    Assistant {
        parts: Vec<AssistantPart>,
        error: Option<String>,
        streaming: bool,
        duration_ms: Option<u64>,
    },
    Notice {
        text: String,
        is_error: bool,
    },
}

/// Each turn starts at the user message's immediate transcript scroll-child index.
struct TurnAnchor {
    item: usize,
    timestamp_ms: Option<i64>,
    preview: String,
}

#[derive(Clone, Debug)]
enum PendingUiKind {
    Confirm(ConfirmationContent),
    Select {
        options: Vec<String>,
    },
    TextInput {
        placeholder: String,
        multiline: bool,
    },
}

#[derive(Clone)]
struct PendingExtensionUi {
    id: String,
    title: String,
    kind: PendingUiKind,
}

#[derive(Clone, Debug)]
enum SlashAction {
    ToggleActiveContext,
    Compact,
    MountSkill(String),
}

#[derive(Clone, Debug)]
struct SlashItem {
    id: String,
    title: String,
    subtitle: String,
    icon: IconName,
    action: SlashAction,
}

#[derive(Clone, Copy)]
struct AiTypography {
    body: Pixels,
    sm: Pixels,
    xs: Pixels,
}

impl AiTypography {
    fn from_settings(settings: &AppSettings) -> Self {
        let base = settings.ai_font_size as f32;
        Self {
            body: px(base),
            sm: px((base - 1.5).max(10.5)),
            xs: px((base - 2.5).max(9.5)),
        }
    }
}

const AI_COMPOSER_MAX_WIDTH: Pixels = px(720.);
const AI_WELCOME_ICON_SIZE: Pixels = px(64.);
const AI_WELCOME_ICON_TITLE_GAP: Pixels = px(18.);
const AI_COPY_BUTTON_SIZE: Pixels = px(20.);
const AI_COPY_ICON_SIZE: Pixels = px(12.);
const AI_COPY_BUTTON_RADIUS: Pixels = px(5.);

enum ChatOverlay {
    Image(Arc<gpui::Image>),
    Rewind { entry_id: String, edit: bool },
}

struct ChatPane {
    session: Rc<RefCell<Session>>,
    project_name: Option<String>,
    settings: AppSettings,
    active_file: Option<AttachedFile>,
    attach_active_file: bool,
    quoted_selections: Vec<SelectionQuote>,
    quote_popover: Option<AnswerQuotePopover>,
    quote_drag: Option<(usize, usize, Point<Pixels>)>,
    quote_bounds: Rc<Cell<Bounds<Pixels>>>,
    pasted_images: Vec<PastedImage>,
    overlay: Option<(ChatOverlay, Option<FocusHandle>)>,
    overlay_focus: FocusHandle,
    next_image_id: u64,
    pending_ext_prefill: Option<(String, bool)>,
    pending_composer_text: Option<String>,
    ext_input: Entity<InputState>,
    ext_textarea: Entity<TextareaState>,
    toast: Option<(String, bool)>,
    toast_task: Task<()>,
    toast_anim_gen: u64,
    turn_navigation_open: bool,
    turn_navigation_scroll: ScrollHandle,
    model_popover_open: bool,
    model_popover_anim_gen: u64,
    model_search_input: Entity<InputState>,
    favorite_models: HashSet<String>,
    slash_anim_gen: u64,
    slash_was_open: bool,
    slash_index: usize,
    slash_scroll: ScrollHandle,
    last_input_text: String,
    dismissed_slash_text: Option<String>,
    collapsed_tool_groups: HashSet<(usize, usize)>,
    expanded_thinking: HashSet<(usize, usize)>,
    expanded_diffs: HashSet<(usize, usize)>,
    expanded_tag_receipts: HashSet<String>,
    expanded_approval_groups: HashSet<String>,
    toggled_task_processes: HashSet<usize>,
    viewed_diff_files: HashSet<String>,
    input: Entity<TextareaState>,
    _math: Entity<MathRenderer>,
    _mermaid: Entity<MermaidRenderer>,
    md_extensions: MarkdownExtensions,
    scroll: ScrollHandle,
    stick_to_bottom: bool,
    _subscriptions: Vec<Subscription>,
}

impl ChatPane {
    pub fn new(
        session: Rc<RefCell<Session>>,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mermaid = cx.new(MermaidRenderer::new);
        let md_extensions = crate::markdown::extensions::markdown_extensions(
            &math, MathDisplaySize::Ai, cx,
        )
            .plugin(mermaid.read(cx).plugin(mermaid.downgrade()));
        let sub_math = cx.subscribe(&math, |_, _, event: &MathReady, cx| {
            if matches!(event, MathReady::Ready) {
                cx.notify();
            }
        });
        let sub_mermaid = cx.subscribe(&mermaid, |_, _, _: &MermaidReady, cx| {
            cx.notify();
        });
        let quote_view = cx.entity().downgrade();
        let quote_window = window.window_handle();
        let sub_popover_escape = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() == quote_window
                && event.keystroke.key == "escape"
                && event.keystroke.modifiers.number_of_modifiers() == 0
            {
                let _ = quote_view.update(cx, |pane, cx| {
                    if pane.turn_navigation_open {
                        pane.turn_navigation_open = false;
                        cx.notify();
                        cx.stop_propagation();
                    } else if pane.quote_popover.is_some() {
                        pane.dismiss_quote(cx);
                        cx.stop_propagation();
                    }
                });
            }
        });
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 7)
                .submit_on_enter(true)
        });
        let model_search_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::ai::MODEL_SEARCH_PLACEHOLDER.get()));
        let ext_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::ai::EXT_INPUT_PLACEHOLDER.get()));
        let ext_textarea = cx.new(|cx| TextareaState::new(window, cx).placeholder(crate::i18n::ai::EXT_TEXTAREA_PLACEHOLDER.get()));

        let sub_input = cx.subscribe_in(
            &input,
            window,
            |pane, state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let val = state.read(cx).value().to_string();
                    if val != pane.last_input_text {
                        pane.last_input_text = val.clone();
                        pane.slash_index = 0;
                    }
                    if pane.dismissed_slash_text.as_deref() != Some(val.as_str()) {
                        pane.dismissed_slash_text = None;
                    }
                    let is_open = pane
                        .slash_query(cx)
                        .is_some_and(|q| !pane.filtered_slash_items(&q).is_empty());
                    if is_open && !pane.slash_was_open {
                        pane.slash_anim_gen = pane.slash_anim_gen.wrapping_add(1);
                    }
                    pane.slash_was_open = is_open;
                    cx.notify();
                }
                InputEvent::PressEnter { shift: false, .. } => {
                    if let Some(q) = pane.slash_query(cx)
                        && !pane.filtered_slash_items(&q).is_empty()
                    {
                        return;
                    }
                    pane.send_prompt(window, cx);
                }
                _ => {}
            },
        );
        let sub_model_search = cx.subscribe(&model_search_input, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        let sub_ext = cx.subscribe_in(
            &ext_input,
            window,
            |pane, _, event: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    pane.submit_pending_ext_text(false, cx);
                }
            },
        );

        let pane = Self {
            session,
            project_name: None,
            settings,
            active_file: None,
            attach_active_file: false,
            quoted_selections: Vec::new(),
            quote_popover: None,
            quote_drag: None,
            quote_bounds: Rc::new(Cell::new(Bounds::default())),
            pasted_images: Vec::new(),
            overlay: None,
            overlay_focus: cx.focus_handle(),
            next_image_id: 1,
            pending_ext_prefill: None,
            pending_composer_text: None,
            ext_input,
            ext_textarea,
            toast: None,
            toast_task: Task::ready(()),
            toast_anim_gen: 0,
            turn_navigation_open: false,
            turn_navigation_scroll: ScrollHandle::new(),
            model_popover_open: false,
            model_popover_anim_gen: 0,
            model_search_input,
            favorite_models: load_favorite_models(),
            slash_anim_gen: 0,
            slash_was_open: false,
            slash_index: 0,
            slash_scroll: ScrollHandle::new(),
            last_input_text: String::new(),
            dismissed_slash_text: None,
            collapsed_tool_groups: HashSet::new(),
            expanded_thinking: HashSet::new(),
            expanded_diffs: HashSet::new(),
            expanded_tag_receipts: HashSet::new(),
            expanded_approval_groups: HashSet::new(),
            toggled_task_processes: HashSet::new(),
            viewed_diff_files: HashSet::new(),
            input,
            _math: math,
            _mermaid: mermaid,
            md_extensions,
            scroll: ScrollHandle::new(),
            stick_to_bottom: true,
            _subscriptions: vec![
                sub_input,
                sub_model_search,
                sub_ext,
                sub_math,
                sub_mermaid,
                sub_popover_escape,
            ],
        };
        pane
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        cx.notify();
    }

    fn apply_session_changes(&mut self, changes: SessionChanges, cx: &mut Context<Self>) {
        if changes.reset_transcript {
            self.collapsed_tool_groups.clear();
            self.expanded_thinking.clear();
            self.expanded_diffs.clear();
            self.expanded_tag_receipts.clear();
            self.expanded_approval_groups.clear();
            self.toggled_task_processes.clear();
            self.viewed_diff_files.clear();
            self.quote_popover = None;
            self.quote_drag = None;
        }
        if changes.follow_output {
            self.scroll_to_bottom();
        }
        if let Some((text, is_error)) = changes.toast {
            self.show_toast(text, is_error, cx);
        }
        if let Some(prefill) = changes.extension_prefill {
            self.pending_ext_prefill = Some(prefill);
        }
        if let Some(text) = changes.composer_text {
            self.pending_composer_text = Some(text);
        }
        if let Some(content) = changes.rewind_draft {
            // Restore Pi's original prompt text, including reference paths and quoted context.
            self.pending_composer_text = Some(extract_text_content(Some(&content)));
            self.attach_active_file = false;
            self.quoted_selections.clear();
            self.pasted_images.clear();
            for block in content.as_array().into_iter().flatten() {
                if let Some(image) = PastedImage::from_pi(self.next_image_id, block) {
                    self.next_image_id += 1;
                    self.pasted_images.push(image);
                }
            }
            self.stick_to_bottom = true;
        }
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn open_overlay(&mut self, overlay: ChatOverlay, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some((overlay, window.focused(cx)));
        window.focus(&self.overlay_focus, cx);
        cx.notify();
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Return to the composer or document that had focus before the modal opened.
        if let Some((_, previous_focus)) = self.overlay.take() {
            if let Some(previous_focus) = previous_focus {
                window.focus(&previous_focus, cx);
            } else {
                window.blur(cx);
            }
        }
        cx.notify();
    }

    fn dismiss_quote(&mut self, cx: &mut Context<Self>) {
        self.quote_drag = None;
        if self.quote_popover.take().is_some() {
            cx.notify();
        }
    }

    fn quote_popover_bounds(&self, pending: &AnswerQuotePopover) -> Bounds<Pixels> {
        let bounds = self.quote_bounds.get();
        let width = px(128.).min(bounds.size.width);
        let height = px(30.);
        let x = (pending.position.x - bounds.left()).max(px(4.))
            .min((bounds.size.width - width - px(4.)).max(px(0.)));
        let above = pending.position.y - bounds.top() - height - px(8.);
        let y = if above >= px(4.) { above } else { pending.position.y - bounds.top() + px(20.) };
        let y = y.max(px(4.)).min((bounds.size.height - height - px(4.)).max(px(0.)));
        Bounds::new(point(bounds.left() + x, bounds.top() + y), size(width, height))
    }

    fn render_quote_popover(&self, palette: Palette, typo: AiTypography, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.quote_popover.as_ref()?;
        let bounds = self.quote_popover_bounds(pending);
        let origin = self.quote_bounds.get().origin;
        Some(deferred(
            div()
                .id("ai-add-selection")
                .absolute()
                .left(bounds.left() - origin.x).top(bounds.top() - origin.y)
                .w(bounds.size.width).h(bounds.size.height)
                .rounded(px(7.)).shadow_md()
                .bg(gpui_kit::rgb(palette.surface))
                .border_1().border_color(subtle_border(palette.border))
                .flex().items_center().justify_center()
                .text_size(typo.sm).text_color(gpui_kit::rgb(palette.text))
                .cursor_pointer()
                .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                // The window selection may clear on press; the quote was frozen on release.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|pane, _, window, cx| {
                    let pending = pane.quote_popover.take().expect("visible quote popover");
                    if pending.scroll_offset == pane.scroll.offset()
                        && !pane.quoted_selections.iter().any(|quote|
                            quote.answer == pending.quote.answer && quote.text == pending.quote.text)
                    {
                        pane.quoted_selections.push(pending.quote);
                    }
                    TextSelection::clear(window, cx);
                    pane.quote_drag = None;
                    pane.focus(window, cx);
                    cx.notify();
                }))
                .child(crate::i18n::ai::ADD_TO_CHAT.get()),
        ).with_priority(4).into_any_element())
    }

    fn export_answer(&mut self, markdown: &str, cx: &mut Context<Self>) {
        // This view owns its project's cwd, so exports stay with the source conversation.
        let title: String = self.session.borrow().session_name.as_deref().unwrap_or(crate::i18n::ai::DEFAULT_NOTE_TITLE.get())
            .chars()
            .filter(|ch| !ch.is_control() && !r#"<>:"/\|?*"#.contains(*ch))
            .take(48)
            .collect();
        let title = title.trim_matches([' ', '.']);
        let title = if title.is_empty() { crate::i18n::ai::DEFAULT_NOTE_TITLE.get() } else { title };
        let filename = format!("{title}-{}.md", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"));
        let path = self.session.borrow().cwd.join(&filename);
        let result = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
            .and_then(|mut file| file.write_all(markdown.as_bytes()));
        match result {
            Ok(()) => {
                self.show_toast(crate::i18n::ai::export_note_ok(&filename), false, cx);
                cx.emit(AiPaneEvent::OpenFile(path));
            }
            Err(error) => self.show_toast(crate::i18n::ai::export_note_err(&error), true, cx),
        }
    }

    pub fn set_project_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let name = name.filter(|s| !s.trim().is_empty());
        if self.project_name == name {
            return;
        }
        self.project_name = name;
        cx.notify();
    }

    pub fn refresh_locale(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.model_search_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::ai::MODEL_SEARCH_PLACEHOLDER.get(), window, cx);
        });
        self.ext_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::ai::EXT_INPUT_PLACEHOLDER.get(), window, cx);
        });
        self.ext_textarea.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::ai::EXT_TEXTAREA_PLACEHOLDER.get(), window, cx);
        });
    }

    pub fn sync_settings(&mut self, settings: &AppSettings, cx: &mut Context<Self>) {
        if self.settings.tool_execution_view != settings.tool_execution_view {
            self.toggled_task_processes.clear();
            if settings.tool_execution_view == ToolExecutionView::Detailed {
                self.collapsed_tool_groups.clear();
                self.expanded_approval_groups.clear();
            }
        }
        self.settings = settings.clone();
        cx.notify();
    }

    pub fn set_active_file(&mut self, file: Option<AttachedFile>, cx: &mut Context<Self>) {
        if self.active_file == file {
            return;
        }
        if file.is_none() {
            self.attach_active_file = false;
        }
        self.active_file = file;
        cx.notify();
    }

    pub fn attach_selection(
        &mut self,
        source: String,
        file: Option<AttachedFile>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            self.quoted_selections.push(SelectionQuote {
                source,
                text: trimmed.to_owned(),
                answer: None,
            });
        }
        if let Some(f) = file {
            self.active_file = Some(f);
            self.attach_active_file = true;
        }
        self.open(window, cx);
    }

    fn effective_attached_file(&self) -> Option<&AttachedFile> {
        self.active_file
            .as_ref()
            .filter(|_| self.attach_active_file)
    }

    fn show_toast(&mut self, text: String, is_error: bool, cx: &mut Context<Self>) {
        self.toast = Some((text, is_error));
        self.toast_anim_gen = self.toast_anim_gen.wrapping_add(1);
        self.toast_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(3200))
                .await;
            let _ = this.update(cx, |pane, cx| {
                pane.toast = None;
                cx.notify();
            });
        });
        cx.notify();
    }

    fn respond_confirm(
        &mut self,
        id: &str,
        confirmed: bool,
        auto_allow_shell: bool,
        cx: &mut Context<Self>,
    ) {
        self.session.borrow_mut().respond_confirm(id, confirmed, auto_allow_shell);
        cx.notify();
    }

    fn respond_select(&mut self, option: String, cx: &mut Context<Self>) {
        self.session.borrow_mut().respond_extension(Some(&option));
        cx.notify();
    }

    fn submit_pending_ext_text(&mut self, multiline: bool, cx: &mut Context<Self>) {
        let value = if multiline {
            self.ext_textarea.read(cx).value().to_string()
        } else {
            self.ext_input.read(cx).value().to_string()
        };
        self.session.borrow_mut().respond_extension(Some(&value));
        cx.notify();
    }

    fn cancel_pending_ext(&mut self, cx: &mut Context<Self>) {
        self.session.borrow_mut().respond_extension(None);
        cx.notify();
    }

    fn slash_span_and_query(&self, cx: &App) -> Option<(Range<usize>, String)> {
        let state = self.input.read(cx);
        let content = state.content();
        let text = content.text().as_ref();
        if self.dismissed_slash_text.as_deref() == Some(text) {
            return None;
        }
        let cursor = state.selected_range().start.min(text.len());
        if !text.is_char_boundary(cursor) {
            return None;
        }
        if content
            .tokens()
            .iter()
            .any(|sp| sp.range().start < cursor && cursor < sp.range().end)
        {
            return None;
        }
        let min_start = content
            .tokens()
            .iter()
            .filter(|sp| sp.range().end <= cursor)
            .map(|sp| sp.range().end)
            .max()
            .unwrap_or(0);
        let slice = text.get(min_start..cursor)?;
        let word_rel_start = slice
            .char_indices()
            .rfind(|(_, ch)| ch.is_whitespace())
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        let word_start = min_start + word_rel_start;
        let word = text.get(word_start..cursor)?;
        let query = word.strip_prefix('/')?;
        if query.contains(char::is_whitespace) {
            return None;
        }
        Some((word_start..cursor, query.to_ascii_lowercase()))
    }

    fn slash_query(&self, cx: &App) -> Option<String> {
        self.slash_span_and_query(cx).map(|(_, q)| q)
    }

    fn current_slash_items(&self, cx: &App) -> Vec<SlashItem> {
        self.slash_query(cx)
            .as_deref()
            .map(|q| self.filtered_slash_items(q))
            .unwrap_or_default()
    }

    fn slash_items(&self) -> Vec<SlashItem> {
        let pct_label = self.session.borrow().context_usage
            .as_ref()
            .and_then(|cu| cu.percent)
            .map(crate::i18n::ai::compact_subtitle_used)
            .unwrap_or_else(|| crate::i18n::ai::COMPACT_SUBTITLE_DEFAULT.get().to_owned());

        let mut items = Vec::new();
        if let Some(file) = &self.active_file {
            if self.attach_active_file {
                items.push(SlashItem {
                    id: "detach-active-context".into(),
                    title: crate::i18n::ai::DETACH_CURRENT_CTX_TITLE.get().into(),
                    subtitle: crate::i18n::ai::detach_ctx_subtitle(&file.display),
                    icon: IconName::FileText,
                    action: SlashAction::ToggleActiveContext,
                });
            } else {
                items.push(SlashItem {
                    id: "attach-active-context".into(),
                    title: crate::i18n::ai::ATTACH_CURRENT_CTX_TITLE.get().into(),
                    subtitle: crate::i18n::ai::attach_ctx_subtitle(&file.display),
                    icon: IconName::FileText,
                    action: SlashAction::ToggleActiveContext,
                });
            }
        }
        items.push(SlashItem {
            id: "compact".into(),
            title: crate::i18n::ai::COMPACT_TITLE.get().into(),
            subtitle: pct_label,
            icon: IconName::CircleDot,
            action: SlashAction::Compact,
        });

        for cmd in &self.session.borrow().commands {
            if cmd.source != "skill" {
                continue;
            }
            let display_name = cmd
                .name
                .strip_prefix("skill:")
                .unwrap_or(&cmd.name)
                .to_owned();
            items.push(SlashItem {
                id: cmd.name.clone(),
                title: display_name,
                subtitle: if cmd.description.is_empty() {
                    "skill".to_owned()
                } else {
                    cmd.description.clone()
                },
                icon: IconName::Sparkles,
                action: SlashAction::MountSkill(cmd.name.clone()),
            });
        }
        items
    }

    fn filtered_slash_items(&self, query: &str) -> Vec<SlashItem> {
        self.slash_items()
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item.id.to_ascii_lowercase().contains(query)
                    || item.title.to_ascii_lowercase().contains(query)
                    || item.subtitle.to_ascii_lowercase().contains(query)
            })
            .collect()
    }

    fn execute_slash_item(
        &mut self,
        item: SlashItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismissed_slash_text = None;
        self.slash_index = 0;
        self.slash_was_open = false;
        let slash_range = self.slash_span_and_query(cx).map(|(r, _)| r);
        let clear_slash_span = |pane: &Self, window: &mut Window, cx: &mut Context<Self>| {
            pane.input.update(cx, |input, cx| {
                if let Some(range) = slash_range.clone() {
                    replace_utf8_range(input, range, "", window, cx);
                } else {
                    input.set_value(String::new(), window, cx);
                }
                input.focus(window, cx);
            });
        };

        match item.action {
            SlashAction::ToggleActiveContext => {
                self.attach_active_file = !self.attach_active_file;
                clear_slash_span(self, window, cx);
            }
            SlashAction::Compact => {
                clear_slash_span(self, window, cx);
                self.compact_context(cx);
            }
            SlashAction::MountSkill(skill) => {
                let short = skill.strip_prefix("skill:").unwrap_or(&skill).to_owned();
                let token_id = format!("skill:{short}");
                let token_text = format!("/skill:{short}");
                let token = InlineToken::new(token_id, token_text).with_label(short);
                self.input.update(cx, |input, cx| {
                    let range = slash_range.unwrap_or_else(|| input.selected_range());
                    if input
                        .replace_range_with_token(range, token, window, cx)
                        .is_ok()
                    {
                        input.insert(" ", window, cx);
                    }
                    input.focus(window, cx);
                });
            }
        }
        cx.notify();
    }

    fn insert_path_tokens(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.input.update(cx, |input, cx| {
            for path in paths {
                let rel = path_for_prompt(&self.session.borrow().cwd, path);
                if rel.trim().is_empty() {
                    continue;
                }
                let is_dir = path.is_dir();
                let base_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| rel.clone());
                let label = if is_dir && !base_name.ends_with('/') {
                    format!("{base_name}/")
                } else {
                    base_name
                };
                let prefix = if is_dir { "dir" } else { "file" };
                let token = InlineToken::new(format!("{prefix}:{rel}"), format!("@{rel}"))
                    .with_label(label);
                let sel = input.selected_range();
                if input
                    .replace_range_with_token(sel, token, window, cx)
                    .is_ok()
                {
                    input.insert(" ", window, cx);
                }
            }
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_directory(&self.session.borrow().cwd)
            .set_title(crate::i18n::ai::DIALOG_PICK_ATTACHMENT.get());
        cx.spawn_in(window, async move |this, cx| {
            let Some(files) = dialog.pick_files().await else {
                return;
            };
            let paths: Vec<PathBuf> = files.into_iter().map(|f| f.path().to_path_buf()).collect();
            let _ = this.update_in(cx, |pane, window, cx| {
                pane.insert_path_tokens(&paths, window, cx);
            });
        })
        .detach();
    }

    fn handle_paste(
        &mut self,
        item: &gpui::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut handled = false;
        let mut attach_paths: Vec<PathBuf> = Vec::new();
        let mut added_images = false;
        let mut had_error: Option<String> = None;

        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(img) => {
                    let id = self.next_image_id;
                    self.next_image_id += 1;
                    match clipboard_image_to_pasted(id, img) {
                        Ok(pasted) => {
                            self.pasted_images.push(pasted);
                            added_images = true;
                            handled = true;
                        }
                        Err(err) => {
                            had_error = Some(err);
                            handled = true;
                        }
                    }
                }
                ClipboardEntry::ExternalPaths(ext) => {
                    for p in ext.paths() {
                        if is_image_file_path(p) {
                            let id = self.next_image_id;
                            self.next_image_id += 1;
                            match file_path_to_pasted_image(id, p) {
                                Ok(pasted) => {
                                    self.pasted_images.push(pasted);
                                    added_images = true;
                                }
                                Err(err) => {
                                    had_error = Some(err);
                                }
                            }
                        } else {
                            attach_paths.push(p.clone());
                        }
                    }
                    handled = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }

        if !attach_paths.is_empty() {
            self.insert_path_tokens(&attach_paths, window, cx);
        }
        if let Some(err) = had_error {
            self.show_toast(err, true, cx);
        } else if added_images {
            cx.notify();
        }
        handled
    }

    fn toggle_favorite_model(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.favorite_models.remove(key) {
            self.favorite_models.insert(key.to_owned());
        }
        save_favorite_models(&self.favorite_models);
        cx.notify();
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.session.borrow();
        if session.is_streaming || session.is_rewinding {
            return;
        }
        drop(session);
        let content = self.input.read(cx).content();
        let raw_text = content.text().as_ref();
        let trimmed = raw_text.trim();
        if trimmed.is_empty() && self.quoted_selections.is_empty() && self.pasted_images.is_empty() {
            return;
        }

        if trimmed == "/compact" && self.pasted_images.is_empty() {
            self.input.update(cx, |i, cx| i.set_value(String::new(), window, cx));
            self.compact_context(cx);
            return;
        }

        let mut skills: Vec<String> = Vec::new();
        let mut clean_body = String::with_capacity(raw_text.len());
        let mut cursor = 0usize;
        let mut spans = content.tokens().to_vec();
        spans.sort_by_key(|s| s.range().start);
        for span in &spans {
            let range = span.range();
            if range.start > cursor && range.start <= raw_text.len() {
                clean_body.push_str(&raw_text[cursor..range.start]);
            }
            if let Some(skill_short) = span.token().id().strip_prefix("skill:") {
                let full_skill = format!("skill:{skill_short}");
                if !skills.contains(&full_skill) {
                    skills.push(full_skill);
                }
                clean_body.push_str(span.token().label().as_ref());
            } else if range.end <= raw_text.len() {
                clean_body.push_str(&raw_text[range.start..range.end]);
            }
            cursor = range.end.min(raw_text.len());
        }
        if cursor < raw_text.len() {
            clean_body.push_str(&raw_text[cursor..]);
        }
        let body_trimmed = clean_body.trim();

        let attached = self.effective_attached_file().cloned();
        // Keep all draft attachments until the host accepts the request.
        let quotes = self.quoted_selections.clone();
        let rpc_images: Vec<RpcImage> = self.pasted_images
            .iter()
            .map(|p| RpcImage {
                data: p.data_b64.clone(),
                mime_type: p.mime_type.clone(),
            })
            .collect();
        let msg_images: Vec<Arc<gpui::Image>> = self.pasted_images.iter().map(|p| p.image.clone()).collect();

        let mut payload = String::new();
        if let Some(first_skill) = skills.first() {
            // `pi` splits `/skill:<name> <args>` on the first ASCII space.
            payload.push('/');
            payload.push_str(first_skill);
            payload.push_str(" \n");
        }
        if let Some(file) = &attached
            && !body_trimmed.starts_with('/')
        {
            let rel = path_for_prompt(&self.session.borrow().cwd, &file.path);
            payload.push('@');
            payload.push_str(&rel);
            payload.push_str("\n\n");
        }
        append_quotes(&mut payload, &quotes);
        payload.push_str(body_trimmed);

        let message = ChatMessage::User {
            entry_id: None,
            display_text: body_trimmed.to_owned(),
            timestamp_ms: Some(chrono::Utc::now().timestamp_millis()),
            attached_file: attached.map(|f| f.display),
            mounted_skills: skills,
            quotes,
            images: msg_images,
        };
        let result = self.session.borrow_mut().submit_prompt(&payload, &rpc_images, body_trimmed, message);
        match result {
            Ok(true) => {}
            Ok(false) => {
                cx.notify();
                return;
            }
            Err(err) => {
                self.show_toast(crate::i18n::ai::send_failed(&err), true, cx);
                return;
            }
        }
        self.quoted_selections.clear();
        self.pasted_images.clear();
        self.quote_popover = None;
        self.input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.stick_to_bottom = true;
        self.scroll_to_bottom();
        cx.notify();
    }

    fn abort(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = &self.session.borrow().client {
            let _ = client.abort();
        }
        cx.notify();
    }

    fn compact_context(&mut self, cx: &mut Context<Self>) {
        self.session.borrow_mut().compact();
        cx.notify();
    }

    fn scroll_to_bottom(&self) {
        if self.stick_to_bottom {
            self.scroll.scroll_to_bottom();
        }
    }

}

impl Render for ChatPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((text, multiline)) = self.pending_ext_prefill.take() {
            if multiline {
                self.ext_textarea.update(cx, |input, cx| {
                    input.set_value(text, window, cx);
                    input.focus(window, cx);
                });
            } else {
                self.ext_input.update(cx, |input, cx| {
                    input.set_value(text, window, cx);
                    input.focus(window, cx);
                });
            }
        }
        if let Some(text) = self.pending_composer_text.take() {
            self.input.update(cx, |input, cx| {
                input.set_value(text, window, cx);
                input.focus(window, cx);
            });
        }

        let palette = active_palette();
        let typo = AiTypography::from_settings(&self.settings);
        let ui_font = crate::ui::sans_font(&self.settings.ai_font);

        let is_welcome = self.session.borrow().messages.is_empty()
            && self.session.borrow().startup_error.is_none()
            && self.session.borrow().pending_ui.is_none()
            && self.session.borrow().ext_widgets.is_empty();
        let md_style = self.md_style(typo.body, cx);
        let total_msgs = self.session.borrow().messages.len();
        let can_rewind = self.session.borrow().can_rewind();

        let user_bubble_bg = mix_color(
            palette.surface,
            palette.accent,
            if palette.is_dark() { 0.16 } else { 0.08 },
        );
        let user_bubble_border = mix_color(
            palette.border,
            palette.accent,
            if palette.is_dark() { 0.28 } else { 0.18 },
        );
        let drag_tint = mix_color(
            palette.surface,
            palette.accent,
            if palette.is_dark() { 0.12 } else { 0.06 },
        );

        if self.quote_popover.as_ref().is_some_and(|quote| quote.scroll_offset != self.scroll.offset()) {
            self.quote_popover = None;
        }
        let quote_bounds = self.quote_bounds.clone();
        let popover_bounds = self.quote_popover.as_ref().map(|quote| self.quote_popover_bounds(quote));
        let quote_view = cx.entity().downgrade();

        div()
            .id("folio-ai-pane")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(gpui_kit::rgb(palette.surface))
            .capture_any_mouse_down(cx.listener(move |pane, event: &MouseDownEvent, _, cx| {
                pane.quote_drag = None;
                // Capture also sees scrollbar/button presses that stop bubbling inside the pane.
                if !popover_bounds.is_some_and(|bounds| bounds.contains(&event.position)) {
                    pane.dismiss_quote(cx);
                }
            }))
            .text_color(gpui_kit::rgb(palette.text))
            .font_family(ui_font.clone())
            .text_size(typo.body)
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .drag_over::<DraggedPaths>(move |style, _, _, _| {
                style
                    .bg(drag_tint)
                    .border_color(gpui_kit::rgb(palette.accent))
            })
            .drag_over::<DraggedFolder>(move |style, _, _, _| {
                style
                    .bg(drag_tint)
                    .border_color(gpui_kit::rgb(palette.accent))
            })
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style
                    .bg(drag_tint)
                    .border_color(gpui_kit::rgb(palette.accent))
            })
            .on_drop(cx.listener(|pane, dragged: &DraggedPaths, window, cx| {
                pane.insert_path_tokens(&dragged.0, window, cx);
            }))
            .on_drop(cx.listener(|pane, dragged: &DraggedFolder, window, cx| {
                let full = pane.session.borrow().cwd.join(&dragged.0);
                pane.insert_path_tokens(&[full], window, cx);
            }))
            .on_drop(cx.listener(|pane, external: &ExternalPaths, window, cx| {
                pane.insert_path_tokens(external.paths(), window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, _, _, cx| {
                    let mut changed = false;
                    if pane.model_popover_open {
                        pane.model_popover_open = false;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down_out(cx.listener(|pane, _, _, cx| pane.dismiss_quote(cx)))
            .when(is_welcome, |root| {
                root.child(self.render_welcome(palette, typo, &ui_font, window, cx))
            })
            .when(!is_welcome, |root| {
                if self.session.borrow().is_streaming && self.stick_to_bottom {
                    self.scroll.scroll_to_bottom();
                }
                let thumb_bg = if palette.is_dark() { 0x45474b } else { 0xd9d9d7 };
                let thumb_hover = if palette.is_dark() { 0x5c5f66 } else { 0xbcbcb9 };
                let thumb_active = if palette.is_dark() { 0x72767f } else { 0xa8a8a5 };
                let mut navigation = Vec::new();

                root.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            div()
                                .id("ai-message-stream")
                                .track_scroll(&self.scroll)
                                .size_full()
                                .overflow_y_scroll()
                                .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                                    let delta_y = f32::from(event.delta.pixel_delta(px(20.)).y);
                                    let max_y = f32::from(pane.scroll.max_offset().y);
                                    let cur_y = -f32::from(pane.scroll.offset().y);
                                    let prev_stick = pane.stick_to_bottom;
                                    if delta_y > 0.5 {
                                        pane.stick_to_bottom = false;
                                    } else if cur_y >= (max_y - 28.0).max(0.0) {
                                        pane.stick_to_bottom = true;
                                    }
                                    if pane.stick_to_bottom != prev_stick {
                                        cx.notify();
                                    }
                                }))
                                .pl(px(42.))
                                .pr_3p5()
                                .py_3p5()
                                .flex()
                                .flex_col()
                                .gap_3p5()
                                .when_some(self.session.borrow().startup_error.clone(), |stream, err| {
                                    stream.child(
                                        div()
                                            .p_2p5()
                                            .rounded(px(10.))
                                            .bg(gpui_kit::rgb(palette.surface))
                                            .border_1()
                                            .border_color(gpui_kit::rgb(palette.danger))
                                            .shadow(card_shadow(palette.is_dark()))
                                            .text_size(typo.sm)
                                            .text_color(gpui_kit::rgb(palette.danger))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(err)
                                            .child(
                                                Button::new("ai-retry-start")
                                                    .ghost()
                                                    .compact()
                                                    .label(crate::i18n::common::RETRY.get())
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.session.borrow_mut().connect();
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                })
                    .children({
                        let mut rendered_messages: Vec<AnyElement> = Vec::new();
                        let mut msg_idx = 0usize;
                        while msg_idx < total_msgs {
                            // The startup banner is also an immediate scroll child and must count toward jump indices.
                            let item = rendered_messages.len() + usize::from(self.session.borrow().startup_error.is_some());
                            match &self.session.borrow().messages[msg_idx] {
                                ChatMessage::User {
                                    entry_id,
                                    display_text,
                                    timestamp_ms,
                                    attached_file,
                                    mounted_skills,
                                    quotes,
                                    images,
                                } => {
                                    let text = if !display_text.is_empty() {
                                        display_text.as_str()
                                    } else if let Some(quote) = quotes.first() {
                                        quote.text.as_str()
                                    } else {
                                        attached_file.as_deref().unwrap_or(crate::i18n::ai::IMAGE_MESSAGE.get())
                                    };
                                    let preview = text.lines().map(str::trim)
                                        .find(|line| !line.is_empty()).unwrap_or_default();
                                    navigation.push(TurnAnchor {
                                        item,
                                        timestamp_ms: *timestamp_ms,
                                        preview: preview.chars().take(160).collect(),
                                    });
                                    rendered_messages.push(
                                        div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_end()
                                .child(
                                    div()
                                        .max_w(relative(0.9))
                                        .px_3p5()
                                        .py_2()
                                        .rounded(px(14.))
                                        .bg(user_bubble_bg)
                                        .border_1()
                                        .border_color(user_bubble_border)
                                        .shadow_2xs()
                                        .flex()
                                        .flex_col()
                                        .gap_1p5()
                                        .when(!images.is_empty(), |b| {
                                            b.child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .gap_1p5()
                                                    .children(images.iter().enumerate().map(|(index, im)| {
                                                        let preview = im.clone();
                                                        div()
                                                            .id(SharedString::from(format!("ai-image-{msg_idx}-{index}")))
                                                            .cursor_pointer()
                                                            .on_click(cx.listener(move |pane, _, window, cx| {
                                                                pane.open_overlay(ChatOverlay::Image(preview.clone()), window, cx);
                                                            }))
                                                            .child(
                                                                img(im.clone())
                                                                    .w(px(72.))
                                                                    .h(px(72.))
                                                                    .rounded(px(8.))
                                                                    .object_fit(ObjectFit::Cover),
                                                            )
                                                    })),
                                            )
                                        })
                                        .when(
                                            attached_file.is_some() || !mounted_skills.is_empty(),
                                            |b| {
                                                b.child(
                                                    div()
                                                        .flex()
                                                        .flex_wrap()
                                                        .items_center()
                                                        .gap_1()
                                                        .when_some(attached_file.as_ref(), |r, f| {
                                                            r.child(
                                                                div()
                                                                    .px_1p5()
                                                                    .py(px(1.))
                                                                    .rounded(px(5.))
                                                                    .bg(gpui_kit::rgb(
                                                                        palette.surface,
                                                                    ))
                                                                    .border_1()
                                                                    .border_color(subtle_border(
                                                                        palette.border,
                                                                    ))
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_1()
                                                                    .text_size(typo.xs)
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.accent,
                                                                    ))
                                                                    .child(
                                                                        crate::ui::icon(
                                                                            IconName::FileText,
                                                                        )
                                                                        .w(px(11.))
                                                                        .h(px(11.)),
                                                                    )
                                                                    .child(format!("@{f}")),
                                                            )
                                                        })
                                                        .children(mounted_skills.iter().map(
                                                            |sk| {
                                                                let lbl = sk
                                                                    .strip_prefix("skill:")
                                                                    .unwrap_or(sk)
                                                                    .to_owned();
                                                                div()
                                                                    .px_1p5()
                                                                    .py(px(1.))
                                                                    .rounded(px(5.))
                                                                    .bg(gpui_kit::rgb(
                                                                        palette.surface,
                                                                    ))
                                                                    .border_1()
                                                                    .border_color(subtle_border(
                                                                        palette.border,
                                                                    ))
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_1()
                                                                    .text_size(typo.xs)
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.muted,
                                                                    ))
                                                                    .child(
                                                                        crate::ui::icon(
                                                                            IconName::Sparkles,
                                                                        )
                                                                        .w(px(10.))
                                                                        .h(px(10.))
                                                                        .text_color(gpui_kit::rgb(
                                                                            palette.accent,
                                                                        )),
                                                                    )
                                                                    .child(lbl)
                                                            },
                                                        )),
                                                )
                                            },
                                        )
                                        .children(quotes.iter().map(|q| {
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded(px(6.))
                                                    .bg(gpui_kit::rgb(palette.surface))
                                                    .border_l_2()
                                                    .border_color(gpui_kit::rgb(palette.accent))
                                                    .text_size(typo.sm)
                                                    .text_color(gpui_kit::rgb(palette.muted))
                                                    .child(format!(
                                                        "{}: {}",
                                                        q.source,
                                                        q.text
                                                            .chars()
                                                            .take(160)
                                                            .collect::<String>()
                                                    ))
                                        }))
                                        .when(!display_text.is_empty(), |b| {
                                            b.child(
                                                TextView::markdown(
                                                    SharedString::from(format!(
                                                        "ai-user-md-{msg_idx}"
                                                    )),
                                                    display_text.clone(),
                                                )
                                                .selectable(true)
                                                .style(md_style.clone())
                                                .font_family(ui_font.clone())
                                                .text_size(typo.body),
                                            )
                                        }),
                                )
                                .when(!display_text.is_empty() || (can_rewind && entry_id.is_some()), |col| {
                                    col.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .pt_0p5()
                                            .gap_1()
                                            .when(!display_text.is_empty(), |row| row.child(ai_copy_button(
                                                SharedString::from(format!(
                                                    "ai-copy-user-{msg_idx}"
                                                )),
                                                crate::i18n::ai::COPY_MESSAGE.get(),
                                                crate::i18n::common::COPIED.get(),
                                                display_text.clone(),
                                            )))
                                            .children(entry_id.as_ref().filter(|_| can_rewind).into_iter().flat_map(|entry_id| {
                                                [
                                                    (true, IconName::PencilLine, crate::i18n::ai::EDIT_MESSAGE.get()),
                                                    (false, IconName::RotateCcw, crate::i18n::ai::REWIND_MESSAGE.get()),
                                                ].map(|(edit, icon, label)| {
                                                    let entry_id = entry_id.clone();
                                                    Button::new(SharedString::from(format!("ai-rewind-{edit}-{msg_idx}")))
                                                        .ghost().compact().with_size(px(16.)).w(px(22.)).h(px(22.))
                                                        .icon(crate::ui::icon(icon)).tooltip(label)
                                                        .on_click(cx.listener(move |pane, _, window, cx| {
                                                            pane.open_overlay(ChatOverlay::Rewind {
                                                                entry_id: entry_id.clone(), edit,
                                                            }, window, cx);
                                                        }))
                                                })
                                            })),
                                    )
                                })
                                            .into_any_element(),
                                    );
                                    msg_idx += 1;
                                }
                                ChatMessage::Assistant { .. } => {
                                    let run_start = msg_idx;
                                    let mut run_end = msg_idx + 1;
                                    while matches!(
                                        self.session.borrow().messages.get(run_end),
                                        Some(ChatMessage::Assistant { .. })
                                    ) {
                                        run_end += 1;
                                    }
                                    rendered_messages.push(self.render_assistant_run(
                                        run_start,
                                        run_end,
                                        total_msgs,
                                        &md_style,
                                        &ui_font,
                                        palette,
                                        typo,
                                        cx,
                                    ));
                                    msg_idx = run_end;
                                }
                                ChatMessage::Notice { text, is_error } => {
                                    rendered_messages.push(
                                        div()
                                            .px_3()
                                            .py_1p5()
                                            .rounded(px(9.))
                                            .bg(gpui_kit::rgb(palette.surface))
                                            .border_1()
                                            .border_color(if *is_error {
                                                gpui_kit::rgb(palette.danger)
                                            } else {
                                                subtle_border(palette.border)
                                            })
                                            .text_size(typo.sm)
                                            .text_color(gpui_kit::rgb(if *is_error {
                                                palette.danger
                                            } else {
                                                palette.muted
                                            }))
                                            .child(text.clone())
                                            .into_any_element(),
                                    );
                                    msg_idx += 1;
                                }
                            }
                        }
                        rendered_messages
                    }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(6.))
                            .bottom(px(6.))
                            .left_0()
                            .right_0()
                            .child(
                                Scrollbar::vertical(&self.scroll)
                                    .mode(ScrollbarMode::Always)
                                    .styles(move |styles| {
                                        styles
                                            .thumb(move |style| {
                                                style
                                                    .bg(gpui_kit::rgb(thumb_bg))
                                                    .width(px(6.))
                                                    .inset(px(1.))
                                                    .radius(px(4.))
                                                    .min_length(px(24.))
                                            })
                                            .thumb_hover(move |style| {
                                                style
                                                    .bg(gpui_kit::rgb(thumb_hover))
                                                    .width(px(6.))
                                                    .inset(px(1.))
                                                    .radius(px(4.))
                                            })
                                            .thumb_active(move |style| {
                                                style
                                                    .bg(gpui_kit::rgb(thumb_active))
                                                    .width(px(6.))
                                                    .inset(px(1.))
                                                    .radius(px(4.))
                                            })
                                    }),
                            ),
                    )
                    .when(!navigation.is_empty(), |container| {
                        container.child(self.render_turn_navigation(&navigation, palette, typo, &ui_font, cx))
                    })
                    .when(
                        {
                            let max_y = f32::from(self.scroll.max_offset().y);
                            let cur_y = -f32::from(self.scroll.offset().y);
                            max_y > 20.0 && cur_y < (max_y - 28.0).max(0.0)
                        },
                        |container| {
                            container.child(
                                div()
                                    .absolute()
                                    .bottom(px(12.))
                                    .left_0()
                                    .right_0()
                                    .flex()
                                    .justify_center()
                                    .child(
                                        div()
                                            .id("ai-scroll-to-bottom")
                                            .size(px(32.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(palette.surface))
                                            .border_1()
                                            .border_color(subtle_border(palette.border))
                                            .shadow(card_shadow(palette.is_dark()))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(gpui_kit::rgb(palette.hover)))
                                            .active(|s| s.bg(gpui_kit::rgb(palette.pressed)))
                                            .tooltip(|window, cx| {
                                                Tooltip::new(crate::i18n::ai::SCROLL_TO_BOTTOM.get()).build(window, cx)
                                            })
                                            .child(
                                                crate::ui::icon(IconName::ArrowDown)
                                                    .w(px(15.))
                                                    .h(px(15.))
                                                    .text_color(gpui_kit::rgb(palette.text)),
                                            )
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.stick_to_bottom = true;
                                                pane.scroll.scroll_to_bottom();
                                                cx.notify();
                                            })),
                                    ),
                            )
                        },
                    ),
                )
            })
            .when_some(self.session.borrow().pending_ui.as_ref(), |root, pending| {
                root.child(self.render_pending_ui(pending, palette, cx))
            })
            .when(!self.session.borrow().ext_widgets.is_empty(), |root| {
                root.child(
                    div()
                        .mx_3()
                        .mb_1()
                        .p_2()
                        .rounded(px(9.))
                        .bg(gpui_kit::rgb(palette.surface))
                        .border_1()
                        .border_color(subtle_border(palette.border))
                        .text_size(typo.sm)
                        .text_color(gpui_kit::rgb(palette.muted))
                        .children(
                            self.session.borrow().ext_widgets
                                .values()
                                .flat_map(|lines| lines.iter().cloned())
                                .map(|l| div().child(l)),
                        ),
                )
            })
            .when(!is_welcome, |root| {
                root.child(
                    div()
                        .flex_shrink_0()
                        .w_full()
                        .px_3()
                        .pb_3()
                        .pt_1p5()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .w_full()
                                .max_w(AI_COMPOSER_MAX_WIDTH)
                                .child(self.render_composer(palette, typo, &ui_font, window, cx)),
                        ),
                )
            })
            .child(canvas(move |bounds, _, _| quote_bounds.set(bounds), move |bounds, (), window, _| {
                let view = quote_view.clone();
                window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
                    if phase.capture() && bounds.contains(&event.position) {
                        let _ = view.update(cx, |pane, cx| pane.dismiss_quote(cx));
                    }
                });
            })
                .absolute().inset_0())
            .children(self.render_quote_popover(palette, typo, cx))
            .when_some(self.overlay.as_ref(), |root, (overlay, _)| {
                let viewport = window.viewport_size();
                let dialog = Dialog::new(cx)
                    .focus_handle(self.overlay_focus.clone())
                    .on_close(cx.listener(|pane, _, window, cx| pane.close_overlay(window, cx)));
                root.child(match overlay {
                    ChatOverlay::Image(image) => dialog
                        .accessibility_label(crate::i18n::ai::IMAGE_MESSAGE.get())
                        .backdrop(div().size_full().occlude().bg(gpui::black().opacity(0.56)))
                        .popup(
                            DialogPopup::new()
                                .relative()
                                .rounded(px(10.))
                                .overflow_hidden()
                                .bg(gpui_kit::rgb(palette.surface))
                                .shadow(popover_shadow(palette.is_dark()))
                                .child(
                                    img(image.clone())
                                        .id("ai-image-preview")
                                        .max_w(viewport.width * 0.9)
                                        .max_h(viewport.height * 0.9)
                                        .object_fit(ObjectFit::Contain),
                                )
                                .child(div().absolute().top_2().right_2().child(
                                    Button::new("close-ai-image-preview")
                                        .small()
                                        .icon(crate::ui::icon(IconName::Close))
                                        .tooltip(crate::i18n::common::CLOSE.get())
                                        .on_click(cx.listener(|pane, _, window, cx| pane.close_overlay(window, cx))),
                                )),
                        ),
                    ChatOverlay::Rewind { entry_id, edit } => {
                        use crate::i18n::ai;
                        let entry_id = entry_id.clone();
                        let edit = *edit;
                        let (label, detail) = if edit {
                            (ai::EDIT_MESSAGE.get(), ai::EDIT_REWIND_DETAIL.get())
                        } else {
                            (ai::REWIND_MESSAGE.get(), ai::REWIND_DETAIL.get())
                        };
                        dialog.accessibility_label(label)
                            .on_ok(|_, _, _| false)
                            .popup(div().absolute().inset_0().child(crate::ui::modal_overlay(
                                label,
                                detail,
                                div().flex().justify_end().gap_2()
                                    .child(crate::ui::button_secondary("cancel-ai-rewind", crate::i18n::common::CANCEL.get())
                                        .px_3().min_w(px(64.))
                                        .on_click(cx.listener(|pane, _, window, cx| pane.close_overlay(window, cx))))
                                    .child(crate::ui::button_danger("confirm-ai-rewind", label)
                                        .min_w(px(64.))
                                        .on_click(cx.listener(move |pane, _, window, cx| {
                                            pane.close_overlay(window, cx);
                                            let result = pane.session.borrow_mut().rewind(&entry_id, edit);
                                            if let Err(error) = result {
                                                pane.show_toast(ai::command_failed(ai::REWIND_MESSAGE.get(), &error), true, cx);
                                            }
                                        }))),
                            )))
                    }
                })
            })
    }
}

fn replace_utf8_range(
    input: &mut TextareaState,
    range: Range<usize>,
    replacement: &str,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    let text = input.value();
    let start = range.start.min(text.len());
    let end = range.end.min(text.len());
    if start > end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return;
    }
    let start_u16 = text[..start].encode_utf16().count();
    let end_u16 = start_u16 + text[start..end].encode_utf16().count();
    EntityInputHandler::replace_text_in_range(
        input,
        Some(start_u16..end_u16),
        replacement,
        window,
        cx,
    );
}

fn subtle_border(border: u32) -> Rgba {
    let mut c = gpui_kit::rgb(border);
    c.a = 0.78;
    c
}

fn fg_alpha(hex: u32, alpha: f32) -> Rgba {
    let mut c = gpui_kit::rgb(hex);
    c.a = alpha;
    c
}

#[derive(Clone, Copy)]
struct ProcessStyle {
    fg_secondary: Rgba,
    fg_tertiary: Rgba,
    hover_bg: Rgba,
    guide_line: Rgba,
    diff_add: Rgba,
    diff_del: Rgba,
    error_fg: Rgba,
    unviewed_dot: Rgba,
    header_size: Pixels,
    detail_size: Pixels,
    meta_size: Pixels,
    icon_size: Pixels,
    row_gap: Pixels,
    section_gap: Pixels,
}

impl ProcessStyle {
    fn from_theme(palette: Palette, typo: AiTypography) -> Self {
        let dark = palette.is_dark();
        Self {
            fg_secondary: fg_alpha(palette.text, 0.60),
            fg_tertiary: fg_alpha(palette.text, 0.42),
            hover_bg: fg_alpha(palette.text, 0.06),
            guide_line: fg_alpha(palette.text, 0.10),
            diff_add: mix_color(palette.muted, 0x16a34a, if dark { 0.50 } else { 0.58 }),
            diff_del: mix_color(palette.muted, palette.danger, if dark { 0.50 } else { 0.58 }),
            error_fg: mix_color(palette.text, palette.danger, 0.80),
            unviewed_dot: gpui_kit::rgb(palette.accent),
            header_size: px(f32::from(typo.sm).clamp(12.5, 14.0)),
            detail_size: typo.sm,
            meta_size: typo.xs,
            icon_size: px(13.),
            row_gap: px(4.),
            section_gap: px(16.),
        }
    }
}

enum ProcessStep<'a> {
    Thinking {
        msg_idx: usize,
        part_idx: usize,
        text: &'a str,
        duration_ms: Option<u64>,
        streaming: bool,
    },
    Tool {
        msg_idx: usize,
        part_idx: usize,
        block: &'a ToolBlock,
    },
}

impl ProcessStep<'_> {
    fn ids(&self) -> (usize, usize) {
        match self {
            ProcessStep::Thinking {
                msg_idx, part_idx, ..
            }
            | ProcessStep::Tool {
                msg_idx, part_idx, ..
            } => (*msg_idx, *part_idx),
        }
    }
}

fn format_duration_compact(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let total_secs = (ms / 1000).max(1);
    if total_secs < 60 {
        format!("{total_secs}s")
    } else {
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    }
}

fn format_thinking_duration(ms: u64) -> String {
    crate::i18n::ai::format_duration_zh_en((ms / 1000).max(1))
}

fn process_group_title(steps: &[ProcessStep<'_>], is_running: bool) -> String {
    let mut has_edit = false;
    let mut has_cmd = false;
    let mut has_read = false;
    let mut has_other = false;
    let mut has_thinking = false;

    for step in steps {
        match step {
            ProcessStep::Thinking { .. } => has_thinking = true,
            ProcessStep::Tool { block, .. } => match block.name.as_str() {
                "edit" | "write" => has_edit = true,
                "bash" | "powershell" | "run_python" => has_cmd = true,
                "read" | "grep" | "find" | "ls" => has_read = true,
                _ => has_other = true,
            },
        }
    }

    let mut verbs: Vec<&'static str> = Vec::new();
    if is_running {
        if has_edit {
            verbs.push(crate::i18n::ai::VERB_EDITING_FILE.get());
        }
        if has_cmd {
            verbs.push(crate::i18n::ai::VERB_RUNNING_CMD.get());
        }
        if has_read && verbs.is_empty() {
            verbs.push(crate::i18n::ai::VERB_READING_FILE.get());
        }
        if has_other && verbs.is_empty() {
            verbs.push(crate::i18n::ai::VERB_CALLING_TOOL.get());
        }
        if verbs.is_empty() && has_thinking {
            return crate::i18n::ai::STILL_THINKING.get().to_owned();
        }
        return format!("{}…", verbs.join(" · "));
    }

    if has_edit {
        verbs.push(crate::i18n::ai::VERB_EDITED_FILE.get());
    }
    if has_cmd {
        verbs.push(crate::i18n::ai::VERB_RAN_CMD.get());
    }
    if has_read && !has_edit && !has_cmd {
        verbs.push(crate::i18n::ai::VERB_READ_FILE.get());
    } else if has_read && verbs.len() < 2 {
        verbs.push(crate::i18n::ai::VERB_READ_FILE.get());
    }
    if has_other {
        verbs.push(crate::i18n::ai::VERB_CALLED_TOOL.get());
    }
    if verbs.is_empty() && has_thinking {
        return crate::i18n::ai::THINKING_PROCESS.get().to_owned();
    }
    verbs.join(" · ")
}

fn process_group_icon(steps: &[ProcessStep<'_>]) -> IconName {
    let has_edit = steps
        .iter()
        .any(|s| matches!(s, ProcessStep::Tool { block, .. } if block.name == "edit" || block.name == "write"));
    let has_cmd = steps
        .iter()
        .any(|s| matches!(s, ProcessStep::Tool { block, .. } if matches!(block.name.as_str(), "bash" | "powershell" | "run_python")));
    let has_read = steps
        .iter()
        .any(|s| matches!(s, ProcessStep::Tool { block, .. } if matches!(block.name.as_str(), "read" | "grep" | "find" | "ls")));
    let has_tool = steps
        .iter()
        .any(|s| matches!(s, ProcessStep::Tool { .. }));
    if has_edit {
        IconName::PencilLine
    } else if has_cmd {
        IconName::Terminal
    } else if has_read {
        IconName::FileText
    } else if has_tool {
        IconName::Wrench
    } else {
        IconName::Brain
    }
}

fn render_process_header(
    id: SharedString,
    icon: Option<IconName>,
    label: String,
    is_running: bool,
    is_error: bool,
    expanded: bool,
    ps: ProcessStyle,
) -> Stateful<Div> {
    let fg = if is_error { ps.error_fg } else { ps.fg_secondary };
    let chevron = div()
        .opacity(0.38)
        .group_hover(id.clone(), |s| s.opacity(0.85))
        .child(
            crate::ui::icon(IconName::ChevronRight)
                .w(px(12.))
                .h(px(12.))
                .text_color(fg)
                .with_animation(
                    (id.clone(), expanded as usize),
                    Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                    move |ic, t| {
                        let angle = if expanded {
                            t * std::f32::consts::FRAC_PI_2
                        } else {
                            (1.0 - t) * std::f32::consts::FRAC_PI_2
                        };
                        ic.rotate(gpui::radians(angle))
                    },
                ),
        );

    let title_el = if is_running {
        div()
            .text_size(ps.header_size)
            .text_color(fg)
            .with_animation(
                (id.clone(), 91usize),
                Animation::new(Duration::from_millis(1200)).repeat(),
                |el, t| {
                    let a = 0.55 + 0.45 * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos());
                    el.opacity(a)
                },
            )
            .child(label)
            .into_any_element()
    } else {
        div()
            .text_size(ps.header_size)
            .text_color(fg)
            .child(label)
            .into_any_element()
    };

    div()
        .id(id.clone())
        .group(id.clone())
        .px_1p5()
        .py_1()
        .min_h(px(24.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .gap_1p5()
        .cursor_pointer()
        .hover(move |s| s.bg(ps.hover_bg))
        .when_some(icon, |row, ic| {
            let icon_el = if is_running {
                crate::ui::icon(IconName::RefreshCw)
                    .w(ps.icon_size)
                    .h(ps.icon_size)
                    .flex_shrink_0()
                    .text_color(fg)
                    .with_animation(
                        (id.clone(), 77usize),
                        Animation::new(Duration::from_millis(900)).repeat(),
                        |svg, t| svg.rotate(gpui::radians(t * std::f32::consts::TAU)),
                    )
                    .into_any_element()
            } else {
                crate::ui::icon(if is_error { IconName::ShieldAlert } else { ic })
                    .w(ps.icon_size)
                    .h(ps.icon_size)
                    .flex_shrink_0()
                    .text_color(fg)
                    .into_any_element()
            };
            row.child(icon_el)
        })
        .child(title_el)
        .child(chevron)
}

#[allow(clippy::too_many_arguments)]
fn render_command_detail_row(
    id: SharedString,
    icon: IconName,
    text: String,
    done: bool,
    is_error: bool,
    duration_ms: Option<u64>,
    text_font: &SharedString,
    meta_font: &SharedString,
    ps: ProcessStyle,
) -> Stateful<Div> {
    let fg = if is_error {
        ps.error_fg
    } else {
        ps.fg_tertiary
    };
    let icon_el = if !done {
        crate::ui::icon(IconName::RefreshCw)
            .w(ps.icon_size)
            .h(ps.icon_size)
            .flex_shrink_0()
            .text_color(fg)
            .with_animation(
                (id.clone(), 1usize),
                Animation::new(Duration::from_millis(900)).repeat(),
                |svg, t| svg.rotate(gpui::radians(t * std::f32::consts::TAU)),
            )
            .into_any_element()
    } else {
        crate::ui::icon(if is_error { IconName::ShieldAlert } else { icon })
            .w(ps.icon_size)
            .h(ps.icon_size)
            .flex_shrink_0()
            .text_color(fg)
            .into_any_element()
    };

    div()
        .id(id.clone())
        .relative()
        .px_1p5()
        .py_1()
        .min_h(px(24.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .flex_1()
                .min_w_0()
                .child(icon_el)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(text_font.clone())
                        .text_size(ps.detail_size)
                        .text_color(fg)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(text),
                ),
        )
        .when_some(duration_ms, |r, ms| {
            r.child(
                div()
                    .flex_shrink_0()
                    .font_family(meta_font.clone())
                    .text_size(ps.meta_size)
                    .text_color(ps.fg_tertiary)
                    .child(format_duration_compact(ms)),
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn render_file_edit_row(
    id: SharedString,
    diff: &FileDiff,
    is_created: bool,
    is_error: bool,
    unviewed: bool,
    code_font: &SharedString,
    _palette: Palette,
    ps: ProcessStyle,
    on_open_file: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_toggle_diff: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let fg = if is_error {
        ps.error_fg
    } else {
        ps.fg_tertiary
    };
    let name_fg = if is_error {
        ps.error_fg
    } else {
        ps.fg_secondary
    };
    let file_name = Path::new(&diff.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| diff.path.clone());
    let verb = if is_created {
        crate::i18n::ai::VERB_CREATED.get()
    } else {
        crate::i18n::ai::VERB_EDITED.get()
    };
    let file_path: SharedString = diff.path.clone().into();

    div()
        .id(id.clone())
        .relative()
        .px_1p5()
        .py_1()
        .min_h(px(24.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .cursor_pointer()
        .hover(move |s| s.bg(ps.hover_bg))
        .on_click(on_toggle_diff)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .flex_1()
                .min_w_0()
                .child(
                    crate::ui::icon(IconName::PencilLine)
                        .w(ps.icon_size)
                        .h(ps.icon_size)
                        .flex_shrink_0()
                        .text_color(fg),
                )
                .child(
                    div()
                        .text_size(ps.detail_size)
                        .text_color(fg)
                        .flex_shrink_0()
                        .child(verb),
                )
                .child(
                    div()
                        .id((id.clone(), 9usize))
                        .min_w_0()
                        .cursor_pointer()
                        .border_b_1()
                        .border_dashed()
                        .border_color(ps.guide_line)
                        .text_size(ps.detail_size)
                        .text_color(name_fg)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .tooltip(move |window, cx| Tooltip::new(file_path.clone()).build(window, cx))
                        .hover(move |s| s.underline().border_color(ps.fg_secondary))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(on_open_file)
                        .child(file_name),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .flex_shrink_0()
                        .font_family(code_font.clone())
                        .text_size(ps.meta_size)
                        .child(
                            div()
                                .text_color(ps.diff_add)
                                .child(format!("+{}", diff.added)),
                        )
                        .child(
                            div()
                                .text_color(ps.diff_del)
                                .child(format!("-{}", diff.removed)),
                        ),
                ),
        )
        .when(unviewed, |row| {
            row.child(
                div()
                    .size(px(5.))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(ps.unviewed_dot),
            )
        })
        .with_animation(
            (id, 3usize),
            Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).top(px(4.0 * (1.0 - t))),
        )
        .into_any_element()
}

fn render_thinking_detail(
    id: SharedString,
    text: String,
    streaming: bool,
    ui_font: &SharedString,
    ps: ProcessStyle,
) -> AnyElement {
    ThinkingScrollBox {
        id,
        text,
        streaming,
        ui_font: ui_font.clone(),
        ps,
    }
    .into_any_element()
}

#[derive(IntoElement)]
struct DiffPatchView {
    id: SharedString,
    patch: EditPatch,
}

impl RenderOnce for DiffPatchView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = gpui_kit::component::theme::Theme::global(cx);
        let rem = window.rem_size();
        // Match the native diff's row metrics so short edits hug their content.
        let font_size = rem * (f32::from(theme.mono_font_size) / 16.);
        let line_height = font_size * 1.6;
        let height = line_height * self.patch.line_numbers.len() + rem * 0.25 * self.patch.hunk_count;
        // The native marker lane is one row wide. Include the row's left
        // padding so multi-digit numbers stay inside the scroll viewport.
        let left_inset = rem * 0.5;
        let number_size = font_size.min(
            (line_height + left_inset - px(2.)) / (self.patch.line_number_digits as f32 * 0.65),
        );
        let number_color = theme.muted_foreground;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| {
            DiffState::new([self.patch.file], cx).with_context_lines(None)
        });
        div()
            .id(self.id)
            .min_w_0()
            .rounded(px(6.))
            .overflow_hidden()
            .when(height > px(280.), |el| el.on_scroll(|_, _, cx| cx.stop_propagation()))
            .child(
                Diff::new(&state)
                    .header_visible(false)
                    .line_number(false)
                    .gutter_markers(self.patch.line_numbers.iter().cloned())
                    .render_gutter_marker(move |marker, _, _| {
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(-left_inset)
                            .right_0()
                            .pr(px(2.))
                            .flex()
                            .items_center()
                            .justify_end()
                            .text_size(number_size)
                            .text_color(number_color)
                            .child(marker.position().line().to_string())
                    })
                    .hunk_separator(DiffHunkSeparator::Simple)
                    .change_indicator(DiffChangeIndicator::Bars)
                    .h((height + px(2.)).min(px(280.))),
            )
    }
}

#[derive(IntoElement)]
struct ThinkingScrollBox {
    id: SharedString,
    text: String,
    streaming: bool,
    ui_font: SharedString,
    ps: ProcessStyle,
}

impl RenderOnce for ThinkingScrollBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scroll = window
            .use_keyed_state((self.id.clone(), 44usize), cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        if self.streaming {
            scroll.scroll_to_bottom();
        }
        div()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id(self.id)
                    .track_scroll(&scroll)
                    .px_1p5()
                    .py_1()
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .font_family(self.ui_font)
                    .text_size(self.ps.detail_size)
                    .line_height(relative(1.55))
                    .text_color(self.ps.fg_secondary)
                    .child(self.text),
            )
    }
}

fn card_shadow(dark: bool) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(4.), hsla(0., 0., 0., if dark { 0.34 } else { 0.06 }))
            .blur_radius(px(14.)),
        BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., if dark { 0.22 } else { 0.03 }))
            .blur_radius(px(3.)),
    ]
}

fn popover_shadow(dark: bool) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(10.), hsla(0., 0., 0., if dark { 0.45 } else { 0.12 }))
            .blur_radius(px(26.)),
        BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., if dark { 0.25 } else { 0.04 }))
            .blur_radius(px(6.)),
    ]
}

fn mix_color(base: u32, tint: u32, amount: f32) -> Rgba {
    let base = gpui_kit::rgb(base);
    let tint = gpui_kit::rgb(tint);
    Rgba {
        r: base.r * (1. - amount) + tint.r * amount,
        g: base.g * (1. - amount) + tint.g * amount,
        b: base.b * (1. - amount) + tint.b * amount,
        a: 1.,
    }
}

fn short_model_name(name: &str) -> String {
    let first = name.split('·').next().unwrap_or(name).trim();
    first.chars().take(20).collect()
}

fn thinking_short_label(level: &str) -> String {
    match level {
        "off" => "Off".into(),
        "minimal" => "Minimal".into(),
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "XHigh".into(),
        "max" => "Max".into(),
        other => other.to_owned(),
    }
}

fn model_brand_icon_path(model: &ModelInfo) -> Option<&'static str> {
    let provider = model.provider.to_ascii_lowercase();
    let id = model.id.to_ascii_lowercase();
    let name = model.name.to_ascii_lowercase();
    for s in [&id, &name, &provider] {
        if s.contains("claude") || s.contains("anthropic") {
            return Some("brand/claude.svg");
        }
        if s.contains("gemini") || s.contains("google") {
            return Some("brand/gemini.svg");
        }
        if s.contains("hunyuan")
            || s.starts_with("hy")
            || s.starts_with("Hy")
        {
            return Some("brand/hunyuan.svg");
        }
        if s.contains("deepseek") {
            return Some("brand/deepseek.svg");
        }
        if s.contains("qwen")
            || s.contains("tongyi")
            || s.contains("dashscope")
            || s.contains("alibaba")
        {
            return Some("brand/qwen.svg");
        }
        if s.contains("glm") || s.contains("zhipu") || s.contains("chatglm") {
            return Some("brand/zhipu.svg");
        }
        if s.contains("grok") || s.contains("xai") || s.contains("x-ai") {
            return Some("brand/grok.svg");
        }
        if s.contains("kimi") || s.contains("moonshot") {
            return Some("brand/kimi.svg");
        }
        if s.contains("minimax") {
            return Some("brand/minimax.svg");
        }
        if s.contains("mistral")
            || s.contains("codestral")
            || s.contains("mixtral")
            || s.contains("pixtral")
        {
            return Some("brand/mistral.svg");
        }
        if s.contains("llama") || s.contains("meta") {
            return Some("brand/meta.svg");
        }
        if s.contains("antigravity") {
            return Some("brand/antigravity.svg");
        }
        if s.contains("openai")
            || s.contains("codex")
            || s.contains("gpt")
            || s.starts_with("o1")
            || s.starts_with("o3")
            || s.starts_with("o4")
        {
            return Some("brand/codex.svg");
        }
    }
    None
}

fn render_model_icon(model: Option<&ModelInfo>, size: Pixels, tint: u32) -> AnyElement {
    if let Some(path) = model.and_then(model_brand_icon_path) {
        if path == "brand/codex.svg" {
            return gpui::svg()
                .path(path)
                .w(size)
                .h(size)
                .flex_shrink_0()
                .text_color(gpui_kit::rgb(tint))
                .into_any_element();
        }
        return img(path)
            .w(size)
            .h(size)
            .object_fit(ObjectFit::Contain)
            .flex_shrink_0()
            .into_any_element();
    }
    crate::ui::icon(IconName::Sparkles)
        .w(size)
        .h(size)
        .flex_shrink_0()
        .text_color(gpui_kit::rgb(tint))
        .into_any_element()
}

fn ai_copy_button(
    id: impl Into<ElementId>,
    idle_tip: &'static str,
    copied_tip: &'static str,
    text: String,
) -> impl IntoElement {
    AiCopyButton {
        id: id.into(),
        idle_tip,
        copied_tip,
        text,
    }
}

#[derive(IntoElement)]
struct AiCopyButton {
    id: ElementId,
    idle_tip: &'static str,
    copied_tip: &'static str,
    text: String,
}

struct AiCopyFeedback {
    copied: bool,
    reset: Task<()>,
}

impl RenderOnce for AiCopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = active_palette();
        let feedback = window.use_keyed_state(self.id.clone(), cx, |_, _| AiCopyFeedback {
            copied: false,
            reset: Task::ready(()),
        });
        let copied = feedback.read(cx).copied;
        let text = self.text;
        let tip = if copied {
            self.copied_tip
        } else {
            self.idle_tip
        };
        let idle_fg = fg_alpha(palette.text, if copied { 0.85 } else { 0.55 });
        let hover_fg = fg_alpha(palette.text, 0.85);
        let hover_bg = fg_alpha(palette.text, 0.06);

        div()
            .id(self.id)
            .size(AI_COPY_BUTTON_SIZE)
            .rounded(AI_COPY_BUTTON_RADIUS)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(idle_fg)
            .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
            .child(
                crate::ui::icon(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .w(AI_COPY_ICON_SIZE)
                .h(AI_COPY_ICON_SIZE),
            )
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                feedback.update(cx, |feedback, cx| {
                    feedback.copied = true;
                    cx.notify();
                    feedback.reset = cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(1500))
                            .await;
                        let _ = this.update(cx, |feedback, cx| {
                            feedback.copied = false;
                            cx.notify();
                        });
                    });
                });
            })
    }
}

fn favorite_models_path() -> Option<PathBuf> {
    crate::storage::data_directory()
        .ok()
        .map(|d| d.join("ai-favorite-models.json"))
}

fn load_favorite_models() -> HashSet<String> {
    let Some(path) = favorite_models_path() else {
        return HashSet::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

fn save_favorite_models(favs: &HashSet<String>) {
    let Some(path) = favorite_models_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut items: Vec<&String> = favs.iter().collect();
    items.sort();
    if let Ok(json) = serde_json::to_string_pretty(&items) {
        let _ = std::fs::write(path, json);
    }
}

fn is_image_file_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
    )
}

fn convert_bytes_to_png(bytes: &[u8], src_format: image::ImageFormat) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory_with_format(bytes, src_format)
        .map_err(|e| crate::i18n::ai::err_decode_image(&e))?;
    let mut out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| crate::i18n::ai::err_encode_png(&e))?;
    Ok(out)
}

fn clipboard_image_to_pasted(id: u64, img: &gpui::Image) -> Result<PastedImage, String> {
    let (gpui_fmt, mime, bytes) = match img.format {
        ImageFormat::Png => (ImageFormat::Png, "image/png", img.bytes.clone()),
        ImageFormat::Jpeg => (ImageFormat::Jpeg, "image/jpeg", img.bytes.clone()),
        ImageFormat::Webp => (ImageFormat::Webp, "image/webp", img.bytes.clone()),
        ImageFormat::Gif => (ImageFormat::Gif, "image/gif", img.bytes.clone()),
        ImageFormat::Bmp => (
            ImageFormat::Png,
            "image/png",
            convert_bytes_to_png(&img.bytes, image::ImageFormat::Bmp)?,
        ),
        ImageFormat::Tiff => (
            ImageFormat::Png,
            "image/png",
            convert_bytes_to_png(&img.bytes, image::ImageFormat::Tiff)?,
        ),
        ImageFormat::Svg => return Err(crate::i18n::ai::ERR_PASTE_SVG.get().to_owned()),
        ImageFormat::Ico | ImageFormat::Pnm => {
            return Err(crate::i18n::ai::ERR_PASTE_IMAGE_FORMAT.get().to_owned())
        }
    };
    let data_b64 = encode_base64(&bytes);
    let render_img = Arc::new(gpui::Image::from_bytes(gpui_fmt, bytes));
    Ok(PastedImage {
        id,
        image: render_img,
        mime_type: mime.to_owned(),
        data_b64,
        label: crate::i18n::ai::image_label(id),
    })
}

impl PastedImage {
    fn from_pi(id: u64, block: &Value) -> Option<Self> {
        if block["type"].as_str()? != "image" {
            return None;
        }
        let mime_type = block["mimeType"].as_str()?;
        let format = match mime_type {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            "image/webp" => ImageFormat::Webp,
            "image/gif" => ImageFormat::Gif,
            _ => return None,
        };
        let data = block["data"].as_str()?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
        Some(Self {
            id,
            image: Arc::new(gpui::Image::from_bytes(format, bytes)),
            mime_type: mime_type.to_owned(),
            data_b64: data.to_owned(),
            label: crate::i18n::ai::image_label(id),
        })
    }
}

fn file_path_to_pasted_image(id: u64, path: &Path) -> Result<PastedImage, String> {
    let bytes = std::fs::read(path).map_err(|e| crate::i18n::ai::err_read_image(&e))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (gpui_fmt, mime, final_bytes) = match ext.as_str() {
        "png" => (ImageFormat::Png, "image/png", bytes),
        "jpg" | "jpeg" => (ImageFormat::Jpeg, "image/jpeg", bytes),
        "webp" => (ImageFormat::Webp, "image/webp", bytes),
        "gif" => (ImageFormat::Gif, "image/gif", bytes),
        "bmp" => (
            ImageFormat::Png,
            "image/png",
            convert_bytes_to_png(&bytes, image::ImageFormat::Bmp)?,
        ),
        _ => return Err(crate::i18n::ai::ERR_UNSUPPORTED_IMAGE_FORMAT.get().to_owned()),
    };
    let label = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::i18n::ai::image_label(id));
    let data_b64 = encode_base64(&final_bytes);
    let render_img = Arc::new(gpui::Image::from_bytes(gpui_fmt, final_bytes));
    Ok(PastedImage {
        id,
        image: render_img,
        mime_type: mime.to_owned(),
        data_b64,
        label,
    })
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn parse_model(val: &Value) -> Option<ModelInfo> {
    let id = val.get("id")?.as_str()?.to_owned();
    let provider = val
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let name = val
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&id)
        .to_owned();
    let reasoning = val
        .get("reasoning")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(ModelInfo {
        provider,
        id,
        name,
        reasoning,
    })
}

fn extract_text_content(val: Option<&Value>) -> String {
    let Some(val) = val else {
        return String::new();
    };
    match val {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text").and_then(|v| v.as_str()).map(str::to_owned)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn extract_path_arg(args: &Value) -> Option<String> {
    args.get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

fn summarize_tool_call(name: &str, args: &Value, diffs: &[FileDiff]) -> String {
    if let Some(first) = diffs.first() {
        let fname = Path::new(&first.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.path.clone());
        return crate::i18n::ai::edited_diff(&fname, first.added, first.removed);
    }
    match name {
        "folio_tags_list" => crate::i18n::ai::TOOL_TAGS_LIST.get().into(),
        "folio_files_query" => crate::i18n::ai::TOOL_FILES_QUERY.get().into(),
        "folio_file_tags_update" => {
            if let Some(folder) = args["under_directory"].as_str() {
                crate::i18n::ai::tool_edit_folder_tags(folder)
            } else {
                crate::i18n::ai::tool_edit_files_tags(args["paths"].as_array().map_or(0, Vec::len))
            }
        }
        "folio_tags_manage" => crate::i18n::ai::tool_manage_tag(args["tag"].as_str().unwrap_or_default()),
        "folio_kb_search" => {
            let summary = args["intent"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| args["searches"][0]["query"].as_str())
                .unwrap_or("");
            let query: String = summary.chars().take(48).collect();
            crate::i18n::ai::tool_kb_search(&query)
        }
        "folio_kb_status" => crate::i18n::ai::TOOL_KB_STATUS.get().into(),
        "folio_kb_reindex" => crate::i18n::ai::TOOL_KB_REINDEX.get().into(),
        "folio_pdf_read" => crate::i18n::ai::tool_pdf_read(args["path"].as_str().unwrap_or_default()),
        "folio_tex_compile" => {
            if let Some(folder) = args["under_directory"].as_str() {
                crate::i18n::ai::tool_compile_tex_folder(folder)
            } else if let Some(paths) = args["paths"].as_array() {
                crate::i18n::ai::tool_compile_tex_files(paths.len())
            } else {
                crate::i18n::ai::TOOL_COMPILE_TEX.get().into()
            }
        }
        "folio_drawio_read" => crate::i18n::ai::tool_drawio("read", args["path"].as_str().unwrap_or_default()),
        "folio_drawio_create" => crate::i18n::ai::tool_drawio("create", args["path"].as_str().unwrap_or_default()),
        "folio_drawio_update" => crate::i18n::ai::tool_drawio("update", args["path"].as_str().unwrap_or_default()),
        "folio_drawio_get_selection" => "读取 Draw.io 选区".into(),
        "folio_drawio_edit_cells" => format!(
            "编辑 Draw.io 元素 ({})",
            args["operations"].as_array().map_or(0, Vec::len)
        ),
        "run_python" => crate::i18n::ai::TOOL_RUN_PYTHON.get().into(),
        "bash" | "powershell" => args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or(name)
            .chars()
            .take(64)
            .collect(),
        "read" | "edit" | "write" => {
            let path = extract_path_arg(args).unwrap_or_default();
            format!("{name} {path}")
        }
        "grep" | "find" | "ls" => {
            let pat = args
                .get("pattern")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{name} {pat}")
        }
        _ => format!("{name} {}", serde_json::to_string(args).expect("tool arguments are JSON")),
    }
}

fn build_diffs_for_tool(
    name: &str,
    args: &Value,
    write_before: Option<&str>,
    edit_patch: Option<&str>,
) -> Vec<FileDiff> {
    let path = extract_path_arg(args).unwrap_or_else(|| "file".to_owned());
    match name {
        "edit" => {
            if let Some(patch) = edit_patch
                && let Some(diff) = FileDiff::from_edit_patch(path.clone(), patch)
            {
                return vec![diff];
            }
            let edits = args.get("edits").and_then(Value::as_array)
                .map_or(std::slice::from_ref(args), Vec::as_slice);
            let mut diff = FileDiff { path, ..Default::default() };
            for edit in edits {
                let old_t = edit
                    .get("oldText")
                    .or_else(|| edit.get("old_text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new_t = edit
                    .get("newText")
                    .or_else(|| edit.get("new_text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !old_t.is_empty() || !new_t.is_empty() {
                    if !diff.lines.is_empty() && diff.lines.last() != Some(&DiffLine::Ellipsis) {
                        diff.lines.push(DiffLine::Ellipsis);
                    }
                    diff.append_preview(old_t, new_t, false);
                }
            }
            if diff.lines.is_empty() { Vec::new() } else { vec![diff] }
        }
        "write" => {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.is_empty() && write_before.is_none() {
                return Vec::new();
            }
            let mut diff = FileDiff { path, ..Default::default() };
            diff.append_preview(write_before.unwrap_or(""), content, true);
            vec![diff]
        }
        _ => Vec::new(),
    }
}

fn path_for_prompt(cwd: &Path, file: &Path) -> String {
    if let Ok(rel) = file.strip_prefix(cwd) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    file.to_string_lossy().replace('\\', "/")
}
