//! Native GPUI conversation view; the manager owns project tabs and the shared SDK host.
mod confirmation;
mod diff;
mod rpc;
mod manager;
mod quote;
pub use manager::AiPane;
pub(crate) use rpc::project_key;
pub use quote::SelectionQuote;
use quote::{append_quotes, split_injected_prompt};

use crate::math::{MathDisplaySize, MathReady, MathRenderer};
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
        Scrollbar, ScrollbarMode, StyledExt, TextSelection,
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
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

pub enum AiPaneEvent {
    Annotation(Value),
    TagRequest(Value),
    UndoTags(Vec<crate::tags::agent::TagConfirmation>),
    OpenFile(PathBuf),
    ToggleMaximize,
    Hide,
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
    expanded: bool,
    approval: Option<TagApproval>,
}

#[derive(Clone, Debug)]
enum AssistantPart {
    Thinking {
        text: String,
        collapsed: bool,
        duration_ms: Option<u64>,
        streaming: bool,
    },
    Text(String),
    Tool(ToolBlock),
}

#[derive(Clone)]
enum ChatMessage {
    User {
        display_text: String,
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

/// Navigation targets use immediate scroll-child indices; consecutive assistant messages share one row.
struct MessageAnchor {
    item: usize,
    user: bool,
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
const AI_INPUT_PLACEHOLDER: &str = "随心输入，/ 呼出命令…";

struct ChatPane {
    host: Arc<rpc::Host>,
    runtime_id: String,
    last_sequence: u64,
    cwd: PathBuf,
    project_name: Option<String>,
    settings: AppSettings,
    client: Option<RpcClient>,
    startup_error: Option<String>,
    messages: Vec<ChatMessage>,
    is_streaming: bool,
    is_compacting: bool,
    session_file: Option<PathBuf>,
    session_id: Option<String>,
    session_name: Option<String>,
    available_models: Vec<ModelInfo>,
    current_model: Option<ModelInfo>,
    thinking_levels: Vec<String>,
    current_thinking: String,
    context_usage: Option<ContextUsage>,
    commands: Vec<PiCommandInfo>,
    active_file: Option<AttachedFile>,
    attach_active_file: bool,
    quoted_selections: Vec<SelectionQuote>,
    quote_popover: Option<AnswerQuotePopover>,
    quote_drag: Option<(usize, usize, Point<Pixels>)>,
    quote_bounds: Rc<Cell<Bounds<Pixels>>>,
    pasted_images: Vec<PastedImage>,
    next_image_id: u64,
    auto_approve_shell: bool,
    pending_ui: Option<PendingExtensionUi>,
    pending_ext_prefill: Option<(String, bool)>,
    pending_composer_text: Option<String>,
    ext_input: Entity<InputState>,
    ext_textarea: Entity<TextareaState>,
    ext_statuses: BTreeMap<String, String>,
    ext_widgets: BTreeMap<String, Vec<String>>,
    toast: Option<(String, bool)>,
    toast_task: Task<()>,
    toast_anim_gen: u64,
    nav_hovered: bool,
    hovered_anchor: Option<usize>,
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
    expanded_approval_groups: HashSet<String>,
    toggled_task_processes: HashSet<usize>,
    viewed_diff_files: HashSet<String>,
    active_run_start: Option<std::time::Instant>,
    active_thinking_start: Option<std::time::Instant>,
    write_snapshots: HashMap<String, String>,
    active_part_map: HashMap<usize, usize>,
    input: Entity<TextareaState>,
    _math: Entity<MathRenderer>,
    _mermaid: Entity<MermaidRenderer>,
    md_extensions: MarkdownExtensions,
    scroll: ScrollHandle,
    stick_to_bottom: bool,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl ChatPane {
    pub fn new(
        cwd: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        host: Arc<rpc::Host>,
        runtime_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mermaid = cx.new(MermaidRenderer::new);
        let md_extensions = crate::markdown::markdown_extensions(&math, MathDisplaySize::Ai, cx)
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
        let sub_quote_escape = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() == quote_window
                && event.keystroke.key == "escape"
                && event.keystroke.modifiers.number_of_modifiers() == 0
            {
                let _ = quote_view.update(cx, |pane, cx| {
                    if pane.quote_popover.is_some() {
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
        let model_search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search models..."));
        let ext_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入内容…"));
        let ext_textarea = cx.new(|cx| TextareaState::new(window, cx).placeholder("编辑文本…"));

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

        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(35))
                    .await;
                if this
                    .update(cx, |pane: &mut Self, cx| {
                        pane.poll_rpc(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let pane = Self {
            host,
            runtime_id,
            last_sequence: 0,
            cwd,
            project_name: None,
            settings,
            client: None,
            startup_error: None,
            messages: Vec::new(),
            is_streaming: false,
            is_compacting: false,
            session_file: None,
            session_id: None,
            session_name: None,
            available_models: Vec::new(),
            current_model: None,
            thinking_levels: vec!["off".into(), "low".into(), "medium".into(), "high".into()],
            current_thinking: "high".into(),
            context_usage: None,
            commands: Vec::new(),
            active_file: None,
            attach_active_file: false,
            quoted_selections: Vec::new(),
            quote_popover: None,
            quote_drag: None,
            quote_bounds: Rc::new(Cell::new(Bounds::default())),
            pasted_images: Vec::new(),
            next_image_id: 1,
            auto_approve_shell: false,
            pending_ui: None,
            pending_ext_prefill: None,
            pending_composer_text: None,
            ext_input,
            ext_textarea,
            ext_statuses: BTreeMap::new(),
            ext_widgets: BTreeMap::new(),
            toast: None,
            toast_task: Task::ready(()),
            toast_anim_gen: 0,
            nav_hovered: false,
            hovered_anchor: None,
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
            expanded_approval_groups: HashSet::new(),
            toggled_task_processes: HashSet::new(),
            viewed_diff_files: HashSet::new(),
            active_run_start: None,
            active_thinking_start: None,
            write_snapshots: HashMap::new(),
            active_part_map: HashMap::new(),
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
                sub_quote_escape,
            ],
            _poll: poll,
        };
        pane
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.client.is_none() {
            self.start_rpc(cx);
        }
        self.focus(window, cx);
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
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
                .child("添加到对话"),
        ).with_priority(4).into_any_element())
    }

    fn export_answer(&mut self, markdown: &str, cx: &mut Context<Self>) {
        // This view owns its project's cwd, so exports stay with the source conversation.
        let title: String = self.session_name.as_deref().unwrap_or("AI回答")
            .chars()
            .filter(|ch| !ch.is_control() && !r#"<>:"/\|?*"#.contains(*ch))
            .take(48)
            .collect();
        let title = title.trim_matches([' ', '.']);
        let title = if title.is_empty() { "AI回答" } else { title };
        let filename = format!("{title}-{}.md", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"));
        let path = self.cwd.join(&filename);
        let result = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
            .and_then(|mut file| file.write_all(markdown.as_bytes()));
        match result {
            Ok(()) => {
                self.show_toast(format!("已导出为笔记：{filename}"), false, cx);
                cx.emit(AiPaneEvent::OpenFile(path));
            }
            Err(error) => self.show_toast(format!("导出笔记失败：{error}"), true, cx),
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

    fn start_rpc(&mut self, cx: &mut Context<Self>) {
        self.startup_error = None;
        let client = RpcClient::connect(self.host.clone(), &self.cwd, Some(self.runtime_id.clone()));
        match client.send(serde_json::json!({"type":"snapshot"})) {
            Ok(_) => {
                let _ = client.get_available_models();
                let _ = client.get_available_thinking_levels();
                let _ = client.get_session_stats();
                let _ = client.get_commands();
                self.client = Some(client);
            }
            Err(err) => {
                self.startup_error = Some(err);
            }
        }
        cx.notify();
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

    fn poll_rpc(&mut self, cx: &mut Context<Self>) {
        let mut batch = Vec::new();
        if let Some(client) = &self.client {
            while let Ok(item) = client.inbound.try_recv() {
                batch.push(item);
            }
        }
        if batch.is_empty() {
            return;
        }
        for item in batch {
            if let RpcInbound::Event(ev) | RpcInbound::ExtensionUiRequest(ev) = &item {
                if let Some(sequence) = ev["sequence"].as_u64() {
                    if sequence <= self.last_sequence { continue; }
                    self.last_sequence = sequence;
                }
            }
            self.handle_inbound(item, cx);
        }
        cx.notify();
    }

    fn handle_inbound(&mut self, item: RpcInbound, cx: &mut Context<Self>) {
        match item {
            RpcInbound::Response {
                command,
                success,
                data,
                error,
            } => {
                if !success {
                    if let Some(err) = error {
                        self.messages.push(ChatMessage::Notice {
                            text: format!("{command} 失败：{err}"),
                            is_error: true,
                        });
                    }
                    if command == "prompt" {
                        self.is_streaming = false;
                    }
                    if command == "compact" {
                        self.is_compacting = false;
                    }
                    return;
                }
                let Some(data) = data else {
                    return;
                };
                match command.as_str() {
                    "snapshot" => {
                        // The input is ready while SDK startup runs; an earlier snapshot cannot erase a submitted prompt.
                        let submitted_during_startup = self.is_streaming && data["isStreaming"].as_bool() != Some(true);
                        if !submitted_during_startup { self.apply_messages(&data); }
                        self.apply_state(&data);
                        self.is_streaming |= submitted_during_startup;
                        self.last_sequence = data["sequence"].as_u64().unwrap_or(0);
                    }
                    "get_state" => self.apply_state(&data),
                    "get_available_models" => self.apply_models(&data),
                    "get_available_thinking_levels" => self.apply_thinking_levels(&data),
                    "get_session_stats" => self.apply_stats(&data),
                    "get_commands" => self.apply_commands(&data),
                    "get_messages" => self.apply_messages(&data),
                    "set_model" => {
                        if let Some(m) = parse_model(&data) {
                            self.current_model = Some(m);
                        }
                        if let Some(client) = &self.client {
                            let _ = client.get_available_thinking_levels();
                        }
                    }
                    "compact" => {
                        self.is_compacting = false;
                        self.show_toast("上下文已压缩".into(), false, cx);
                        if let Some(client) = &self.client {
                            let _ = client.get_session_stats();
                        }
                    }
                    _ => {}
                }
            }
            RpcInbound::Event(ev) => self.handle_session_event(&ev, cx),
            RpcInbound::ExtensionUiRequest(req) => self.handle_ext_ui_request(&req, cx),
            RpcInbound::Stderr(line) => {
                if line.to_ascii_lowercase().contains("error") && self.messages.is_empty() {
                    self.startup_error = Some(line);
                }
            }
            RpcInbound::Exited(code) => {
                self.client = None;
                self.is_streaming = false;
                self.pending_ui = None;
                self.messages.push(ChatMessage::Notice {
                    text: format!(
                        "AI 宿主已退出{}",
                        code.map(|c| format!(" (code {c})")).unwrap_or_default()
                    ),
                    is_error: true,
                });
            }
        }
    }

    fn apply_state(&mut self, data: &Value) {
        self.is_streaming = data.get("isStreaming").and_then(|v| v.as_bool()).unwrap_or(false);
        self.is_compacting = data.get("isCompacting").and_then(|v| v.as_bool()).unwrap_or(false);
        self.session_id = data.get("sessionId").and_then(|v| v.as_str()).map(str::to_owned);
        self.session_file = data
            .get("sessionFile")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        self.session_name = data
            .get("sessionName")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if let Some(level) = data.get("thinkingLevel").and_then(|v| v.as_str()) {
            self.current_thinking = level.to_owned();
        }
        if let Some(model_val) = data.get("model")
            && let Some(m) = parse_model(model_val)
        {
            self.current_model = Some(m);
        }
    }

    fn apply_models(&mut self, data: &Value) {
        let Some(arr) = data.get("models").and_then(|v| v.as_array()) else {
            return;
        };
        let mut models: Vec<ModelInfo> = arr.iter().filter_map(parse_model).collect();
        models.sort_by(|a, b| {
            let a_magpie = a.provider == "magpie";
            let b_magpie = b.provider == "magpie";
            b_magpie.cmp(&a_magpie).then_with(|| a.name.cmp(&b.name))
        });
        self.available_models = models;
    }

    fn apply_thinking_levels(&mut self, data: &Value) {
        if let Some(arr) = data.get("levels").and_then(|v| v.as_array()) {
            let levels: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if !levels.is_empty() {
                self.thinking_levels = levels;
            }
        }
    }

    fn apply_commands(&mut self, data: &Value) {
        let Some(arr) = data.get("commands").and_then(|v| v.as_array()) else {
            return;
        };
        self.commands = arr
            .iter()
            .filter_map(|c| {
                let name = c.get("name")?.as_str()?.to_owned();
                let description = c
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let source = c
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("extension")
                    .to_owned();
                Some(PiCommandInfo {
                    name,
                    description,
                    source,
                })
            })
            .collect();
    }

    fn apply_stats(&mut self, data: &Value) {
        if let Some(cu) = data.get("contextUsage") {
            let context_window = cu
                .get("contextWindow")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if context_window > 0 {
                let tokens = cu.get("tokens").and_then(|v| v.as_u64());
                let percent = cu
                    .get("percent")
                    .and_then(|v| v.as_f64())
                    .map(|p| p.round().clamp(0.0, 100.0) as u64)
                    .or_else(|| {
                        tokens.map(|t| {
                            ((t as f64 / context_window as f64) * 100.0)
                                .round()
                                .clamp(0.0, 100.0) as u64
                        })
                    });
                self.context_usage = Some(ContextUsage {
                    tokens,
                    context_window,
                    percent,
                });
            }
        }
    }

    fn apply_messages(&mut self, data: &Value) {
        let Some(arr) = data.get("messages").and_then(|v| v.as_array()) else {
            return;
        };
        self.messages.clear();
        self.collapsed_tool_groups.clear();
        self.expanded_approval_groups.clear();
        self.toggled_task_processes.clear();
        self.viewed_diff_files.clear();
        self.active_run_start = None;
        self.active_thinking_start = None;
        self.active_part_map.clear();
        self.quote_popover = None;
        self.quote_drag = None;
        let mut pending_tool_results = HashMap::new();
        for msg in arr {
            if msg.get("role").and_then(|v| v.as_str()) == Some("toolResult")
                && let Some(id) = msg.get("toolCallId").and_then(|v| v.as_str())
            {
                let text = extract_text_content(msg.get("content"));
                let is_err = msg.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                let patch = msg.pointer("/details/patch").and_then(Value::as_str);
                let approval = msg.get("details").and_then(TagApproval::from_result);
                pending_tool_results.insert(id.to_owned(), (text, is_err, patch, approval));
            }
        }

        for (message_index, msg) in arr.iter().enumerate() {
            match msg.get("role").and_then(|v| v.as_str()) {
                Some("user") => {
                    let raw = extract_text_content(msg.get("content"));
                    let (attached_file, mounted_skills, quotes, display_text) =
                        split_injected_prompt(&raw);
                    self.messages.push(ChatMessage::User {
                        display_text,
                        attached_file,
                        mounted_skills,
                        quotes,
                        images: Vec::new(),
                    });
                }
                Some("assistant") => {
                    // A live snapshot must continue the same content blocks when subsequent deltas arrive.
                    let active = data["partial"].as_bool() == Some(true) && message_index + 1 == arr.len();
                    let mut parts = Vec::new();
                    if let Some(blocks) = msg.get("content").and_then(|v| v.as_array()) {
                        for (content_index, block) in blocks.iter().enumerate() {
                            let part_index = parts.len();
                            match block.get("type").and_then(|v| v.as_str()) {
                                Some("thinking") => {
                                    let t = block
                                        .get("thinking")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_owned();
                                    if !t.is_empty() || active {
                                        parts.push(AssistantPart::Thinking {
                                            text: t,
                                            collapsed: !(active && content_index + 1 == blocks.len()),
                                            duration_ms: None,
                                            streaming: active && content_index + 1 == blocks.len(),
                                        });
                                    }
                                }
                                Some("text") => {
                                    let t = block
                                        .get("text")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_owned();
                                    if !t.is_empty() || active {
                                        parts.push(AssistantPart::Text(t));
                                    }
                                }
                                Some("toolCall") => {
                                    let id = block
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_owned();
                                    let name = block
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .to_owned();
                                    let args = block
                                        .get("arguments")
                                        .cloned()
                                        .unwrap_or(Value::Null);
                                    let done = data["isStreaming"].as_bool() != Some(true) || pending_tool_results.contains_key(&id);
                                    let (output, is_error, patch, approval) = pending_tool_results
                                        .remove(&id)
                                        .unwrap_or_default();
                                    let diffs = build_diffs_for_tool(&name, &args, None, patch);
                                    let summary = summarize_tool_call(&name, &args, &diffs);
                                    parts.push(AssistantPart::Tool(ToolBlock {
                                        id,
                                        name,
                                        args,
                                        summary,
                                        output,
                                        is_error,
                                        done,
                                        duration_ms: None,
                                        diffs,
                                        expanded: false,
                                        approval,
                                    }));
                                }
                                _ => {}
                            }
                            if active && parts.len() > part_index {
                                self.active_part_map.insert(content_index, part_index);
                            }
                        }
                    }
                    let error = msg
                        .get("errorMessage")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    self.messages.push(ChatMessage::Assistant {
                        parts,
                        error,
                        streaming: active,
                        duration_ms: None,
                    });
                }
                _ => {}
            }
        }
        self.scroll_to_bottom();
    }

    fn handle_session_event(&mut self, ev: &Value, cx: &mut Context<Self>) {
        let kind = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "agent_start" => {
                self.is_streaming = true;
                if self.active_run_start.is_none() {
                    self.active_run_start = Some(std::time::Instant::now());
                }
            }
            "agent_settled" => {
                self.is_streaming = false;
                let elapsed = self
                    .active_run_start
                    .take()
                    .map(|t| t.elapsed().as_millis() as u64);
                let think_elapsed = self
                    .active_thinking_start
                    .take()
                    .map(|t| t.elapsed().as_millis() as u64);
                for msg in self.messages.iter_mut().rev() {
                    if let ChatMessage::Assistant {
                        parts,
                        streaming,
                        duration_ms,
                        ..
                    } = msg
                    {
                        *streaming = false;
                        if duration_ms.is_none() {
                            *duration_ms = elapsed;
                        }
                        for p in parts.iter_mut() {
                            if let AssistantPart::Thinking {
                                collapsed,
                                streaming: p_stream,
                                duration_ms: p_dur,
                                ..
                            } = p
                            {
                                *p_stream = false;
                                *collapsed = true;
                                if p_dur.is_none() {
                                    *p_dur = think_elapsed;
                                }
                            } else if let AssistantPart::Tool(block) = p {
                                // Aborting argument generation produces no tool_execution_end event.
                                if !block.done {
                                    block.done = true;
                                    block.is_error = true;
                                    block.summary = "工具调用未完成".into();
                                }
                            }
                        }
                        break;
                    }
                }
                self.pending_ui = None;
                if let Some(client) = &self.client {
                    let _ = client.get_session_stats();
                }
            }
            "thinking_level_changed" => {
                if let Some(lvl) = ev.get("level").and_then(|v| v.as_str()) {
                    self.current_thinking = lvl.to_owned();
                }
            }
            "session_info_changed" => {
                self.session_name = ev
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
            }
            "title_error" => {
                self.show_toast(format!("标题生成失败，可手动重命名：{}", ev["error"].as_str().unwrap_or("请求失败")), true, cx);
            }
            "compaction_start" => {
                self.is_compacting = true;
            }
            "compaction_end" => {
                self.is_compacting = false;
                if let Some(err) = ev.get("errorMessage").and_then(|v| v.as_str()) {
                    self.show_toast(format!("压缩失败：{err}"), true, cx);
                } else {
                    self.show_toast("上下文已压缩".into(), false, cx);
                }
                if let Some(client) = &self.client {
                    let _ = client.get_session_stats();
                }
            }
            "extension_error" => {
                let err = ev
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("扩展运行错误");
                self.show_toast(format!("扩展异常：{err}"), true, cx);
            }
            "message_start" => {
                if let Some(msg) = ev.get("message")
                    && msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
                {
                    self.active_part_map.clear();
                    self.messages.push(ChatMessage::Assistant {
                        parts: Vec::new(),
                        error: None,
                        streaming: true,
                        duration_ms: None,
                    });
                    self.scroll_to_bottom();
                }
            }
            "message_update" => {
                if let Some(ame) = ev.get("assistantMessageEvent") {
                    self.handle_assistant_delta(ame);
                }
            }
            "message_end" => {
                let think_elapsed = self
                    .active_thinking_start
                    .take()
                    .map(|t| t.elapsed().as_millis() as u64);
                if let Some(msg) = ev.get("message")
                    && msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
                    && let Some(ChatMessage::Assistant {
                        parts,
                        error,
                        streaming,
                        ..
                    }) = self.messages.last_mut()
                {
                    *streaming = false;
                    if let Some(err) = msg
                        .get("errorMessage")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        *error = Some(err.to_owned());
                    }
                    for p in parts.iter_mut() {
                        if let AssistantPart::Thinking {
                            collapsed,
                            streaming: p_stream,
                            duration_ms,
                            ..
                        } = p
                        {
                            *p_stream = false;
                            *collapsed = true;
                            if duration_ms.is_none() {
                                *duration_ms = think_elapsed;
                            }
                        }
                    }
                }
                self.scroll_to_bottom();
            }
            "tool_execution_start" => {
                let call_id = ev
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let name = ev
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let args = ev.get("args").cloned().unwrap_or(Value::Null);

                if name == "write"
                    && let Some(old) = ev.get("writeBefore").and_then(|v| v.as_str())
                {
                    // The host captures this before execution; reading here races the actual write.
                    self.write_snapshots.insert(call_id.clone(), old.to_owned());
                }

                self.ensure_tool_block(&call_id, &name, &args);
                self.scroll_to_bottom();
            }
            "tool_execution_update" => {
                let call_id = ev
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let partial = extract_text_content(
                    ev.get("partialResult").and_then(|r| r.get("content")),
                );
                if let Some(block) = self.find_tool_mut(call_id) {
                    block.output = partial;
                }
                self.scroll_to_bottom();
            }
            "tool_execution_end" => {
                let call_id = ev
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let name = ev
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let is_err = ev.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                let duration_ms = ev.get("durationMs").and_then(|v| v.as_u64());
                let output =
                    extract_text_content(ev.get("result").and_then(|r| r.get("content")));
                let write_before = self.write_snapshots.remove(call_id);
                let write_new_fallback = if name == "write" && !is_err {
                    self.find_tool_mut(call_id)
                        .map(|block| (block.name.clone(), block.args.clone()))
                        .map(|(block_name, mut write_args)| {
                            if write_args.get("content").and_then(|v| v.as_str()).is_none()
                                && let Some(rel) = extract_path_arg(&write_args)
                                && let Ok(disk_new) =
                                    std::fs::read_to_string(self.resolve_tool_path(&rel))
                                && let Some(obj) = write_args.as_object_mut()
                            {
                                obj.insert("content".to_owned(), Value::String(disk_new));
                            }
                            let diffs = build_diffs_for_tool(
                                &block_name,
                                &write_args,
                                write_before.as_deref(),
                                None,
                            );
                            (write_args, diffs)
                        })
                } else {
                    None
                };

                if let Some(block) = self.find_tool_mut(call_id) {
                    block.done = true;
                    block.is_error = is_err;
                    block.duration_ms = duration_ms;
                    if let Some(approval) = ev.pointer("/result/details").and_then(TagApproval::from_result) {
                        block.approval = Some(approval);
                    }
                    if !output.is_empty() {
                        block.output = output;
                    }
                    if let Some((write_args, diffs)) = write_new_fallback {
                        block.args = write_args;
                        block.diffs = diffs;
                    }
                    if name == "edit" && !is_err
                        && let Some(patch) = ev.pointer("/result/details/patch").and_then(Value::as_str)
                    {
                        block.diffs = build_diffs_for_tool(&name, &block.args, None, Some(patch));
                    }
                    if let Some(first_diff) = block.diffs.first() {
                        let name_only = Path::new(&first_diff.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| first_diff.path.clone());
                        block.summary = format!(
                            "已编辑 {} +{} -{}",
                            name_only, first_diff.added, first_diff.removed
                        );
                    }
                }
                self.scroll_to_bottom();
            }
            _ => {}
        }
    }

    fn handle_assistant_delta(&mut self, ame: &Value) {
        let ty = ame.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let content_index = ame
            .get("contentIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        if !matches!(self.messages.last(), Some(ChatMessage::Assistant { .. })) {
            self.messages.push(ChatMessage::Assistant {
                parts: Vec::new(),
                error: None,
                streaming: true,
                duration_ms: None,
            });
        }

        match ty {
            "thinking_start" => {
                self.active_thinking_start = Some(std::time::Instant::now());
                let idx = self.push_assistant_part(AssistantPart::Thinking {
                    text: String::new(),
                    collapsed: false,
                    duration_ms: None,
                    streaming: true,
                });
                self.active_part_map.insert(content_index, idx);
            }
            "thinking_delta" => {
                let delta = ame.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if self.active_thinking_start.is_none() {
                    self.active_thinking_start = Some(std::time::Instant::now());
                }
                let idx = self.get_or_create_part(content_index, || AssistantPart::Thinking {
                    text: String::new(),
                    collapsed: false,
                    duration_ms: None,
                    streaming: true,
                });
                if let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Thinking {
                        text, streaming, ..
                    }) = parts.get_mut(idx)
                {
                    text.push_str(delta);
                    *streaming = true;
                }
                self.scroll_to_bottom();
            }
            "thinking_end" => {
                let elapsed = self
                    .active_thinking_start
                    .take()
                    .map(|t| t.elapsed().as_millis() as u64);
                if let Some(&idx) = self.active_part_map.get(&content_index)
                    && let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Thinking {
                        text,
                        collapsed,
                        duration_ms,
                        streaming,
                    }) = parts.get_mut(idx)
                {
                    if let Some(content) = ame.get("content").and_then(|v| v.as_str()) {
                        *text = content.to_owned();
                    }
                    *streaming = false;
                    *collapsed = true;
                    if duration_ms.is_none() {
                        *duration_ms = elapsed;
                    }
                }
                self.scroll_to_bottom();
            }
            "text_start" => {
                let idx = self.push_assistant_part(AssistantPart::Text(String::new()));
                self.active_part_map.insert(content_index, idx);
                self.scroll_to_bottom();
            }
            "text_delta" => {
                let delta = ame.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                let idx = self.get_or_create_part(content_index, || AssistantPart::Text(String::new()));
                if let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Text(text)) = parts.get_mut(idx)
                {
                    text.push_str(delta);
                }
                self.scroll_to_bottom();
            }
            "text_end" => {
                if let Some(content) = ame.get("content").and_then(|v| v.as_str())
                    && let Some(&idx) = self.active_part_map.get(&content_index)
                    && let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Text(text)) = parts.get_mut(idx)
                {
                    *text = content.to_owned();
                }
                self.scroll_to_bottom();
            }
            "toolcall_start" | "toolcall_delta" | "toolcall_end" => {
                // Pi carries live tool arguments in partial.content, not top-level id/toolName fields.
                let tool_call = if ty == "toolcall_end" { ame.get("toolCall") }
                    else { ame.get("partial").and_then(|partial| partial.get("content"))
                        .and_then(|content| content.get(content_index)) };
                if let Some(tc) = tool_call {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                    let name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_owned();
                    let args = tc.get("arguments").cloned().unwrap_or(Value::Null);
                    let diffs = if ty == "toolcall_end" && name == "edit" {
                        build_diffs_for_tool(&name, &args, None, None)
                    } else {
                        Vec::new()
                    };
                    let summary = summarize_tool_call(&name, &args, &diffs);
                    let idx = self.get_or_create_part(content_index, || AssistantPart::Tool(ToolBlock {
                        id: id.clone(), name: name.clone(), args: args.clone(), summary: summary.clone(),
                        output: String::new(), is_error: false, done: false, duration_ms: None,
                        diffs: Vec::new(), expanded: false, approval: None,
                    }));
                    if let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                        && let Some(AssistantPart::Tool(block)) = parts.get_mut(idx)
                    {
                        block.id = id;
                        block.name = name;
                        block.args = args;
                        block.summary = summary;
                        if !diffs.is_empty() {
                            block.diffs = diffs;
                        }
                    }
                }
                self.scroll_to_bottom();
            }
            _ => {}
        }
    }

    fn push_assistant_part(&mut self, part: AssistantPart) -> usize {
        if let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut() {
            parts.push(part);
            return parts.len() - 1;
        }
        0
    }

    fn get_or_create_part<F: FnOnce() -> AssistantPart>(
        &mut self,
        content_index: usize,
        make: F,
    ) -> usize {
        if let Some(&idx) = self.active_part_map.get(&content_index) {
            return idx;
        }
        let idx = self.push_assistant_part(make());
        self.active_part_map.insert(content_index, idx);
        idx
    }

    fn ensure_tool_block(&mut self, call_id: &str, name: &str, args: &Value) {
        let diffs = if name == "edit" {
            build_diffs_for_tool(name, args, None, None)
        } else {
            Vec::new()
        };
        let summary = summarize_tool_call(name, args, &diffs);
        if let Some(block) = self.find_tool_mut(call_id) {
            block.args = args.clone();
            block.summary = summary;
            if !diffs.is_empty() {
                block.diffs = diffs;
            }
            return;
        }
        self.push_assistant_part(AssistantPart::Tool(ToolBlock {
            id: call_id.to_owned(),
            name: name.to_owned(),
            args: args.clone(),
            summary,
            output: String::new(),
            is_error: false,
            done: false,
            duration_ms: None,
            diffs,
            expanded: false,
            approval: None,
        }));
    }

    fn find_tool_mut(&mut self, call_id: &str) -> Option<&mut ToolBlock> {
        for msg in self.messages.iter_mut().rev() {
            if let ChatMessage::Assistant { parts, .. } = msg {
                for part in parts.iter_mut().rev() {
                    if let AssistantPart::Tool(block) = part
                        && block.id == call_id
                    {
                        return Some(block);
                    }
                }
            }
        }
        None
    }

    fn handle_ext_ui_request(&mut self, req: &Value, cx: &mut Context<Self>) {
        let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        match method {
            "confirm" => {
                let title = req
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("请求确认")
                    .to_owned();
                let message = req
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let content = if title.contains("bash") || title.contains("powershell") {
                    ConfirmationContent::Command(message)
                } else {
                    ConfirmationContent::Message(message)
                };
                if matches!(content, ConfirmationContent::Command(_)) && self.auto_approve_shell {
                    if let Some(client) = &self.client {
                        let _ = client.respond_extension_confirm(&id, true);
                    }
                    return;
                }
                self.pending_ui = Some(PendingExtensionUi {
                    id,
                    title,
                    kind: PendingUiKind::Confirm(content),
                });
                self.scroll_to_bottom();
            }
            "select" => {
                let title = req
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("请选择")
                    .to_owned();
                let options = req
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                self.pending_ui = Some(PendingExtensionUi {
                    id,
                    title,
                    kind: PendingUiKind::Select { options },
                });
                self.scroll_to_bottom();
            }
            "input" => {
                let title = req
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("请输入")
                    .to_owned();
                let placeholder = req
                    .get("placeholder")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                self.pending_ui = Some(PendingExtensionUi {
                    id,
                    title,
                    kind: PendingUiKind::TextInput {
                        placeholder,
                        multiline: false,
                    },
                });
                self.pending_ext_prefill = Some((String::new(), false));
                self.scroll_to_bottom();
            }
            "editor" => {
                let title = req
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("编辑内容")
                    .to_owned();
                let prefill = req
                    .get("prefill")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                self.pending_ext_prefill = Some((prefill.clone(), true));
                self.pending_ui = Some(PendingExtensionUi {
                    id,
                    title,
                    kind: PendingUiKind::TextInput {
                        placeholder: prefill,
                        multiline: true,
                    },
                });
                self.scroll_to_bottom();
            }
            "notify" => {
                let message = req
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let is_error = req.get("notifyType").and_then(|v| v.as_str()) == Some("error");
                if !message.is_empty() && is_error {
                    self.show_toast(message, true, cx);
                }
            }
            "setStatus" => {
                if let Some(key) = req.get("statusKey").and_then(|v| v.as_str()) {
                    match req.get("statusText").and_then(|v| v.as_str()) {
                        Some(text) if !text.is_empty() => {
                            self.ext_statuses.insert(key.to_owned(), text.to_owned());
                        }
                        _ => {
                            self.ext_statuses.remove(key);
                        }
                    }
                }
            }
            "setWidget" => {
                if let Some(key) = req.get("widgetKey").and_then(|v| v.as_str()) {
                    match req.get("widgetLines").and_then(|v| v.as_array()) {
                        Some(lines) if !lines.is_empty() => {
                            let collected = lines
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect();
                            self.ext_widgets.insert(key.to_owned(), collected);
                        }
                        _ => {
                            self.ext_widgets.remove(key);
                        }
                    }
                }
            }
            "set_editor_text" => {
                if let Some(text) = req.get("text").and_then(|v| v.as_str()) {
                    self.pending_composer_text = Some(text.to_owned());
                }
            }
            _ => {}
        }
    }

    fn respond_confirm(
        &mut self,
        id: &str,
        confirmed: bool,
        auto_allow_shell: bool,
        cx: &mut Context<Self>,
    ) {
        // A click from an old render must not confirm the next queued request.
        let Some(pending) = self.pending_ui.take_if(|pending| pending.id == id) else {
            return;
        };
        self.auto_approve_shell |= auto_allow_shell;
        if let Some(client) = &self.client {
            let _ = client.respond_extension_confirm(&pending.id, confirmed);
        }
        cx.notify();
    }

    fn respond_select(&mut self, option: String, cx: &mut Context<Self>) {
        if let Some(pending) = self.pending_ui.take()
            && let Some(client) = &self.client
        {
            let _ = client.respond_extension_value(&pending.id, &option);
        }
        cx.notify();
    }

    fn submit_pending_ext_text(&mut self, multiline: bool, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_ui.take() else {
            return;
        };
        let value = if multiline {
            self.ext_textarea.read(cx).value().to_string()
        } else {
            self.ext_input.read(cx).value().to_string()
        };
        if let Some(client) = &self.client {
            let _ = client.respond_extension_value(&pending.id, &value);
        }
        cx.notify();
    }

    fn cancel_pending_ext(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.pending_ui.take()
            && let Some(client) = &self.client
        {
            let _ = client.respond_extension_cancelled(&pending.id);
        }
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
        let pct_label = self
            .context_usage
            .as_ref()
            .and_then(|cu| cu.percent)
            .map(|p| format!("压缩此聊天的上下文（已使用 {p}%）"))
            .unwrap_or_else(|| "压缩此聊天的上下文".to_owned());

        let mut items = Vec::new();
        if let Some(file) = &self.active_file {
            if self.attach_active_file {
                items.push(SlashItem {
                    id: "detach-active-context".into(),
                    title: "取消当前上下文".into(),
                    subtitle: format!("移除 @{}", file.display),
                    icon: IconName::FileText,
                    action: SlashAction::ToggleActiveContext,
                });
            } else {
                items.push(SlashItem {
                    id: "attach-active-context".into(),
                    title: "附加当前上下文".into(),
                    subtitle: format!("附加 @{}", file.display),
                    icon: IconName::FileText,
                    action: SlashAction::ToggleActiveContext,
                });
            }
        }
        items.push(SlashItem {
            id: "compact".into(),
            title: "压缩".into(),
            subtitle: pct_label,
            icon: IconName::CircleDot,
            action: SlashAction::Compact,
        });

        for cmd in &self.commands {
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
                let rel = path_for_prompt(&self.cwd, path);
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
            .set_directory(&self.cwd)
            .set_title("选择附加文件");
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
        if self.is_streaming {
            return;
        }
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

        if self.client.is_none() {
            self.start_rpc(cx);
        }
        let Some(client) = &self.client else {
            return;
        };

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
            let rel = path_for_prompt(&self.cwd, &file.path);
            payload.push('@');
            payload.push_str(&rel);
            payload.push_str("\n\n");
        }
        append_quotes(&mut payload, &quotes);
        payload.push_str(body_trimmed);

        if let Err(err) = client.prompt_with_images(&payload, &rpc_images, body_trimmed) {
            self.show_toast(format!("发送失败：{err}"), true, cx);
            return;
        }

        self.messages.push(ChatMessage::User {
            display_text: body_trimmed.to_owned(),
            attached_file: attached.map(|f| f.display),
            mounted_skills: skills,
            quotes,
            images: msg_images,
        });
        self.quoted_selections.clear();
        self.pasted_images.clear();
        self.quote_popover = None;
        self.input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.is_streaming = true;
        self.active_run_start = Some(std::time::Instant::now());
        self.stick_to_bottom = true;
        self.scroll_to_bottom();
        cx.notify();
    }

    fn abort(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = &self.client {
            let _ = client.abort();
        }
        cx.notify();
    }

    fn compact_context(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = &self.client
            && !self.is_compacting
        {
            self.is_compacting = true;
            let _ = client.compact();
            cx.notify();
        }
    }

    fn resolve_tool_path(&self, raw: &str) -> PathBuf {
        let p = Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }

    fn scroll_to_bottom(&self) {
        if self.stick_to_bottom {
            self.scroll.scroll_to_bottom();
        }
    }

    fn render_message_navigation(
        &self,
        anchors: &[MessageAnchor],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nav_hovered = self.nav_hovered;
        let rail = div()
            .id("ai-nav-rail")
            .h_full()
            .max_h(px(anchors.len() as f32 * 14.))
            .w(px(26.))
            .py_1()
            .px_0p5()
            .rounded(px(8.))
            .flex()
            .flex_col()
            .on_hover(cx.listener(|pane, hovered, _, cx| {
                if pane.nav_hovered != *hovered {
                    pane.nav_hovered = *hovered;
                    if !hovered {
                        pane.hovered_anchor = None;
                    }
                    cx.notify();
                }
            }))
            .children(anchors.iter().enumerate().map(|(index, anchor)| {
                let item = anchor.item;
                let next = anchors.get(index + 1).map_or(usize::MAX, |next| next.item);
                let user = anchor.user;
                let is_hovered = self.hovered_anchor == Some(item);
                let scroll = self.scroll.clone();
                let preview = anchor.preview.clone();
                let label = if user { "你的消息" } else { "AI 回答" };
                let row = div()
                    .id(("ai-message-anchor", item))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |pane, hovered, _, cx| {
                        let next_hover = if *hovered {
                            Some(item)
                        } else if pane.hovered_anchor == Some(item) {
                            None
                        } else {
                            pane.hovered_anchor
                        };
                        if pane.hovered_anchor != next_hover {
                            pane.hovered_anchor = next_hover;
                            cx.notify();
                        }
                    }))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, (), window, _| {
                                // Read during paint so wheel scrolling and scrollbar dragging both update the marker.
                                let top = scroll.top_item();
                                let active = (index == 0 || top >= item) && top < next;
                                let width = if is_hovered { 16. } else { 14. };
                                let color = if active {
                                    fg_alpha(palette.accent, if is_hovered { 0.85 } else { 0.68 })
                                } else if is_hovered {
                                    fg_alpha(palette.text, 0.34)
                                } else if user {
                                    fg_alpha(palette.text, 0.16)
                                } else {
                                    fg_alpha(palette.text, 0.08)
                                };
                                let x = bounds.center().x - px(width / 2.);
                                let y = bounds.center().y - px(1.25);
                                window.paint_quad(
                                    gpui::fill(
                                        gpui::Bounds::new(point(x, y), size(px(width), px(2.5))),
                                        color,
                                    )
                                    .corner_radii(px(1.5)),
                                );
                            },
                        )
                        .size_full(),
                    )
                    .tooltip(move |window, cx| {
                        let preview = preview.clone();
                        Tooltip::element(move |_, _| {
                            div()
                                .w(px(220.))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(gpui_kit::rgb(palette.text))
                                        .child(preview.clone()),
                                )
                        })
                        .p_2()
                        .build(window, cx)
                    })
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.stick_to_bottom = false;
                        pane.scroll.scroll_to_top_of_item(item);
                        cx.notify();
                    }));

                row.with_animation(
                    (
                        "ai-nav-anchor-anim",
                        ((item as u64) << 1) | u64::from(is_hovered),
                    ),
                    Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()),
                    move |el, t| {
                        let alpha = if is_hovered { 0.05 * t } else { 0.05 * (1. - t) };
                        el.bg(fg_alpha(palette.text, alpha))
                    },
                )
            }));

        div()
            .absolute()
            .left_1()
            .top_2()
            .bottom_2()
            .w(px(28.))
            .flex()
            .items_center()
            .child(
                rail.with_animation(
                    ("ai-nav-rail-anim", if nav_hovered { 1u64 } else { 0u64 }),
                    Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                    move |el, t| {
                        let opacity = if nav_hovered {
                            0.72 + 0.28 * t
                        } else {
                            1.0 - 0.28 * t
                        };
                        let bg_alpha = if nav_hovered { 0.03 * t } else { 0.03 * (1. - t) };
                        el.opacity(opacity).bg(fg_alpha(palette.text, bg_alpha))
                    },
                ),
            )
            .into_any_element()
    }

    fn md_style(&self, body_size: Pixels, cx: &App) -> TextViewStyle {
        let base = TextViewStyle::from_theme(&gpui_kit::base::Theme::global(cx));
        let soft_selection = base.selection().alpha(0.3);
        base.with_inline_code_style(crate::ui::inline_code_style(&self.settings, body_size))
            .with_code_block(crate::ui::block_code_style(&self.settings, body_size))
            .with_table(crate::ui::markdown_table_style())
            .with_selection(soft_selection)
    }

    fn render_diff_card(
        &self,
        id: SharedString,
        diff: &FileDiff,
        code_font: &SharedString,
        palette: Palette,
    ) -> AnyElement {
        if let Some(patch) = &diff.patch {
            return DiffPatchView {
                id,
                patch: patch.clone(),
            }
            .into_any_element();
        }
        let add_tint = mix_color(palette.surface, 0x22c55e, 0.14);
        let add_fg = theme_color(0x16a34a);
        let del_tint = mix_color(palette.surface, palette.danger, 0.14);
        let del_fg = gpui_kit::rgb(palette.danger);
        let muted = gpui_kit::rgb(palette.muted);
        let typo = AiTypography::from_settings(&self.settings);

        div()
            .min_w_0()
            .rounded(px(6.))
            .border_1()
            .border_color(gpui_kit::rgb(palette.border))
            .overflow_hidden()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id(id)
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .font_family(code_font.clone())
                    .text_size(typo.sm)
                    .line_height(relative(1.4))
                    .children(diff.lines.iter().take(120).map(|line| {
                        let (number, text, marker, fg, bg) = match line {
                            DiffLine::Context(n, s) => (n, s, "", muted, None),
                            DiffLine::Removed(n, s) => (n, s, "−", del_fg, Some(del_tint)),
                            DiffLine::Added(n, s) => (n, s, "+", add_fg, Some(add_tint)),
                            DiffLine::Ellipsis => {
                                return div().h(px(3.))
                                    .bg(gpui_kit::rgb(palette.border)).into_any_element();
                            }
                        };
                        div()
                            .flex()
                            .min_h(typo.sm * 1.4)
                            .pr_2()
                            .text_color(fg)
                            .when_some(bg, |el, bg| el.bg(bg))
                            .when_some(*number, |el, n| el.child(
                                div().w(typo.sm * 4.6).pr_2().flex_shrink_0()
                                    .text_right().child(n.to_string()),
                            ))
                            .child(div().w_5().flex_shrink_0().text_center().child(marker))
                            .child(div().min_w_0().flex_1().child(text.clone()))
                            .into_any_element()
                    }))
                    .when(diff.lines.len() > 120, |el| el.child(
                        div().px_2().py_1().text_color(muted).child("仅显示前 120 行预览"),
                    )),
            )
            .into_any_element()
    }

    fn render_markdown_block(
        &self,
        id: SharedString,
        text: &str,
        msg_idx: usize,
        part_idx: usize,
        md_style: &TextViewStyle,
        ui_font: &SharedString,
        palette: Palette,
        typo: AiTypography,
        answer: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let normalized = markdown_editor::normalize_math(text);
        let chat = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("ai-quote-source-{msg_idx}-{part_idx}")))
            .on_mouse_down(MouseButton::Left, cx.listener(move |pane, event: &MouseDownEvent, _, _| {
                // Streaming must not scroll the paragraph away while a selection is being made.
                pane.stick_to_bottom = false;
                pane.quote_drag = Some((msg_idx, part_idx, event.position));
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(move |pane, event: &MouseUpEvent, window, cx| {
                let Some((start_msg, start_part, anchor)) = pane.quote_drag.take() else { return; };
                if (start_msg, start_part) != (msg_idx, part_idx) { return; }
                let position = point(anchor.x.min(event.position.x), anchor.y.min(event.position.y));
                let view = cx.entity().downgrade();
                // Read after the window's selection layer has completed the mouse-up gesture.
                window.defer(cx, move |window, cx| {
                    let text = TextSelection::selected_text(window, cx);
                    let text = text.trim();
                    if text.is_empty() { return; }
                    let _ = view.update(cx, |pane, cx| {
                        pane.stick_to_bottom = false;
                        pane.quote_popover = Some(AnswerQuotePopover {
                            quote: SelectionQuote { source: "AI 回答".into(), text: text.to_owned(), answer: Some(answer) },
                            position,
                            scroll_offset: pane.scroll.offset(),
                        });
                        cx.notify();
                    });
                });
            }))
            .font_family(ui_font.clone())
            .text_size(typo.body)
            .line_height(relative(1.6))
            .text_color(gpui_kit::rgb(palette.text))
            .child(
                TextView::markdown(id, normalized)
                    .selectable(true)
                    .markdown_extensions(self.md_extensions.clone())
                    .on_link_click(move |url, event, _, cx| {
                        // Match TextView's activation gestures when overriding its link handler.
                        let activate = match event {
                            ClickEvent::Mouse(click) => {
                                matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
                            }
                            ClickEvent::Keyboard(_) => true,
                            ClickEvent::Touch(click) => !click.long_press,
                        };
                        if !activate {
                            return;
                        }
                        let relative = url.starts_with("./");
                        if !relative && !url.starts_with("file://") {
                            cx.open_url(url);
                            return;
                        }
                        let _ = chat.update(cx, |pane, cx| {
                            // Relative citations belong to the conversation's workspace.
                            let path = if relative {
                                let path = url.split(['#', '?']).next().expect("link has a path");
                                Some(pane.cwd.join(crate::files::percent_decode(path)))
                            } else {
                                url::Url::parse(url).ok().and_then(|uri| uri.to_file_path().ok())
                            };
                            if let Some(path) = path {
                                cx.emit(AiPaneEvent::OpenFile(path));
                            } else {
                                pane.show_toast("无法解析文件引用".into(), true, cx);
                            }
                        });
                    })
                    .style(md_style.clone())
                    .font_family(ui_font.clone())
                    .text_size(typo.body)
                    .code_block_actions(move |block, _, _| {
                        let code = block.code().to_string();
                        let lang = block.lang().filter(|l| !l.is_empty());
                        let span_start = block.span.as_ref().map(|s| s.start).unwrap_or(0);
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when_some(lang, |row, lang| {
                                row.child(
                                    div()
                                        .pl_1p5()
                                        .text_size(typo.xs)
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child(lang),
                                )
                            })
                            .child(ai_copy_button(
                                SharedString::from(format!(
                                    "ai-copy-code-{msg_idx}-{part_idx}-{span_start}"
                                )),
                                "复制代码",
                                "已复制",
                                code,
                            ))
                    })
                    .table_actions(move |table, _, _| {
                        let md = table.markdown.clone();
                        let span_start = table.span.as_ref().map(|s| s.start).unwrap_or(0);
                        div()
                            .w_full()
                            .flex()
                            .justify_end()
                            .child(ai_copy_button(
                                SharedString::from(format!(
                                    "ai-copy-table-{msg_idx}-{part_idx}-{span_start}"
                                )),
                                "复制表格",
                                "已复制",
                                md,
                            ))
                    }),
            )
            .into_any_element()
    }

    fn render_assistant_run(
        &self,
        run_start: usize,
        run_end: usize,
        total_msgs: usize,
        md_style: &TextViewStyle,
        ui_font: &SharedString,
        palette: Palette,
        typo: AiTypography,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ps = ProcessStyle::from_theme(palette, typo);
        let (code_font, _) = crate::theme::tool_typography(&self.settings);

        let mut all_parts: Vec<(usize, usize, &AssistantPart)> = Vec::new();
        let mut run_streaming = false;
        let mut errors: Vec<String> = Vec::new();
        let mut recorded_duration_ms: Option<u64> = None;

        for (m_idx, msg) in self.messages[run_start..run_end].iter().enumerate() {
            let abs_msg_idx = run_start + m_idx;
            if let ChatMessage::Assistant {
                parts,
                error,
                streaming,
                duration_ms,
            } = msg
            {
                if *streaming {
                    run_streaming = true;
                }
                if let Some(err) = error.as_ref().filter(|e| !e.is_empty()) {
                    errors.push(err.clone());
                }
                if let Some(ms) = *duration_ms {
                    recorded_duration_ms = Some(recorded_duration_ms.unwrap_or(0).saturating_add(ms));
                }
                for (p_idx, part) in parts.iter().enumerate() {
                    all_parts.push((abs_msg_idx, p_idx, part));
                }
            }
        }

        let is_last_run = run_end == total_msgs;
        let any_tool_running = all_parts
            .iter()
            .any(|(_, _, p)| matches!(p, AssistantPart::Tool(b) if !b.done));
        let any_thinking_running = all_parts
            .iter()
            .any(|(_, _, p)| matches!(p, AssistantPart::Thinking { streaming, .. } if *streaming));
        let is_task_running =
            run_streaming || any_tool_running || any_thinking_running || (is_last_run && self.is_streaming);

        let run_has_error = !errors.is_empty()
            || all_parts
                .iter()
                .any(|(_, _, p)| matches!(p, AssistantPart::Tool(b) if b.is_error));

        // Only a trailing text block is an answer; tools/thinking after earlier prose must remain in the timeline.
        let final_text_pos = all_parts.last()
            .filter(|(_, _, p)| matches!(p, AssistantPart::Text(t) if !t.trim().is_empty()))
            .map(|_| all_parts.len() - 1);

        enum TimelineNode<'a> {
            Group(Vec<ProcessStep<'a>>),
            Approval(&'a ToolBlock),
            Prose {
                msg_idx: usize,
                part_idx: usize,
                text: &'a str,
            },
        }

        let split_at = final_text_pos.unwrap_or(all_parts.len());
        let process_slice = &all_parts[..split_at];
        let mut timeline: Vec<TimelineNode<'_>> = Vec::new();
        let mut pending_steps: Vec<ProcessStep<'_>> = Vec::new();
        let mut process_step_count = 0usize;
        let mut sum_tool_ms = 0u64;
        let mut sum_think_ms = 0u64;
        let mut tool_step_count = 0usize;
        let mut thinking_step_count = 0usize;

        for &(m_idx, p_idx, part) in process_slice {
            match part {
                AssistantPart::Thinking {
                    text,
                    collapsed,
                    duration_ms,
                    streaming,
                } => {
                    if text.trim().is_empty() && !*streaming {
                        continue;
                    }
                    process_step_count += 1;
                    thinking_step_count += 1;
                    if let Some(ms) = *duration_ms {
                        sum_think_ms = sum_think_ms.saturating_add(ms);
                    }
                    pending_steps.push(ProcessStep::Thinking {
                        msg_idx: m_idx,
                        part_idx: p_idx,
                        text,
                        collapsed: *collapsed,
                        duration_ms: *duration_ms,
                        streaming: *streaming,
                    });
                }
                AssistantPart::Tool(block) if block.approval.is_some() => {
                    if !pending_steps.is_empty() {
                        timeline.push(TimelineNode::Group(std::mem::take(&mut pending_steps)));
                    }
                    timeline.push(TimelineNode::Approval(block));
                }
                AssistantPart::Tool(block) => {
                    process_step_count += 1;
                    tool_step_count += 1;
                    if let Some(ms) = block.duration_ms {
                        sum_tool_ms = sum_tool_ms.saturating_add(ms);
                    }
                    pending_steps.push(ProcessStep::Tool {
                        msg_idx: m_idx,
                        part_idx: p_idx,
                        block,
                    });
                }
                AssistantPart::Text(text) => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    if !pending_steps.is_empty() {
                        timeline.push(TimelineNode::Group(std::mem::take(&mut pending_steps)));
                    }
                    timeline.push(TimelineNode::Prose {
                        msg_idx: m_idx,
                        part_idx: p_idx,
                        text,
                    });
                }
            }
        }
        if !pending_steps.is_empty() {
            timeline.push(TimelineNode::Group(pending_steps));
        }

        // If task finished cleanly and has intermediate steps before final text (or pure process),
        // allow collapsing the entire process area into one sleek single line ("用时 1m 58s").
        let has_process_area = timeline.iter().any(|node| !matches!(node, TimelineNode::Approval(_)));
        let has_timeline = !timeline.is_empty();
        let show_tool_details = self.settings.tool_execution_view == ToolExecutionView::Detailed
            && tool_step_count > 0;
        let collapse_eligible = !is_task_running && !run_has_error && has_process_area;
        // A manual toggle reverses the current mode's expansion default.
        let task_expanded = !collapse_eligible
            || (show_tool_details != self.toggled_task_processes.contains(&run_start));

        let total_elapsed_ms = recorded_duration_ms.or_else(|| {
            if is_last_run && self.is_streaming {
                self.active_run_start
                    .map(|t| t.elapsed().as_millis() as u64)
            } else {
                let summed = sum_tool_ms.saturating_add(sum_think_ms);
                (summed > 0).then_some(summed)
            }
        });

        let task_summary_label = if thinking_step_count == 1
            && tool_step_count == 0
            && timeline.len() == 1
        {
            match total_elapsed_ms {
                Some(ms) => format!("工作了 {}", format_thinking_duration(ms)),
                None => "工作过程".to_owned(),
            }
        } else if let Some(ms) = total_elapsed_ms {
            format!("用时 {}", format_duration_compact(ms))
        } else {
            format!("已完成 {process_step_count} 个步骤")
        };

        let mut rendered_process: Vec<AnyElement> = Vec::new();
        let mut applied_run: Vec<&ToolBlock> = Vec::new();
        let flush_applied = |this: &Self,
                             run: &mut Vec<&ToolBlock>,
                             out: &mut Vec<AnyElement>,
                             cx: &Context<Self>| {
            if run.is_empty() {
                return;
            }
            if run.len() == 1 {
                out.push(this.render_tag_approval(run[0], palette, typo, ui_font, cx));
            } else {
                out.push(this.render_applied_approval_group(run, palette, typo, ui_font, cx));
            }
            run.clear();
        };
        for node in timeline
            .into_iter()
            .filter(|node| task_expanded || matches!(node, TimelineNode::Approval(_)))
        {
            match node {
                TimelineNode::Approval(block)
                    if block.approval.as_ref().is_some_and(|a| {
                        matches!(a.state, ApprovalState::Applied | ApprovalState::Undone)
                    }) =>
                {
                    applied_run.push(block);
                }
                TimelineNode::Approval(block) => {
                    flush_applied(self, &mut applied_run, &mut rendered_process, cx);
                    rendered_process.push(self.render_tag_approval(block, palette, typo, ui_font, cx));
                }
                TimelineNode::Group(steps) => {
                    flush_applied(self, &mut applied_run, &mut rendered_process, cx);
                    rendered_process.push(self.render_process_group(
                        &steps,
                        ui_font,
                        &code_font,
                        palette,
                        ps,
                        cx,
                    ));
                }
                TimelineNode::Prose {
                    msg_idx,
                    part_idx,
                    text,
                } => {
                    flush_applied(self, &mut applied_run, &mut rendered_process, cx);
                    rendered_process.push(div().pt(px(2.)).pb(px(4.)).child(
                        self.render_markdown_block(
                            SharedString::from(format!("ai-assistant-mid-{msg_idx}-{part_idx}")),
                            text,
                            msg_idx,
                            part_idx,
                            md_style,
                            ui_font,
                            palette,
                            typo,
                            run_start,
                            cx,
                        ),
                    ).into_any_element());
                }
            }
        }
        flush_applied(self, &mut applied_run, &mut rendered_process, cx);

        let final_answer_el = final_text_pos.map(|pos| {
            let (m_idx, p_idx, part) = all_parts[pos];
            let text = match part {
                AssistantPart::Text(t) => t.as_str(),
                _ => "",
            };
            self.render_markdown_block(
                SharedString::from(format!("ai-assistant-md-{m_idx}-{p_idx}")),
                text,
                m_idx,
                p_idx,
                md_style,
                ui_font,
                palette,
                typo,
                run_start,
                cx,
            )
        });

        let full_answer = all_parts
            .iter()
            .filter_map(|(_, _, p)| match p {
                AssistantPart::Text(t) if !t.trim().is_empty() => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        div()
            .flex()
            .flex_col()
            .when(has_timeline, |root| {
                root.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(ps.row_gap)
                        .when(collapse_eligible, |col| {
                            col.child(
                                render_process_header(
                                    SharedString::from(format!("ai-task-toggle-{run_start}")),
                                    None,
                                    task_summary_label,
                                    false,
                                    false,
                                    task_expanded,
                                    ps,
                                )
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    if !pane.toggled_task_processes.remove(&run_start) {
                                        pane.toggled_task_processes.insert(run_start);
                                    }
                                    cx.notify();
                                })),
                            )
                        })
                        .children(rendered_process),
                )
            })
            .when_some(final_answer_el, |root, answer| {
                root.child(
                    div()
                        .when(has_timeline, |el| el.mt(ps.section_gap))
                        .child(answer),
                )
            })
            .when(!is_task_running && !full_answer.is_empty(), |root| {
                root.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_start()
                        .pt_1()
                        .child(ai_copy_button(
                            SharedString::from(format!("ai-copy-msg-{run_start}")),
                            "复制回答",
                            "已复制",
                            full_answer.clone(),
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("ai-export-note-{run_start}")))
                                .size(AI_COPY_BUTTON_SIZE)
                                .rounded(AI_COPY_BUTTON_RADIUS)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(fg_alpha(palette.text, 0.55))
                                .hover(move |style| style.bg(fg_alpha(palette.text, 0.06)).text_color(fg_alpha(palette.text, 0.85)))
                                .child(crate::ui::icon(IconName::NotebookPen).w(AI_COPY_ICON_SIZE).h(AI_COPY_ICON_SIZE))
                                .tooltip(|window, cx| Tooltip::new("导出为笔记").build(window, cx))
                                .on_click(cx.listener(move |pane, _, _, cx| pane.export_answer(&full_answer, cx))),
                        ),
                )
            })
            .when(is_task_running && all_parts.is_empty(), |root| {
                root.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .py_1()
                        .min_h(px(24.))
                        .child(
                            crate::ui::icon(IconName::RefreshCw)
                                .w(ps.icon_size)
                                .h(ps.icon_size)
                                .text_color(ps.fg_secondary)
                                .with_animation(
                                    ("ai-stream-spin", run_start),
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    |icon, t| {
                                        icon.rotate(gpui::radians(t * std::f32::consts::TAU))
                                    },
                                ),
                        )
                        .child(
                            div()
                                .text_size(ps.header_size)
                                .text_color(ps.fg_secondary)
                                .with_animation(
                                    ("ai-stream-breathe", run_start),
                                    Animation::new(Duration::from_millis(1200)).repeat(),
                                    |el, t| {
                                        let a = 0.55
                                            + 0.45
                                                * (0.5
                                                    - 0.5
                                                        * (t * std::f32::consts::TAU).cos());
                                        el.opacity(a)
                                    },
                                )
                                .child("思考中…"),
                        ),
                )
            })
            .children(errors.into_iter().map(|err| {
                div()
                    .mt_2()
                    .px_2p5()
                    .py_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ps.error_fg)
                    .text_size(ps.detail_size)
                    .text_color(ps.error_fg)
                    .child(err)
            }))
            .into_any_element()
    }

    fn render_process_group(
        &self,
        steps: &[ProcessStep<'_>],
        ui_font: &SharedString,
        code_font: &SharedString,
        palette: Palette,
        ps: ProcessStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(first) = steps.first() else {
            return div().into_any_element();
        };
        let (first_msg, first_part) = first.ids();
        let group_key = (first_msg, first_part);

        // Single standalone thinking step renders cleanly as one line without double nesting.
        if steps.len() == 1
            && let ProcessStep::Thinking {
                msg_idx,
                part_idx,
                text,
                collapsed,
                duration_ms,
                streaming,
            } = first
        {
            let title = if *streaming {
                "正在思考…".to_owned()
            } else if let Some(ms) = *duration_ms {
                format!("思考了 {}", format_thinking_duration(ms))
            } else {
                "思考过程".to_owned()
            };
            let msg_idx = *msg_idx;
            let part_idx = *part_idx;
            let expanded = !*collapsed || *streaming;
            return div()
                .flex()
                .flex_col()
                .gap(ps.row_gap)
                .child(
                    render_process_header(
                        SharedString::from(format!("ai-think-hdr-{msg_idx}-{part_idx}")),
                        Some(IconName::Brain),
                        title,
                        *streaming,
                        false,
                        expanded,
                        ps,
                    )
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if let Some(ChatMessage::Assistant { parts, .. }) =
                            pane.messages.get_mut(msg_idx)
                            && let Some(AssistantPart::Thinking { collapsed, .. }) =
                                parts.get_mut(part_idx)
                        {
                            *collapsed = !*collapsed;
                            cx.notify();
                        }
                    })),
                )
                .when(expanded && !text.trim().is_empty(), |col| {
                    col.child(
                        div()
                            .ml(px(12.))
                            .pl_3()
                            .border_l_1()
                            .border_color(ps.guide_line)
                            .child(render_thinking_detail(
                                SharedString::from(format!("ai-think-body-{msg_idx}-{part_idx}")),
                                (*text).to_owned(),
                                *streaming,
                                ui_font,
                                ps,
                            )),
                    )
                })
                .into_any_element();
        }

        let any_running = steps.iter().any(|s| match s {
            ProcessStep::Thinking { streaming, .. } => *streaming,
            ProcessStep::Tool { block, .. } => !block.done,
        });
        let any_error = steps.iter().any(|s| match s {
            ProcessStep::Thinking { .. } => false,
            ProcessStep::Tool { block, .. } => block.is_error,
        });
        let group_expanded = !self.collapsed_tool_groups.contains(&group_key);
        let title = process_group_title(steps, any_running);
        let header_icon = process_group_icon(steps);

        div()
            .flex()
            .flex_col()
            .gap(ps.row_gap)
            .child(
                render_process_header(
                    SharedString::from(format!("ai-proc-grp-{first_msg}-{first_part}")),
                    Some(header_icon),
                    title,
                    any_running,
                    any_error,
                    group_expanded,
                    ps,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    if !pane.collapsed_tool_groups.remove(&group_key) {
                        pane.collapsed_tool_groups.insert(group_key);
                    }
                    cx.notify();
                })),
            )
            .when(group_expanded, |col| {
                col.child(
                    div()
                        .ml(px(12.))
                        .pl_3()
                        .border_l_1()
                        .border_color(ps.guide_line)
                        .flex()
                        .flex_col()
                        .gap(ps.row_gap)
                        .children(steps.iter().map(|step| match step {
                            ProcessStep::Thinking {
                                msg_idx,
                                part_idx,
                                text,
                                collapsed,
                                duration_ms,
                                streaming,
                            } => {
                                let msg_idx = *msg_idx;
                                let part_idx = *part_idx;
                                let expanded = !*collapsed || *streaming;
                                let label = if *streaming {
                                    "正在思考…".to_owned()
                                } else if let Some(ms) = *duration_ms {
                                    format!("思考了 {}", format_thinking_duration(ms))
                                } else {
                                    "思考过程".to_owned()
                                };
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(ps.row_gap)
                                    .child(
                                        render_process_header(
                                            SharedString::from(format!(
                                                "ai-think-sub-{msg_idx}-{part_idx}"
                                            )),
                                            Some(IconName::Brain),
                                            label,
                                            *streaming,
                                            false,
                                            expanded,
                                            ps,
                                        )
                                        .on_click(cx.listener(move |pane, _, _, cx| {
                                            if let Some(ChatMessage::Assistant { parts, .. }) =
                                                pane.messages.get_mut(msg_idx)
                                                && let Some(AssistantPart::Thinking {
                                                    collapsed,
                                                    ..
                                                }) = parts.get_mut(part_idx)
                                            {
                                                *collapsed = !*collapsed;
                                                cx.notify();
                                            }
                                        })),
                                    )
                                    .when(expanded && !text.trim().is_empty(), |box_| {
                                        box_.child(render_thinking_detail(
                                            SharedString::from(format!(
                                                "ai-think-subbody-{msg_idx}-{part_idx}"
                                            )),
                                            (*text).to_owned(),
                                            *streaming,
                                            ui_font,
                                            ps,
                                        ))
                                    })
                                    .into_any_element()
                            }
                            ProcessStep::Tool {
                                msg_idx,
                                part_idx,
                                block,
                            } => self.render_tool_step_detail(
                                *msg_idx,
                                *part_idx,
                                block,
                                ui_font,
                                code_font,
                                palette,
                                ps,
                                cx,
                            ),
                        })),
                )
            })
            .into_any_element()
    }

    fn render_tool_step_detail(
        &self,
        msg_idx: usize,
        part_idx: usize,
        block: &ToolBlock,
        ui_font: &SharedString,
        code_font: &SharedString,
        palette: Palette,
        ps: ProcessStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_file_edit = block.name == "edit" || block.name == "write";
        let is_cmd = block.name == "bash" || block.name == "powershell";
        let mode = self.settings.tool_execution_view;
        let icon = if is_cmd {
            IconName::Terminal
        } else if is_file_edit {
            IconName::PencilLine
        } else if block.name == "read" {
            IconName::FileText
        } else {
            IconName::Wrench
        };
        let row_id = SharedString::from(format!("ai-cmd-row-{msg_idx}-{part_idx}"));
        if mode == ToolExecutionView::Minimal {
            let text = match (block.name.as_str(), block.done, block.is_error) {
                ("edit" | "write", _, true) => "编辑文件失败".into(),
                ("edit" | "write", true, false) => "编辑了文件".into(),
                ("edit" | "write", false, false) => "正在编辑文件".into(),
                ("bash" | "powershell", _, true) => "命令执行失败".into(),
                ("bash" | "powershell", true, false) => "执行了命令".into(),
                ("bash" | "powershell", false, false) => "正在执行命令".into(),
                (name, _, true) => format!("{name} 执行失败"),
                (name, true, false) => format!("已执行 {name}"),
                (name, false, false) => format!("正在执行 {name}"),
            };
            return render_command_detail_row(
                row_id, icon, text, block.done, block.is_error, None, ui_font, code_font, ps,
            ).into_any_element();
        }

        // The two payloads belong to the tool call, even when its summary is a file diff.
        let details = (mode == ToolExecutionView::Detailed
            && (!block.args.is_null() || !block.output.is_empty())).then(|| {
            div().flex().flex_col().gap(ps.row_gap).children(
                [
                    ("input", "输入参数", (!block.args.is_null()).then(|| {
                        serde_json::to_string_pretty(&block.args).expect("tool arguments are JSON")
                    }), ps.fg_secondary),
                    ("output", "输出", (!block.output.is_empty()).then(|| block.output.clone()),
                        if block.is_error { ps.error_fg } else { ps.fg_secondary }),
                ].into_iter().filter_map(|(kind, title, text, color)| {
                    let text = text?;
                    Some(div().flex().flex_col().gap_1()
                        .child(div().text_size(ps.meta_size).text_color(ps.fg_tertiary).child(title))
                        .child(div().on_scroll(|_, _, cx| cx.stop_propagation()).child(
                            div().id(SharedString::from(format!("tool-{kind}-{msg_idx}-{part_idx}")))
                                .p_2().max_h(px(180.)).overflow_y_scroll().rounded(px(6.))
                                .bg(gpui_kit::rgb(palette.code_background))
                                .font_family(code_font.clone()).text_size(ps.detail_size)
                                .text_color(color).child(text),
                        )))
                }),
            )
        });

        if is_file_edit && !block.diffs.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap(ps.row_gap)
                .children(block.diffs.iter().enumerate().map(|(diff_idx, diff)| {
                    let target = self.resolve_tool_path(&diff.path);
                    let diff_key = format!("{msg_idx}:{part_idx}:{diff_idx}:{}", diff.path);
                    let unviewed = !self.viewed_diff_files.contains(&diff_key);
                    let is_created = block.name == "write" && diff.removed == 0 && diff.added > 0;
                    let row = render_file_edit_row(
                        SharedString::from(format!(
                            "ai-file-row-{msg_idx}-{part_idx}-{diff_idx}"
                        )),
                        diff,
                        is_created,
                        block.is_error,
                        unviewed,
                        code_font,
                        palette,
                        ps,
                        cx.listener({
                            let diff_key = diff_key.clone();
                            let target = target.clone();
                            move |pane, _, _, cx| {
                                pane.viewed_diff_files.insert(diff_key.clone());
                                cx.emit(AiPaneEvent::OpenFile(target.clone()));
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                        cx.listener({
                            let diff_key = diff_key.clone();
                            move |pane, _, _, cx| {
                                pane.viewed_diff_files.insert(diff_key.clone());
                                if let Some(ChatMessage::Assistant { parts, .. }) =
                                    pane.messages.get_mut(msg_idx)
                                    && let Some(AssistantPart::Tool(b)) = parts.get_mut(part_idx)
                                {
                                    b.expanded = !b.expanded;
                                }
                                cx.notify();
                            }
                        }),
                    );
                    div().min_w_0().flex().flex_col().child(row).when(
                        block.expanded && (!diff.lines.is_empty()
                            || diff.patch.as_ref().is_some_and(|patch| !patch.line_numbers.is_empty())),
                        |el| el.child(self.render_diff_card(
                            format!("ai-diff-{msg_idx}-{part_idx}-{diff_idx}-{}", block.id).into(),
                            diff,
                            code_font,
                            palette,
                        )),
                    )
                }))
                .children(details)
                .into_any_element();
        }

        let text = if is_cmd {
            let raw = block
                .args
                .get("command")
                .and_then(|v| v.as_str())
                .map(|c| c.lines().next().unwrap_or(c).trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| block.summary.clone());
            if block.is_error {
                format!("Command failed ({raw})")
            } else {
                raw
            }
        } else {
            block.summary.clone()
        };

        let detail_font = if is_cmd { code_font } else { ui_font };
        let row = render_command_detail_row(
            row_id.clone(),
            icon,
            text,
            block.done,
            block.is_error,
            block.duration_ms,
            detail_font,
            code_font,
            ps,
        )
        .with_animation(
            (row_id, 2usize),
            Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).top(px(4.0 * (1.0 - t))),
        );

        div()
            .flex()
            .flex_col()
            .gap(ps.row_gap)
            .child(row)
            .children(details)
            .into_any_element()
    }

    fn render_pending_ui(
        &self,
        pending: &PendingExtensionUi,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        let warn_bg = mix_color(palette.surface, palette.accent, 0.08);
        let card = || div()
            .mx_3()
            .mb_2()
            .p_2p5()
            .rounded(px(8.))
            .border_1()
            .border_color(gpui_kit::rgb(palette.accent))
            .bg(warn_bg)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(typo.sm)
                    .font_semibold()
                    .text_color(gpui_kit::rgb(palette.text))
                    .child(crate::ui::icon(IconName::ShieldAlert).w(px(13.)).h(px(13.)))
                    .child(pending.title.clone()),
            );

        let card = match &pending.kind {
            PendingUiKind::Confirm(content) => {
                return self.render_confirmation(pending, content, palette, typo, cx);
            }
            PendingUiKind::Select { options } => card().child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1p5()
                        .children(options.iter().enumerate().map(|(idx, opt)| {
                            let choice = opt.clone();
                            Button::new(("ext-opt", idx))
                                .ghost()
                                .compact()
                                .label(opt.clone())
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    pane.respond_select(choice.clone(), cx);
                                }))
                        }))
                        .child(
                            Button::new("ext-select-cancel")
                                .ghost()
                                .compact()
                                .label("取消")
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.cancel_pending_ext(cx);
                                })),
                        ),
                ),
            PendingUiKind::TextInput {
                placeholder,
                multiline,
            } => {
                let is_multi = *multiline;
                card()
                    .when(!placeholder.is_empty() && is_multi, |c| {
                        c.child(
                            div()
                                .text_size(typo.sm)
                                .text_color(gpui_kit::rgb(palette.muted))
                                .child(placeholder.clone()),
                        )
                    })
                    .child(if is_multi {
                        Textarea::new(&self.ext_textarea).h(px(80.)).into_any_element()
                    } else {
                        Input::new(&self.ext_input).into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1p5()
                            .child(
                                Button::new("ext-input-cancel")
                                    .ghost()
                                    .compact()
                                    .label("取消")
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.cancel_pending_ext(cx);
                                    })),
                            )
                            .child(
                                Button::new("ext-input-submit")
                                    .primary()
                                    .compact()
                                    .label("提交")
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        pane.submit_pending_ext_text(is_multi, cx);
                                    })),
                            ),
                    )
            }
        };
        card.into_any_element()
    }

    fn render_context_ring(&self, palette: Palette) -> AnyElement {
        let pct = self
            .context_usage
            .as_ref()
            .and_then(|cu| cu.percent)
            .unwrap_or(0)
            .min(100);
        let tip = match &self.context_usage {
            Some(cu) => {
                let used = cu.tokens.unwrap_or(0) / 1000;
                let total = cu.context_window / 1000;
                let rem = 100u64.saturating_sub(pct);
                format!("上下文窗口：{pct}% 已用（剩余 {rem}%）· {used}k / {total}k")
            }
            None => "上下文窗口：尚无统计".to_owned(),
        };

        let track_color: Rgba = gpui_kit::rgb(palette.border);
        let arc_color: Rgba = if pct >= 85 {
            gpui_kit::rgb(palette.danger)
        } else {
            gpui_kit::rgb(palette.accent)
        };
        let fraction = match &self.context_usage {
            Some(cu) if cu.context_window > 0 => cu
                .tokens
                .map(|t| (t as f32 / cu.context_window as f32).clamp(0.0, 1.0))
                .unwrap_or_else(|| (pct as f32 / 100.0).clamp(0.0, 1.0)),
            _ => (pct as f32 / 100.0).clamp(0.0, 1.0),
        };

        div()
            .id("ai-context-ring")
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        let cx_pos = bounds.center().x;
                        let cy_pos = bounds.center().y;
                        let r = (bounds.size.width.min(bounds.size.height) * 0.5) - px(2.5);
                        if r <= px(1.) {
                            return;
                        }
                        let segments = 28usize;
                        let mut track = gpui_kit::PathBuilder::stroke(px(1.8));
                        for i in 0..=segments {
                            let angle = -std::f32::consts::FRAC_PI_2
                                + (i as f32 / segments as f32) * std::f32::consts::TAU;
                            let pt = point(cx_pos + r * angle.cos(), cy_pos + r * angle.sin());
                            if i == 0 {
                                track.move_to(pt);
                            } else {
                                track.line_to(pt);
                            }
                        }
                        if let Ok(path) = track.build() {
                            window.paint_path(path, track_color);
                        }

                        if fraction > 0.0 {
                            let active_steps = ((segments as f32) * fraction).ceil() as usize;
                            let mut arc = gpui_kit::PathBuilder::stroke(px(1.8));
                            for i in 0..=active_steps.max(1) {
                                let t = (i as f32 / segments as f32).min(fraction);
                                let angle =
                                    -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
                                let pt =
                                    point(cx_pos + r * angle.cos(), cy_pos + r * angle.sin());
                                if i == 0 {
                                    arc.move_to(pt);
                                } else {
                                    arc.line_to(pt);
                                }
                            }
                            if let Ok(path) = arc.build() {
                                window.paint_path(path, arc_color);
                            }
                        }
                    },
                )
                .size(px(16.)),
            )
            .into_any_element()
    }

    fn render_slash_popover(
        &self,
        items: Vec<SlashItem>,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = items.len();
        let active_idx = self.slash_index.min(count.saturating_sub(1));
        let typo = AiTypography::from_settings(&self.settings);
        let sel_bg = mix_color(
            palette.hover,
            palette.accent,
            if palette.is_dark() { 0.14 } else { 0.08 },
        );
        div()
            .id("ai-slash-popover")
            .track_scroll(&self.slash_scroll)
            .w_full()
            .max_h(px(268.))
            .overflow_y_scroll()
            .rounded(px(12.))
            .border_1()
            .border_color(subtle_border(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font)
            .p_1p5()
            .flex()
            .flex_col()
            .gap_0p5()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                let selected = idx == active_idx;
                let item_click = item.clone();
                let icon_box_bg = if selected {
                    mix_color(
                        palette.surface,
                        palette.accent,
                        if palette.is_dark() { 0.22 } else { 0.12 },
                    )
                } else {
                    gpui_kit::rgb(palette.background)
                };
                div()
                    .id(("ai-slash-item", idx))
                    .px_2()
                    .py_1p5()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .when(selected, |row| row.bg(sel_bg))
                    .when(!selected, |row| {
                        row.hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                    })
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.execute_slash_item(item_click.clone(), window, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .size(px(22.))
                                    .rounded(px(6.))
                                    .flex_shrink_0()
                                    .bg(icon_box_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        crate::ui::icon(item.icon)
                                            .w(px(12.))
                                            .h(px(12.))
                                            .text_color(gpui_kit::rgb(if selected {
                                                palette.accent
                                            } else {
                                                palette.muted
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(typo.sm)
                                    .font_medium()
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .flex_shrink_0()
                                    .child(item.title),
                            )
                            .child(
                                div()
                                    .text_size(typo.sm)
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .text_ellipsis()
                                    .child(item.subtitle),
                            ),
                    )
            }))
            .with_animation(
                ("ai-slash-pop", self.slash_anim_gen),
                Animation::new(Duration::from_millis(145)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mb(px((1. - t) * -6.)),
            )
            .into_any_element()
    }

    fn render_model_popover(
        &self,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        let search_q = self
            .model_search_input
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        let current_id = self.current_model.as_ref().map(|m| m.id.as_str());
        let supports_thinking = self.current_model.as_ref().is_none_or(|m| m.reasoning);

        let mut models: Vec<(bool, ModelInfo)> = self
            .available_models
            .iter()
            .filter(|m| {
                search_q.is_empty()
                    || m.name.to_ascii_lowercase().contains(&search_q)
                    || m.id.to_ascii_lowercase().contains(&search_q)
                    || m.provider.to_ascii_lowercase().contains(&search_q)
            })
            .cloned()
            .map(|m| {
                let key = format!("{}/{}", m.provider, m.id);
                let fav = self.favorite_models.contains(&key);
                (fav, m)
            })
            .collect();
        models.sort_by_key(|(fav, _)| !*fav);

        let sel_bg = mix_color(
            palette.hover,
            palette.accent,
            if palette.is_dark() { 0.16 } else { 0.09 },
        );

        div()
            .id("ai-model-popover")
            .w_full()
            .h(px(292.))
            .rounded(px(12.))
            .border_1()
            .border_color(subtle_border(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font.clone())
            .overflow_hidden()
            .flex()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .border_r_1()
                    .border_color(subtle_border(palette.border))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_2p5()
                            .py_1()
                            .border_b_1()
                            .border_color(subtle_border(palette.border))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .flex_shrink_0()
                            .child(
                                crate::ui::icon(IconName::Search)
                                    .w(px(12.))
                                    .h(px(12.))
                                    .text_color(gpui_kit::rgb(palette.muted)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(typo.sm)
                                    .child(
                                        Input::new(&self.model_search_input)
                                            .xsmall()
                                            .h(px(20.))
                                            .appearance(false)
                                            .bordered(false)
                                            .font_family(ui_font.clone())
                                            .text_size(typo.sm),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("ai-model-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_1p5()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .when(models.is_empty(), |list| {
                                list.child(
                                    div()
                                        .px_2p5()
                                        .py_4()
                                        .text_size(typo.sm)
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child(if self.available_models.is_empty() {
                                            "暂无可用模型"
                                        } else {
                                            "无匹配模型"
                                        }),
                                )
                            })
                            .children(models.into_iter().enumerate().map(|(idx, (fav, m))| {
                                let selected = current_id == Some(m.id.as_str());
                                let provider = m.provider.clone();
                                let model_id = m.id.clone();
                                let fav_key = format!("{}/{}", m.provider, m.id);
                                div()
                                    .id(("ai-model-row", idx))
                                    .px_2()
                                    .py_1p5()
                                    .rounded(px(7.))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_1p5()
                                    .when(selected, |r| r.bg(sel_bg))
                                    .when(!selected, |r| {
                                        r.hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                    })
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        if let Some(client) = &pane.client {
                                            let _ = client.set_model(&provider, &model_id);
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .min_w_0()
                                            .flex_1()
                                            .child(render_model_icon(
                                                Some(&m),
                                                px(13.),
                                                if selected {
                                                    palette.accent
                                                } else {
                                                    palette.muted
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .text_size(typo.sm)
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .text_ellipsis()
                                                    .child(m.name),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .flex_shrink_0()
                                            .when(selected, |r| {
                                                r.child(
                                                    crate::ui::icon(IconName::Check)
                                                        .w(px(12.))
                                                        .h(px(12.))
                                                        .text_color(gpui_kit::rgb(palette.accent)),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .id(("ai-model-fav", idx))
                                                    .size(px(20.))
                                                    .rounded(px(5.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_color(gpui_kit::rgb(if fav {
                                                        palette.accent
                                                    } else {
                                                        palette.muted
                                                    }))
                                                    .opacity(if fav { 1.0 } else { 0.45 })
                                                    .hover(move |s| {
                                                        s.opacity(1.0)
                                                            .bg(gpui_kit::rgb(palette.pressed))
                                                    })
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                                        pane.toggle_favorite_model(&fav_key, cx);
                                                        cx.stop_propagation();
                                                    }))
                                                    .child(
                                                        crate::ui::icon(IconName::Star)
                                                            .w(px(11.))
                                                            .h(px(11.)),
                                                    ),
                                            ),
                                    )
                            })),
                    ),
            )
            .child(
                div()
                    .w(px(136.))
                    .flex_shrink_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_2p5()
                            .py_1p5()
                            .border_b_1()
                            .border_color(subtle_border(palette.border))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .text_size(typo.xs)
                                    .font_semibold()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child("Effort"),
                            ),
                    )
                    .child(
                        div()
                            .id("ai-think-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_1p5()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .when(!supports_thinking, |col| {
                                col.child(
                                    div()
                                        .px_2()
                                        .py_2()
                                        .text_size(typo.xs)
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child("当前模型不支持调节"),
                                )
                            })
                            .when(supports_thinking, |col| {
                                col.children(self.thinking_levels.iter().enumerate().map(
                                    |(idx, lvl)| {
                                        let checked = self.current_thinking == *lvl;
                                        let lvl_set = lvl.clone();
                                        div()
                                            .id(("ai-think-row", idx))
                                            .px_2()
                                            .py_1p5()
                                            .rounded(px(7.))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_1p5()
                                            .when(checked, |r| r.bg(sel_bg))
                                            .when(!checked, |r| {
                                                r.hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                            })
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                pane.current_thinking = lvl_set.clone();
                                                if let Some(client) = &pane.client {
                                                    let _ = client.set_thinking_level(&lvl_set);
                                                }
                                                pane.model_popover_open = false;
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .text_size(typo.sm)
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .child(thinking_short_label(lvl)),
                                            )
                                            .when(checked, |r| {
                                                r.child(
                                                    crate::ui::icon(IconName::Check)
                                                        .w(px(12.))
                                                        .h(px(12.))
                                                        .text_color(gpui_kit::rgb(palette.accent)),
                                                )
                                            })
                                    },
                                ))
                            }),
                    ),
            )
            .with_animation(
                ("ai-model-pop", self.model_popover_anim_gen),
                Animation::new(Duration::from_millis(145)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mb(px((1. - t) * -6.)),
            )
            .into_any_element()
    }

    fn render_toast_popover(
        &self,
        msg: String,
        is_err: bool,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        div()
            .id("ai-toast-popover")
            .w_full()
            .px_3()
            .py_2()
            .rounded(px(10.))
            .border_1()
            .border_color(if is_err {
                gpui_kit::rgb(palette.danger)
            } else {
                subtle_border(palette.border)
            })
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .text_size(typo.sm)
            .text_color(gpui_kit::rgb(if is_err {
                palette.danger
            } else {
                palette.text
            }))
            .child(div().min_w_0().text_ellipsis().child(msg))
            .child(
                div()
                    .id("ai-toast-close")
                    .size(px(18.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(gpui_kit::rgb(palette.muted))
                    .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.toast = None;
                        cx.notify();
                    }))
                    .child(crate::ui::icon(IconName::Close).w(px(11.)).h(px(11.))),
            )
            .with_animation(
                ("ai-toast-pop", self.toast_anim_gen),
                Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mb(px((1. - t) * -6.)),
            )
            .into_any_element()
    }

    fn render_welcome(
        &self,
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title_el = if let Some(name) = self
            .project_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            div()
                .mt(AI_WELCOME_ICON_TITLE_GAP)
                .max_w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(22.))
                .font_medium()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(fg_alpha(palette.text, 0.72))
                        .child("你想在 "),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .font_semibold()
                        .text_color(gpui_kit::rgb(palette.text))
                        .child(name.to_owned()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(fg_alpha(palette.text, 0.72))
                        .child(" 里做什么？"),
                )
                .into_any_element()
        } else {
            div()
                .mt(AI_WELCOME_ICON_TITLE_GAP)
                .max_w_full()
                .text_center()
                .text_size(px(22.))
                .font_medium()
                .text_color(gpui_kit::rgb(palette.text))
                .child("今天想做点什么？")
                .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px_3()
            .flex()
            .flex_col()
            .items_center()
            .child(div().flex_grow(2.0).flex_basis(px(0.)))
            .child(
                div()
                    .w_full()
                    .max_w(AI_COMPOSER_MAX_WIDTH)
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        img("brand/ai-mascot.png")
                            .size(AI_WELCOME_ICON_SIZE)
                            .object_fit(ObjectFit::Contain)
                            .flex_shrink_0(),
                    )
                    .child(title_el)
                    .child(
                        div()
                            .w_full()
                            .mt(px(22.))
                            .child(self.render_composer(palette, typo, ui_font, window, cx)),
                    ),
            )
            .child(div().flex_grow(3.0).flex_basis(px(0.)))
            .into_any_element()
    }

    fn render_composer(
        &self,
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model_label = self
            .current_model
            .as_ref()
            .map(|m| short_model_name(&m.name))
            .unwrap_or_else(|| "模型".to_owned());
        let selected_model = self.current_model.as_ref();
        let supports_thinking = self.current_model.as_ref().is_none_or(|m| m.reasoning);
        let attached_chip = self.effective_attached_file().cloned();
        let has_chips = attached_chip.is_some()
            || !self.quoted_selections.is_empty()
            || !self.pasted_images.is_empty();
        let slash_items = self.current_slash_items(cx);
        let has_slash_items = !slash_items.is_empty();
        let input_empty = self.input.read(cx).value().is_empty();
        let can_send = !self.input.read(cx).value().trim().is_empty()
            || !self.quoted_selections.is_empty()
            || !self.pasted_images.is_empty();
        let pane_paste = cx.entity();

        let subtle_btn = |cx: &App| {
            ButtonCustomVariant::new(cx)
                .foreground(gpui_kit::rgb(palette.muted).into())
                .hover(gpui_kit::rgb(palette.hover).into())
                .active(gpui_kit::rgb(palette.pressed).into())
        };
        let quote_bg = mix_color(
            palette.surface,
            palette.accent,
            if palette.is_dark() { 0.15 } else { 0.08 },
        );
        let skill_token_bg = mix_color(
            palette.surface,
            palette.accent,
            if palette.is_dark() { 0.22 } else { 0.11 },
        );
        let skill_token_border = mix_color(
            palette.border,
            palette.accent,
            if palette.is_dark() { 0.45 } else { 0.35 },
        );
        div()
            .relative()
            .w_full()
            .capture_action::<MoveUp>(cx.listener(|pane, _: &MoveUp, _, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let count = items.len();
                    pane.slash_index = (pane.slash_index.min(count - 1) + count - 1) % count;
                    pane.slash_scroll.scroll_to_item(pane.slash_index);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action::<MoveDown>(cx.listener(|pane, _: &MoveDown, _, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let count = items.len();
                    pane.slash_index = (pane.slash_index.min(count - 1) + 1) % count;
                    pane.slash_scroll.scroll_to_item(pane.slash_index);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action::<IndentInline>(cx.listener(|pane, _: &IndentInline, window, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let chosen = items[pane.slash_index.min(items.len() - 1)].clone();
                    cx.stop_propagation();
                    pane.execute_slash_item(chosen, window, cx);
                }
            }))
            .capture_action::<Enter>(cx.listener(|pane, ev: &Enter, window, cx| {
                if ev.shift || ev.secondary {
                    return;
                }
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let chosen = items[pane.slash_index.min(items.len() - 1)].clone();
                    cx.stop_propagation();
                    pane.execute_slash_item(chosen, window, cx);
                }
            }))
            .capture_action::<Escape>(cx.listener(|pane, _: &Escape, _, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    pane.dismissed_slash_text = Some(pane.input.read(cx).value().to_string());
                    pane.slash_was_open = false;
                    cx.stop_propagation();
                    cx.notify();
                } else if pane.model_popover_open {
                    pane.model_popover_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .when(
                has_slash_items || self.model_popover_open || self.toast.is_some(),
                |wrap| {
                    let toast_item = self.toast.clone();
                    wrap.child(
                        deferred(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_full()
                                .mb_2()
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .when_some(toast_item, |col, (msg, is_err)| {
                                    col.child(self.render_toast_popover(
                                        msg,
                                        is_err,
                                        palette,
                                        ui_font.clone(),
                                        cx,
                                    ))
                                })
                                .when(self.model_popover_open && !has_slash_items, |col| {
                                    col.child(self.render_model_popover(
                                        palette,
                                        ui_font.clone(),
                                        cx,
                                    ))
                                })
                                .when(has_slash_items, |col| {
                                    col.child(self.render_slash_popover(
                                        slash_items,
                                        palette,
                                        ui_font.clone(),
                                        cx,
                                    ))
                                }),
                        )
                        .with_priority(3),
                    )
                },
            )
            .child(
                confirmation::conversation_surface(palette)
                    .px_3()
                    .pt_2()
                    .pb_1p5()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(has_chips, |card| {
                        card.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_1()
                                .when_some(attached_chip, |row, file| {
                                    let open_path = file.path.clone();
                                    row.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .px_2()
                                            .py(px(2.))
                                            .rounded(px(6.))
                                            .bg(gpui_kit::rgb(palette.background))
                                            .border_1()
                                            .border_color(subtle_border(palette.border))
                                            .text_size(typo.sm)
                                            .child(
                                                div()
                                                    .id("ai-attached-file")
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |_, _, _, cx| {
                                                        cx.emit(AiPaneEvent::OpenFile(
                                                            open_path.clone(),
                                                        ));
                                                    }))
                                                    .child(
                                                        crate::ui::icon(IconName::FileText)
                                                            .w(px(11.))
                                                            .h(px(11.))
                                                            .text_color(gpui_kit::rgb(
                                                                palette.accent,
                                                            )),
                                                    )
                                                    .child(format!("@{}", file.display)),
                                            )
                                            .child(
                                                div()
                                                    .id("ai-detach-file")
                                                    .cursor_pointer()
                                                    .text_color(gpui_kit::rgb(palette.muted))
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.attach_active_file = false;
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        crate::ui::icon(IconName::Close)
                                                            .w(px(11.))
                                                            .h(px(11.)),
                                                    ),
                                            ),
                                    )
                                })
                                .children(self.quoted_selections.iter().enumerate().map(|(index, q)| {
                                    let tip = format!("{}\n{}", q.source, q.text);
                                        div()
                                            .id(SharedString::from(format!("ai-quote-{index}")))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .max_w(px(260.))
                                            .px_2()
                                            .py(px(2.))
                                            .rounded(px(6.))
                                            .bg(quote_bg)
                                            .border_1()
                                            .border_color(skill_token_border)
                                            .text_size(typo.sm)
                                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_ellipsis()
                                                    .child(format!(
                                                        "引用 {}: {}",
                                                        q.source,
                                                        q.text.replace('\n', " ")
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!("ai-clear-quote-{index}")))
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                                        pane.quoted_selections.remove(index);
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        crate::ui::icon(IconName::Close)
                                                            .w(px(11.))
                                                            .h(px(11.)),
                                                    ),
                                            )
                                }))
                                .children(self.pasted_images.iter().enumerate().map(|(idx, p)| {
                                    let rm_id = p.id;
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .pl_1()
                                        .pr_1p5()
                                        .py(px(2.))
                                        .rounded(px(7.))
                                        .bg(gpui_kit::rgb(palette.background))
                                        .border_1()
                                        .border_color(subtle_border(palette.border))
                                        .text_size(typo.xs)
                                        .child(
                                            img(p.image.clone())
                                                .w(px(24.))
                                                .h(px(24.))
                                                .rounded(px(5.))
                                                .object_fit(ObjectFit::Cover),
                                        )
                                        .child(
                                            div()
                                                .max_w(px(110.))
                                                .text_ellipsis()
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .child(p.label.clone()),
                                        )
                                        .child(
                                            div()
                                                .id(("ai-rm-img", idx))
                                                .cursor_pointer()
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .on_click(cx.listener(move |pane, _, _, cx| {
                                                    pane.pasted_images.retain(|im| im.id != rm_id);
                                                    cx.notify();
                                                }))
                                                .child(
                                                    crate::ui::icon(IconName::Close)
                                                        .w(px(11.))
                                                        .h(px(11.)),
                                                ),
                                        )
                                })),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .child(
                                Textarea::new(&self.input)
                                    .appearance(false)
                                    .bordered(false)
                                    .aria_label(AI_INPUT_PLACEHOLDER)
                                    .font_family(ui_font.clone())
                                    .text_size(typo.body)
                                    .on_paste(move |item, window, cx| {
                                        pane_paste.update(cx, |pane, cx| {
                                            pane.handle_paste(item, window, cx)
                                        })
                                    })
                                    .token(move |ctx, _, _| {
                                        let id = ctx.token().id();
                                        if id.starts_with("skill:") {
                                            InputToken::new(ctx)
                                                .icon(IconName::Sparkles)
                                                .bg(skill_token_bg)
                                                .border_color(skill_token_border)
                                                .text_color(gpui_kit::rgb(palette.accent))
                                        } else if id.starts_with("dir:") {
                                            InputToken::new(ctx)
                                                .icon(IconName::FolderOpen)
                                                .bg(gpui_kit::rgb(palette.background))
                                                .border_color(subtle_border(palette.border))
                                                .text_color(gpui_kit::rgb(palette.text))
                                        } else {
                                            InputToken::new(ctx)
                                                .icon(IconName::FileText)
                                                .bg(gpui_kit::rgb(palette.background))
                                                .border_color(subtle_border(palette.border))
                                                .text_color(gpui_kit::rgb(palette.text))
                                        }
                                    })
                                    .on_token_click(cx.listener(
                                        |pane,
                                         ev: &gpui_kit::component::input::InlineTokenClickEvent,
                                         _,
                                         cx| {
                                            if let Some(rel) = ev.token().id().strip_prefix("file:") {
                                                let target = pane.resolve_tool_path(rel);
                                                cx.emit(AiPaneEvent::OpenFile(target));
                                            }
                                        },
                                    )),
                            )
                            .when(input_empty, |box_wrap| {
                                box_wrap.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .px(px(10.))
                                        .py(px(8.))
                                        .font_family(ui_font.clone())
                                        .text_size(typo.body)
                                        .line_height(rems(1.25))
                                        .text_color(fg_alpha(palette.text, 0.40))
                                        .child(AI_INPUT_PLACEHOLDER),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Button::new("ai-pick-files")
                                            .custom(subtle_btn(cx))
                                            .compact()
                                            .w(px(26.))
                                            .h(px(26.))
                                            .icon(crate::ui::icon(IconName::Plus))
                                            .tooltip("选择文件附加")
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.pick_files(window, cx);
                                            })),
                                    )
                                    .child({
                                        let has_file = self.active_file.is_some();
                                        let active_ctx = has_file && self.attach_active_file;
                                        let ctx_tip = match &self.active_file {
                                            Some(f) if active_ctx => {
                                                format!("已附加 @{}（点击取消）", f.display)
                                            }
                                            Some(f) => {
                                                format!("附加当前文件 @{}", f.display)
                                            }
                                            None => "当前无打开文件".to_owned(),
                                        };
                                        let ctx_bg = if active_ctx {
                                            mix_color(
                                                palette.surface,
                                                palette.accent,
                                                if palette.is_dark() { 0.22 } else { 0.12 },
                                            )
                                        } else {
                                            gpui_kit::rgb(palette.surface).into()
                                        };
                                        div()
                                            .id("ai-toggle-active-context")
                                            .size(px(26.))
                                            .rounded(px(7.))
                                            .bg(ctx_bg)
                                            .when(active_ctx, |el| {
                                                el.border_1().border_color(mix_color(
                                                    palette.border,
                                                    palette.accent,
                                                    0.45,
                                                ))
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(has_file, |el| {
                                                el.cursor_pointer().hover(move |s| {
                                                    s.bg(gpui_kit::rgb(palette.hover))
                                                })
                                            })
                                            .when(!has_file, |el| el.opacity(0.42))
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(ctx_tip.clone()).build(window, cx)
                                            })
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                if pane.active_file.is_some() {
                                                    pane.attach_active_file =
                                                        !pane.attach_active_file;
                                                    cx.notify();
                                                }
                                            }))
                                            .child(
                                                crate::ui::icon(IconName::Paperclip)
                                                    .w(px(13.))
                                                    .h(px(13.))
                                                    .text_color(gpui_kit::rgb(if active_ctx {
                                                        palette.accent
                                                    } else {
                                                        palette.muted
                                                    })),
                                            )
                                    })
                                    .child(
                                        div()
                                            .id("ai-perm-toggle")
                                            .px_2()
                                            .h(px(24.))
                                            .rounded(px(6.))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(typo.sm)
                                            .text_color(gpui_kit::rgb(
                                                if self.auto_approve_shell {
                                                    palette.danger
                                                } else {
                                                    palette.muted
                                                },
                                            ))
                                            .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                            .tooltip(|window, cx| {
                                                Tooltip::new("切换 Shell 命令确认权限")
                                                    .build(window, cx)
                                            })
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.auto_approve_shell = !pane.auto_approve_shell;
                                                cx.notify();
                                            }))
                                            .child(
                                                crate::ui::icon(if self.auto_approve_shell {
                                                    IconName::ShieldAlert
                                                } else {
                                                    IconName::Shield
                                                })
                                                .w(px(13.))
                                                .h(px(13.)),
                                            )
                                            .child(if self.auto_approve_shell {
                                                "完全访问"
                                            } else {
                                                "需确认"
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .min_w_0()
                                    .child(self.render_context_ring(palette))
                                    .child({
                                        let open = self.model_popover_open;
                                        let hover_bg = fg_alpha(palette.text, 0.06);
                                        div()
                                            .id("ai-model-effort-pill")
                                            .group("ai-model-effort-pill")
                                            .h(px(26.))
                                            .px_2()
                                            .rounded(px(8.))
                                            .when(open, |el| el.bg(hover_bg))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .cursor_pointer()
                                            .text_size(typo.sm)
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .hover(move |s| s.bg(hover_bg))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.model_popover_open = !pane.model_popover_open;
                                                if pane.model_popover_open {
                                                    pane.model_popover_anim_gen = pane
                                                        .model_popover_anim_gen
                                                        .wrapping_add(1);
                                                    pane.model_search_input.update(
                                                        cx,
                                                        |input, cx| {
                                                            input.set_value(
                                                                String::new(),
                                                                window,
                                                                cx,
                                                            );
                                                            input.focus(window, cx);
                                                        },
                                                    );
                                                }
                                                cx.notify();
                                            }))
                                            .child(render_model_icon(
                                                selected_model,
                                                px(13.),
                                                palette.muted,
                                            ))
                                            .child(
                                                div()
                                                    .max_w(px(140.))
                                                    .text_ellipsis()
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .child(model_label),
                                            )
                                            .when(supports_thinking, |btn| {
                                                btn.child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .text_color(fg_alpha(palette.text, 0.40))
                                                        .child(thinking_short_label(
                                                            &self.current_thinking,
                                                        )),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .flex_shrink_0()
                                                    .text_color(fg_alpha(
                                                        palette.text,
                                                        if open { 0.85 } else { 0.38 },
                                                    ))
                                                    .group_hover("ai-model-effort-pill", move |s| {
                                                        s.text_color(fg_alpha(palette.text, 0.85))
                                                    })
                                                    .child(
                                                        crate::ui::icon(if open {
                                                            IconName::ChevronUp
                                                        } else {
                                                            IconName::ChevronDown
                                                        })
                                                        .w(px(12.))
                                                        .h(px(12.)),
                                                    ),
                                            )
                                    })
                                    .child(if self.is_streaming {
                                        div()
                                            .id("ai-abort")
                                            .size(px(28.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(palette.danger))
                                            .text_color(gpui_kit::rgb(palette.on_accent))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor_pointer()
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.abort(cx);
                                            }))
                                            .child(
                                                crate::ui::icon(IconName::Square)
                                                    .w(px(11.))
                                                    .h(px(11.)),
                                            )
                                            .into_any_element()
                                    } else {
                                        div()
                                            .id("ai-send")
                                            .size(px(28.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(if can_send {
                                                palette.accent
                                            } else {
                                                palette.hover
                                            }))
                                            .text_color(gpui_kit::rgb(if can_send {
                                                palette.on_accent
                                            } else {
                                                palette.muted
                                            }))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(can_send, |btn| btn.cursor_pointer())
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.send_prompt(window, cx);
                                            }))
                                            .child(
                                                crate::ui::icon(IconName::ArrowUp)
                                                    .w(px(14.))
                                                    .h(px(14.)),
                                            )
                                            .into_any_element()
                                    }),
                            ),
                    ),
            )
            .into_any_element()
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

        let is_welcome = self.messages.is_empty()
            && self.startup_error.is_none()
            && self.pending_ui.is_none()
            && self.ext_widgets.is_empty();
        let md_style = self.md_style(typo.body, cx);
        let total_msgs = self.messages.len();

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
                let full = pane.cwd.join(&dragged.0);
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
                if self.is_streaming && self.stick_to_bottom {
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
                                .when_some(self.startup_error.clone(), |stream, err| {
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
                                                    .label("重试")
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.start_rpc(cx);
                                                    })),
                                            ),
                                    )
                                })
                    .children({
                        let mut rendered_messages: Vec<AnyElement> = Vec::new();
                        let mut msg_idx = 0usize;
                        while msg_idx < total_msgs {
                            // The startup banner is also an immediate scroll child and must count toward jump indices.
                            let item = rendered_messages.len() + usize::from(self.startup_error.is_some());
                            match &self.messages[msg_idx] {
                                ChatMessage::User {
                                    display_text,
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
                                        attached_file.as_deref().unwrap_or("图片消息")
                                    };
                                    navigation.push(MessageAnchor { item, user: true,
                                        preview: text.chars().take(160).collect() });
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
                                                    .children(images.iter().map(|im| {
                                                        img(im.clone())
                                                            .w(px(72.))
                                                            .h(px(72.))
                                                            .rounded(px(8.))
                                                            .object_fit(ObjectFit::Cover)
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
                                .when(!display_text.is_empty(), |col| {
                                    col.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .pt_0p5()
                                            .child(ai_copy_button(
                                                SharedString::from(format!(
                                                    "ai-copy-user-{msg_idx}"
                                                )),
                                                "复制消息",
                                                "已复制",
                                                display_text.clone(),
                                            )),
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
                                        self.messages.get(run_end),
                                        Some(ChatMessage::Assistant { .. })
                                    ) {
                                        run_end += 1;
                                    }
                                    // Preview only visible prose; thinking and tool logs stay out of navigation cards.
                                    let preview: String = self.messages[run_start..run_end].iter()
                                        .flat_map(|message| match message {
                                            ChatMessage::Assistant { parts, .. } => parts.as_slice(),
                                            _ => &[],
                                        })
                                        .filter_map(|part| match part {
                                            AssistantPart::Text(text) => Some(text.as_str()),
                                            _ => None,
                                        })
                                        .flat_map(str::chars)
                                        .take(160)
                                        .collect();
                                    navigation.push(MessageAnchor { item, user: false,
                                        preview: if preview.trim().is_empty() { "思考与工具调用".into() } else { preview } });
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
                    .child(self.render_message_navigation(&navigation, palette, cx))
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
                                                Tooltip::new("回到底部").build(window, cx)
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
            .when_some(self.pending_ui.as_ref(), |root, pending| {
                root.child(self.render_pending_ui(pending, palette, cx))
            })
            .when(!self.ext_widgets.is_empty(), |root| {
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
                            self.ext_widgets
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
        collapsed: bool,
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
    let total_secs = (ms / 1000).max(1);
    if total_secs < 60 {
        format!("{total_secs} 秒")
    } else {
        format!("{} 分 {} 秒", total_secs / 60, total_secs % 60)
    }
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
                "bash" | "powershell" => has_cmd = true,
                "read" | "grep" | "find" | "ls" => has_read = true,
                _ => has_other = true,
            },
        }
    }

    let mut verbs: Vec<&'static str> = Vec::new();
    if is_running {
        if has_edit {
            verbs.push("正在编辑文件");
        }
        if has_cmd {
            verbs.push("正在运行命令");
        }
        if has_read && verbs.is_empty() {
            verbs.push("正在查阅文件");
        }
        if has_other && verbs.is_empty() {
            verbs.push("正在调用工具");
        }
        if verbs.is_empty() && has_thinking {
            return "正在思考…".to_owned();
        }
        return format!("{}…", verbs.join(" · "));
    }

    if has_edit {
        verbs.push("编辑了文件");
    }
    if has_cmd {
        verbs.push("运行了命令");
    }
    if has_read && !has_edit && !has_cmd {
        verbs.push("查阅了文件");
    } else if has_read && verbs.len() < 2 {
        verbs.push("查阅了文件");
    }
    if has_other {
        verbs.push("调用了工具");
    }
    if verbs.is_empty() && has_thinking {
        return "思考过程".to_owned();
    }
    verbs.join(" · ")
}

fn process_group_icon(steps: &[ProcessStep<'_>]) -> IconName {
    let has_edit = steps
        .iter()
        .any(|s| matches!(s, ProcessStep::Tool { block, .. } if block.name == "edit" || block.name == "write"));
    let has_cmd = steps
        .iter()
        .any(|s| matches!(s, ProcessStep::Tool { block, .. } if block.name == "bash" || block.name == "powershell"));
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
    let verb = if is_created { "已创建" } else { "已编辑" };
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
        .map_err(|e| format!("无法解析图片：{e}"))?;
    let mut out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| format!("PNG 转码失败：{e}"))?;
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
        ImageFormat::Svg => return Err("不支持直接粘贴 SVG 矢量图片".to_owned()),
        ImageFormat::Ico | ImageFormat::Pnm => {
            return Err("不支持直接粘贴该图片格式".to_owned())
        }
    };
    let data_b64 = encode_base64(&bytes);
    let render_img = Arc::new(gpui::Image::from_bytes(gpui_fmt, bytes));
    Ok(PastedImage {
        id,
        image: render_img,
        mime_type: mime.to_owned(),
        data_b64,
        label: format!("图片 #{id}"),
    })
}

fn file_path_to_pasted_image(id: u64, path: &Path) -> Result<PastedImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取图片失败：{e}"))?;
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
        _ => return Err("不支持的图片格式".to_owned()),
    };
    let label = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("图片 #{id}"));
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
        return format!("已编辑 {} +{} -{}", fname, first.added, first.removed);
    }
    match name {
        "folio_tags_list" => "查看 Folio 标签".into(),
        "folio_files_query" => "查找 Folio 文件及标签".into(),
        "folio_file_tags_update" => {
            if let Some(folder) = args["under_directory"].as_str() {
                format!("编辑目录「{folder}」下的文件标签")
            } else {
                format!("编辑 {} 个文件的标签", args["paths"].as_array().map_or(0, Vec::len))
            }
        }
        "folio_tags_manage" => format!("管理标签：{}", args["tag"].as_str().unwrap_or_default()),
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
