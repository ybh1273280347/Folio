//! Native GPUI AI interface backed by `pi --mode rpc`.
mod diff;
mod rpc;
mod sessions;

use crate::math::{MathReady, MathRenderer};
use crate::mermaid::{MermaidReady, MermaidRenderer};
use crate::model::AppSettings;
use crate::terminal::{DraggedFolder, DraggedPaths};
use crate::theme::{Palette, color as theme_color, palette as active_palette};
use diff::{DiffLine, FileDiff, compute_diff};
use gpui::{
    ClipboardEntry, EntityInputHandler, ExternalPaths, ImageFormat, MouseButton, ObjectFit,
    ScrollEvent, ScrollHandle, Subscription,
};
use gpui_kit::{
    assets::IconName,
    base::{
        Scrollbar, ScrollbarMode, StyledExt,
        text::{MarkdownExtensions, TextView, TextViewStyle},
    },
    component::{
        button::{Button, ButtonCustomVariant, ButtonVariants},
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
use sessions::SessionSummary;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub enum AiPaneEvent {
    OpenFile(PathBuf),
    ToggleMaximize,
    Hide,
}
impl EventEmitter<AiPaneEvent> for AiPane {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedFile {
    pub display: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SelectionQuote {
    pub source: String,
    pub text: String,
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
}

#[derive(Clone, Debug)]
enum AssistantPart {
    Thinking { text: String, collapsed: bool },
    Text(String),
    Tool(ToolBlock),
}

#[derive(Clone)]
enum ChatMessage {
    User {
        display_text: String,
        attached_file: Option<String>,
        mounted_skills: Vec<String>,
        quote: Option<SelectionQuote>,
        images: Vec<Arc<gpui::Image>>,
    },
    Assistant {
        parts: Vec<AssistantPart>,
        error: Option<String>,
        streaming: bool,
    },
    Notice {
        text: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug)]
enum PendingUiKind {
    Confirm {
        message: String,
        is_shell_gate: bool,
    },
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
        let base = (settings.interface_font_size as f32).clamp(11., 24.);
        Self {
            body: px(base),
            sm: px((base - 1.5).max(10.5)),
            xs: px((base - 2.5).max(9.5)),
        }
    }
}

pub struct AiPane {
    cwd: PathBuf,
    settings: AppSettings,
    client: Option<RpcClient>,
    startup_error: Option<String>,
    messages: Vec<ChatMessage>,
    is_streaming: bool,
    is_compacting: bool,
    session_file: Option<PathBuf>,
    session_id: Option<String>,
    session_name: Option<String>,
    sessions: Vec<SessionSummary>,
    session_drawer_open: bool,
    renaming_session: Option<PathBuf>,
    rename_input: Entity<InputState>,
    available_models: Vec<ModelInfo>,
    current_model: Option<ModelInfo>,
    thinking_levels: Vec<String>,
    current_thinking: String,
    context_usage: Option<ContextUsage>,
    commands: Vec<PiCommandInfo>,
    active_file: Option<AttachedFile>,
    attach_active_file: bool,
    quoted_selection: Option<SelectionQuote>,
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
    model_popover_open: bool,
    model_popover_anim_gen: u64,
    model_search_input: Entity<InputState>,
    favorite_models: HashSet<String>,
    history_anim_gen: u64,
    slash_anim_gen: u64,
    slash_was_open: bool,
    slash_index: usize,
    slash_scroll: ScrollHandle,
    last_input_text: String,
    dismissed_slash_text: Option<String>,
    expanded_tool_groups: HashSet<(usize, usize)>,
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

impl AiPane {
    pub fn new(
        cwd: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::theme::apply_font(&settings.interface_font, cx);
        let mermaid = cx.new(MermaidRenderer::new);
        let md_extensions = crate::markdown::markdown_extensions(&math, cx)
            .plugin(mermaid.read(cx).plugin(mermaid.downgrade()));
        let sub_math = cx.subscribe(&math, |_, _, event: &MathReady, cx| {
            if matches!(event, MathReady::Ready) {
                cx.notify();
            }
        });
        let sub_mermaid = cx.subscribe(&mermaid, |_, _, _: &MermaidReady, cx| {
            cx.notify();
        });
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 7)
                .placeholder("随心输入，/ 呼出命令…")
                .submit_on_enter(true)
        });
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入会话名称"));
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
        let sub_rename = cx.subscribe_in(
            &rename_input,
            window,
            |pane, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    pane.commit_rename(window, cx);
                }
            },
        );
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

        let mut pane = Self {
            cwd,
            settings,
            client: None,
            startup_error: None,
            messages: Vec::new(),
            is_streaming: false,
            is_compacting: false,
            session_file: None,
            session_id: None,
            session_name: None,
            sessions: Vec::new(),
            session_drawer_open: false,
            renaming_session: None,
            rename_input,
            available_models: Vec::new(),
            current_model: None,
            thinking_levels: vec!["off".into(), "low".into(), "medium".into(), "high".into()],
            current_thinking: "high".into(),
            context_usage: None,
            commands: Vec::new(),
            active_file: None,
            attach_active_file: false,
            quoted_selection: None,
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
            model_popover_open: false,
            model_popover_anim_gen: 0,
            model_search_input,
            favorite_models: load_favorite_models(),
            history_anim_gen: 0,
            slash_anim_gen: 0,
            slash_was_open: false,
            slash_index: 0,
            slash_scroll: ScrollHandle::new(),
            last_input_text: String::new(),
            dismissed_slash_text: None,
            expanded_tool_groups: HashSet::new(),
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
                sub_rename,
                sub_ext,
                sub_math,
                sub_mermaid,
            ],
            _poll: poll,
        };
        pane.refresh_sessions();
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

    pub fn set_directory(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        if self.cwd == cwd {
            return;
        }
        self.cwd = cwd;
        self.messages.clear();
        self.expanded_tool_groups.clear();
        self.session_file = None;
        self.session_id = None;
        self.session_name = None;
        self.context_usage = None;
        self.pending_ui = None;
        self.is_streaming = false;
        self.refresh_sessions();
        if self.client.is_some() {
            self.client = None;
            self.start_rpc(cx);
        }
        cx.notify();
    }

    pub fn sync_settings(&mut self, settings: &AppSettings, cx: &mut Context<Self>) {
        let base_changed = self.settings.ai.base_url != settings.ai.base_url;
        self.settings = settings.clone();
        crate::theme::apply_font(&self.settings.interface_font, cx);
        if base_changed && self.client.is_some() {
            self.client = None;
            self.start_rpc(cx);
        }
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
            self.quoted_selection = Some(SelectionQuote {
                source,
                text: trimmed.to_owned(),
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
        match RpcClient::spawn(&self.cwd, &self.settings.ai.base_url) {
            Ok(client) => {
                let _ = client.get_state();
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

    fn refresh_sessions(&mut self) {
        self.sessions = sessions::list_sessions_for_cwd(&self.cwd);
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
                    "new_session" | "clone" => {
                        if !data.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            if command == "new_session" {
                                self.messages.clear();
                                self.expanded_tool_groups.clear();
                                self.context_usage = None;
                            } else {
                                self.show_toast("已从当前位置创建分支会话".into(), false, cx);
                            }
                            self.active_part_map.clear();
                            self.pending_ui = None;
                            self.is_streaming = false;
                            if let Some(client) = &self.client {
                                let _ = client.get_state();
                            }
                            self.refresh_sessions();
                        }
                    }
                    "switch_session" => {
                        if !data.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            self.messages.clear();
                            self.expanded_tool_groups.clear();
                            self.active_part_map.clear();
                            self.pending_ui = None;
                            self.is_streaming = false;
                            self.session_drawer_open = false;
                            if let Some(client) = &self.client {
                                let _ = client.get_state();
                                let _ = client.get_messages();
                                let _ = client.get_session_stats();
                            }
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
                        "pi 进程已退出{}",
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
        self.refresh_sessions();
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
                self.context_usage = Some(ContextUsage {
                    tokens: cu.get("tokens").and_then(|v| v.as_u64()),
                    context_window,
                    percent: cu.get("percent").and_then(|v| v.as_u64()),
                });
            }
        }
    }

    fn apply_messages(&mut self, data: &Value) {
        let Some(arr) = data.get("messages").and_then(|v| v.as_array()) else {
            return;
        };
        self.messages.clear();
        self.expanded_tool_groups.clear();
        let mut pending_tool_results: HashMap<String, (String, bool)> = HashMap::new();
        for msg in arr {
            if msg.get("role").and_then(|v| v.as_str()) == Some("toolResult")
                && let Some(id) = msg.get("toolCallId").and_then(|v| v.as_str())
            {
                let text = extract_text_content(msg.get("content"));
                let is_err = msg.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                pending_tool_results.insert(id.to_owned(), (text, is_err));
            }
        }

        for msg in arr {
            match msg.get("role").and_then(|v| v.as_str()) {
                Some("user") => {
                    let raw = extract_text_content(msg.get("content"));
                    let (attached_file, mounted_skills, quote, display_text) =
                        split_injected_prompt(&raw);
                    self.messages.push(ChatMessage::User {
                        display_text,
                        attached_file,
                        mounted_skills,
                        quote,
                        images: Vec::new(),
                    });
                }
                Some("assistant") => {
                    let mut parts = Vec::new();
                    if let Some(blocks) = msg.get("content").and_then(|v| v.as_array()) {
                        for block in blocks {
                            match block.get("type").and_then(|v| v.as_str()) {
                                Some("thinking") => {
                                    let t = block
                                        .get("thinking")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_owned();
                                    if !t.is_empty() {
                                        parts.push(AssistantPart::Thinking {
                                            text: t,
                                            collapsed: true,
                                        });
                                    }
                                }
                                Some("text") => {
                                    let t = block
                                        .get("text")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_owned();
                                    if !t.is_empty() {
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
                                    let (output, is_error) = pending_tool_results
                                        .remove(&id)
                                        .unwrap_or_default();
                                    let diffs = build_diffs_for_tool(&name, &args, None);
                                    let summary = summarize_tool_call(&name, &args, &diffs);
                                    parts.push(AssistantPart::Tool(ToolBlock {
                                        id,
                                        name,
                                        args,
                                        summary,
                                        output,
                                        is_error,
                                        done: true,
                                        duration_ms: None,
                                        diffs,
                                        expanded: false,
                                    }));
                                }
                                _ => {}
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
                        streaming: false,
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
            }
            "agent_settled" => {
                self.is_streaming = false;
                self.pending_ui = None;
                self.refresh_sessions();
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
                self.refresh_sessions();
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
                if let Some(msg) = ev.get("message")
                    && msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
                    && let Some(ChatMessage::Assistant {
                        parts,
                        error,
                        streaming,
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
                    let has_text = parts
                        .iter()
                        .any(|p| matches!(p, AssistantPart::Text(t) if !t.trim().is_empty()));
                    if has_text {
                        for p in parts.iter_mut() {
                            if let AssistantPart::Thinking { collapsed, .. } = p {
                                *collapsed = true;
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
                    && let Some(rel) = extract_path_arg(&args)
                {
                    let target = self.resolve_tool_path(&rel);
                    if let Ok(meta) = std::fs::metadata(&target)
                        && meta.len() <= 512 * 1024
                        && let Ok(old) = std::fs::read_to_string(&target)
                    {
                        self.write_snapshots.insert(call_id.clone(), old);
                    }
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
                    if !output.is_empty() {
                        block.output = output;
                    }
                    if let Some((write_args, diffs)) = write_new_fallback {
                        block.args = write_args;
                        block.diffs = diffs;
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
            });
        }

        match ty {
            "thinking_start" => {
                let idx = self.push_assistant_part(AssistantPart::Thinking {
                    text: String::new(),
                    collapsed: false,
                });
                self.active_part_map.insert(content_index, idx);
            }
            "thinking_delta" => {
                let delta = ame.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                let idx = self.get_or_create_part(content_index, || AssistantPart::Thinking {
                    text: String::new(),
                    collapsed: false,
                });
                if let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Thinking { text, .. }) = parts.get_mut(idx)
                {
                    text.push_str(delta);
                }
                self.scroll_to_bottom();
            }
            "thinking_end" => {
                if let Some(content) = ame.get("content").and_then(|v| v.as_str())
                    && let Some(&idx) = self.active_part_map.get(&content_index)
                    && let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Thinking { text, .. }) = parts.get_mut(idx)
                {
                    *text = content.to_owned();
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
            "toolcall_start" => {
                let id = ame.get("id").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                let name = ame
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_owned();
                let block = ToolBlock {
                    id,
                    name: name.clone(),
                    args: Value::Null,
                    summary: name,
                    output: String::new(),
                    is_error: false,
                    done: false,
                    duration_ms: None,
                    diffs: Vec::new(),
                    expanded: false,
                };
                let idx = self.push_assistant_part(AssistantPart::Tool(block));
                self.active_part_map.insert(content_index, idx);
                self.scroll_to_bottom();
            }
            "toolcall_end" => {
                if let Some(tc) = ame.get("toolCall") {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                    let name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_owned();
                    let args = tc.get("arguments").cloned().unwrap_or(Value::Null);
                    let diffs = if name == "edit" {
                        build_diffs_for_tool(&name, &args, None)
                    } else {
                        Vec::new()
                    };
                    let summary = summarize_tool_call(&name, &args, &diffs);
                    if let Some(&idx) = self.active_part_map.get(&content_index)
                        && let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
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
            build_diffs_for_tool(name, args, None)
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
                let is_shell_gate = title.contains("bash") || title.contains("powershell");
                if is_shell_gate && self.auto_approve_shell {
                    if let Some(client) = &self.client {
                        let _ = client.respond_extension_confirm(&id, true);
                    }
                    return;
                }
                self.pending_ui = Some(PendingExtensionUi {
                    id,
                    title,
                    kind: PendingUiKind::Confirm {
                        message,
                        is_shell_gate,
                    },
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

    fn respond_confirm(&mut self, confirmed: bool, auto_allow_shell: bool, cx: &mut Context<Self>) {
        if auto_allow_shell {
            self.auto_approve_shell = true;
        }
        if let Some(pending) = self.pending_ui.take()
            && let Some(client) = &self.client
        {
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
        if trimmed.is_empty() && self.quoted_selection.is_none() && self.pasted_images.is_empty() {
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
        let quote = self.quoted_selection.take();
        let pasted = std::mem::take(&mut self.pasted_images);
        let rpc_images: Vec<RpcImage> = pasted
            .iter()
            .map(|p| RpcImage {
                data: p.data_b64.clone(),
                mime_type: p.mime_type.clone(),
            })
            .collect();
        let msg_images: Vec<Arc<gpui::Image>> = pasted.into_iter().map(|p| p.image).collect();

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
        if let Some(q) = &quote {
            payload.push_str("> 引用自 ");
            payload.push_str(&q.source);
            payload.push('\n');
            for line in q.text.lines() {
                payload.push_str("> ");
                payload.push_str(line);
                payload.push('\n');
            }
            payload.push('\n');
        }
        payload.push_str(body_trimmed);

        if let Err(err) = client.prompt_with_images(&payload, &rpc_images) {
            self.show_toast(format!("发送失败：{err}"), true, cx);
            return;
        }

        self.messages.push(ChatMessage::User {
            display_text: body_trimmed.to_owned(),
            attached_file: attached.map(|f| f.display),
            mounted_skills: skills,
            quote,
            images: msg_images,
        });
        self.input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.is_streaming = true;
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

    fn new_session(&mut self, cx: &mut Context<Self>) {
        self.stick_to_bottom = true;
        if let Some(client) = &self.client {
            let _ = client.new_session();
        } else {
            self.messages.clear();
            self.expanded_tool_groups.clear();
            self.start_rpc(cx);
        }
        self.session_drawer_open = false;
        cx.notify();
    }

    fn switch_session(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.stick_to_bottom = true;
        if let Some(client) = &self.client {
            let _ = client.switch_session(&path.to_string_lossy());
        }
        self.session_drawer_open = false;
        cx.notify();
    }

    fn delete_session(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let is_current = self.session_file.as_ref() == Some(&path);
        if is_current {
            self.new_session(cx);
        }
        if let Err(err) = sessions::delete_session_file(&path) {
            self.show_toast(err, true, cx);
        }
        self.refresh_sessions();
        cx.notify();
    }

    fn start_rename(
        &mut self,
        path: PathBuf,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.renaming_session = Some(path);
        self.rename_input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn commit_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(target_path) = self.renaming_session.take() else {
            return;
        };
        let new_name = self.rename_input.read(cx).value().trim().to_owned();
        if new_name.is_empty() {
            cx.notify();
            return;
        }
        if self.session_file.as_ref() == Some(&target_path) {
            if let Some(client) = &self.client {
                let _ = client.set_session_name(&new_name);
            }
        } else if let Some(client) = &self.client {
            let _ = client.switch_session(&target_path.to_string_lossy());
            let _ = client.set_session_name(&new_name);
        }
        self.refresh_sessions();
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

    fn md_style(&self, cx: &App) -> TextViewStyle {
        TextViewStyle::from_theme(&gpui_kit::base::Theme::global(cx))
            .with_inline_code_style(crate::ui::inline_code_style(&self.settings))
            .with_code_block(crate::ui::block_code_style(&self.settings))
    }

    fn render_diff_card(
        &self,
        diff: &FileDiff,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let add_tint = mix_color(palette.surface, 0x22c55e, 0.14);
        let add_fg = theme_color(0x16a34a);
        let del_tint = mix_color(palette.surface, palette.danger, 0.14);
        let del_fg = gpui_kit::rgb(palette.danger);

        let target = self.resolve_tool_path(&diff.path);
        let typo = AiTypography::from_settings(&self.settings);
        let code_font = if self.settings.block_code_font.is_empty() {
            SharedString::from("Consolas")
        } else {
            self.settings.block_code_font.clone().into()
        };

        div()
            .mt_1p5()
            .rounded(px(6.))
            .border_1()
            .border_color(gpui_kit::rgb(palette.border))
            .overflow_hidden()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id(SharedString::from(format!("diff-open-{}", diff.path)))
                    .px_2p5()
                    .py_1()
                    .bg(gpui_kit::rgb(palette.background))
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(AiPaneEvent::OpenFile(target.clone()));
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .child(crate::ui::icon(IconName::FileDiff).w(px(12.)).h(px(12.)))
                            .child(
                                div()
                                    .text_size(typo.sm)
                                    .font_medium()
                                    .text_ellipsis()
                                    .child(diff.path.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(typo.sm)
                            .flex_shrink_0()
                            .when(diff.added > 0, |r| {
                                r.child(div().text_color(add_fg).child(format!("+{}", diff.added)))
                            })
                            .when(diff.removed > 0, |r| {
                                r.child(div().text_color(del_fg).child(format!("-{}", diff.removed)))
                            }),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("diff-body-{}", diff.path)))
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .font_family(code_font)
                    .text_size(typo.sm)
                    .line_height(relative(1.4))
                    .children(diff.lines.iter().take(120).map(|line| match line {
                        DiffLine::Context(s) => div()
                            .px_2()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child(format!("  {s}"))
                            .into_any_element(),
                        DiffLine::Removed(s) => div()
                            .px_2()
                            .bg(del_tint)
                            .text_color(del_fg)
                            .child(format!("- {s}"))
                            .into_any_element(),
                        DiffLine::Added(s) => div()
                            .px_2()
                            .bg(add_tint)
                            .text_color(add_fg)
                            .child(format!("+ {s}"))
                            .into_any_element(),
                        DiffLine::Ellipsis => div()
                            .px_2()
                            .py_0p5()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child("  …")
                            .into_any_element(),
                    })),
            )
            .into_any_element()
    }

    fn render_tool_group(
        &self,
        msg_idx: usize,
        start_idx: usize,
        blocks: &[(usize, &ToolBlock)],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_key = (msg_idx, start_idx);
        let group_expanded = self.expanded_tool_groups.contains(&group_key);

        let any_running = blocks.iter().any(|(_, b)| !b.done);
        let any_error = blocks.iter().any(|(_, b)| b.is_error);
        let has_edit = blocks
            .iter()
            .any(|(_, b)| b.name == "edit" || b.name == "write");
        let has_other = blocks
            .iter()
            .any(|(_, b)| b.name != "edit" && b.name != "write");

        let group_label = if any_running {
            if has_edit && !has_other {
                "正在编辑文件…"
            } else {
                "正在调用工具…"
            }
        } else {
            match (has_edit, has_other) {
                (true, true) => "编辑了文件，调用了工具",
                (true, false) => "编辑了文件",
                _ => "调用了工具",
            }
        };

        let header_icon = if has_edit && !has_other {
            IconName::PencilLine
        } else {
            IconName::Terminal
        };
        let pill_bg = mix_color(palette.background, palette.surface, 0.72);
        let pill_hover = mix_color(palette.hover, palette.surface, 0.35);
        let typo = AiTypography::from_settings(&self.settings);
        let dot_color = if any_error {
            gpui_kit::rgb(palette.danger)
        } else if any_running {
            gpui_kit::rgb(palette.accent)
        } else {
            theme_color(0x16a34a)
        };

        let status_dot = if any_running {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(dot_color)
                .with_animation(
                    ("tool-pulse", msg_idx * 1000 + start_idx),
                    Animation::new(Duration::from_millis(1000)).repeat(),
                    |el, t| {
                        let a = 0.35 + 0.65 * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos());
                        el.opacity(a)
                    },
                )
                .into_any_element()
        } else {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(dot_color)
                .opacity(0.85)
                .into_any_element()
        };

        div()
            .my_0p5()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id(("tool-group", msg_idx * 1000 + start_idx))
                            .px_2p5()
                            .py_1()
                            .rounded(px(8.))
                            .bg(pill_bg)
                            .border_1()
                            .border_color(subtle_border(palette.border))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .text_size(typo.sm)
                            .text_color(gpui_kit::rgb(palette.muted))
                            .hover(move |s| {
                                s.bg(pill_hover).text_color(gpui_kit::rgb(palette.text))
                            })
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                if !pane.expanded_tool_groups.remove(&group_key) {
                                    pane.expanded_tool_groups.insert(group_key);
                                }
                                cx.notify();
                            }))
                            .child(status_dot)
                            .child(crate::ui::icon(header_icon).w(px(12.)).h(px(12.)))
                            .child(div().font_medium().child(group_label))
                            .child(
                                crate::ui::icon(if group_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .w(px(12.))
                                .h(px(12.)),
                            ),
                    ),
            )
            .when(group_expanded, |col| {
                col.child(
                    div()
                        .ml_2p5()
                        .pl_3()
                        .border_l_2()
                        .border_color(subtle_border(palette.border))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(blocks.iter().map(|&(part_idx, block)| {
                            let step_icon = match block.name.as_str() {
                                "edit" | "write" => IconName::PencilLine,
                                "bash" | "powershell" => IconName::Terminal,
                                _ => IconName::Wrench,
                            };
                            let has_detail =
                                !block.diffs.is_empty() || !block.output.trim().is_empty();
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("tool-step", msg_idx * 1000 + part_idx))
                                        .px_2()
                                        .py_1()
                                        .rounded(px(6.))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .when(has_detail, |row| {
                                            row.cursor_pointer()
                                                .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                        })
                                        .on_click(cx.listener(move |pane, _, _, cx| {
                                            if !has_detail {
                                                return;
                                            }
                                            if let Some(ChatMessage::Assistant { parts, .. }) =
                                                pane.messages.get_mut(msg_idx)
                                                && let Some(AssistantPart::Tool(b)) =
                                                    parts.get_mut(part_idx)
                                            {
                                                b.expanded = !b.expanded;
                                                cx.notify();
                                            }
                                        }))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1p5()
                                                .min_w_0()
                                                .text_size(typo.sm)
                                                .text_color(gpui_kit::rgb(if block.is_error {
                                                    palette.danger
                                                } else {
                                                    palette.muted
                                                }))
                                                .child(
                                                    crate::ui::icon(step_icon)
                                                        .w(px(12.))
                                                        .h(px(12.)),
                                                )
                                                .child(
                                                    div()
                                                        .text_ellipsis()
                                                        .child(block.summary.clone()),
                                                ),
                                        )
                                        .when_some(block.duration_ms, |row, ms| {
                                            row.child(
                                                div()
                                                    .text_size(typo.xs)
                                                    .text_color(gpui_kit::rgb(palette.muted))
                                                    .flex_shrink_0()
                                                    .child(format!("{ms}ms")),
                                            )
                                        }),
                                )
                                .when(block.expanded, |step| {
                                    step.when(!block.diffs.is_empty(), |s| {
                                        s.children(
                                            block
                                                .diffs
                                                .iter()
                                                .map(|d| self.render_diff_card(d, palette, cx)),
                                        )
                                    })
                                    .when(!block.output.trim().is_empty(), |s| {
                                        s.child(
                                            div()
                                                .on_scroll(|_, _, cx| cx.stop_propagation())
                                                .child(
                                                    div()
                                                        .id(("tool-output", msg_idx * 1000 + part_idx))
                                                        .p_2p5()
                                                        .max_h(px(180.))
                                                        .overflow_y_scroll()
                                                        .rounded(px(8.))
                                                        .border_1()
                                                        .border_color(subtle_border(palette.border))
                                                        .bg(gpui_kit::rgb(palette.code_background))
                                                        .font_family("Consolas")
                                                        .text_size(typo.sm)
                                                        .text_color(gpui_kit::rgb(palette.text))
                                                        .child(block.output.clone()),
                                                ),
                                        )
                                    })
                                })
                                .into_any_element()
                        }))
                        .with_animation(
                            ("tool-group-open", msg_idx * 1000 + start_idx),
                            Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                            |el, t| el.opacity(t),
                        ),
                )
            })
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
        let mut card = div()
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

        match &pending.kind {
            PendingUiKind::Confirm {
                message,
                is_shell_gate,
            } => {
                if !message.trim().is_empty() {
                    card = card.child(
                        div()
                            .id("ext-confirm-cmd")
                            .p_2()
                            .max_h(px(120.))
                            .overflow_y_scroll()
                            .rounded(px(5.))
                            .bg(gpui_kit::rgb(palette.code_background))
                            .font_family("Consolas")
                            .text_size(typo.sm)
                            .text_color(gpui_kit::rgb(palette.text))
                            .child(message.clone()),
                    );
                }
                let is_gate = *is_shell_gate;
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_1p5()
                        .child(
                            Button::new("ext-deny")
                                .ghost()
                                .compact()
                                .label("拒绝")
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.respond_confirm(false, false, cx);
                                })),
                        )
                        .when(is_gate, |row| {
                            row.child(
                                Button::new("ext-allow-session")
                                    .ghost()
                                    .compact()
                                    .label("总是允许")
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.respond_confirm(true, true, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new("ext-allow")
                                .primary()
                                .compact()
                                .label("允许")
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.respond_confirm(true, false, cx);
                                })),
                        ),
                );
            }
            PendingUiKind::Select { options } => {
                card = card.child(
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
                );
            }
            PendingUiKind::TextInput {
                placeholder,
                multiline,
            } => {
                let is_multi = *multiline;
                card = card
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
                    );
            }
        }
        card.into_any_element()
    }

    fn render_sessions_popover(
        &self,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        let active_bg = mix_color(palette.surface, palette.accent, if palette.is_dark() { 0.16 } else { 0.08 });
        div()
            .id("ai-sessions-popover")
            .w(px(308.))
            .max_h(px(360.))
            .rounded(px(12.))
            .border_1()
            .border_color(subtle_border(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font)
            .p_1p5()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .gap_1()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(self.sessions.is_empty(), |list| {
                list.child(
                    div()
                        .px_3()
                        .py_4()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .text_size(typo.sm)
                        .text_color(gpui_kit::rgb(palette.muted))
                        .child(crate::ui::icon(IconName::Clock).w(px(13.)).h(px(13.)))
                        .child("暂无历史会话"),
                )
            })
            .children(self.sessions.iter().enumerate().map(|(idx, s)| {
                let is_active = self.session_file.as_ref() == Some(&s.path)
                    || self.session_id.as_deref() == Some(s.id.as_str());
                let is_renaming = self.renaming_session.as_ref() == Some(&s.path);
                let switch_path = s.path.clone();
                let rename_path = s.path.clone();
                let delete_path = s.path.clone();
                let current_title = s.display_title().to_owned();
                div()
                    .id(("ai-session-row", idx))
                    .px_2p5()
                    .py_2()
                    .rounded(px(8.))
                    .when(is_active, |row| row.bg(active_bg))
                    .when(!is_active, |row| {
                        row.hover(move |st| st.bg(gpui_kit::rgb(palette.hover)))
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if pane.renaming_session.is_none() {
                            pane.switch_session(&switch_path, cx);
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(if is_renaming {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(Input::new(&self.rename_input))
                                    .into_any_element()
                            } else {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .size(px(6.))
                                            .rounded_full()
                                            .flex_shrink_0()
                                            .bg(gpui_kit::rgb(if is_active {
                                                palette.accent
                                            } else {
                                                palette.border
                                            })),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(typo.sm)
                                            .font_medium()
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .text_ellipsis()
                                            .child(current_title.clone()),
                                    )
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_0p5()
                                    .flex_shrink_0()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(
                                        div()
                                            .id(("ai-sess-ren", idx))
                                            .size(px(22.))
                                            .rounded(px(5.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(gpui_kit::rgb(palette.muted))
                                            .hover(move |st| {
                                                st.bg(gpui_kit::rgb(palette.pressed))
                                                    .text_color(gpui_kit::rgb(palette.text))
                                            })
                                            .child(
                                                crate::ui::icon(IconName::PencilLine)
                                                    .w(px(12.))
                                                    .h(px(12.)),
                                            )
                                            .on_click(cx.listener(move |pane, _, window, cx| {
                                                pane.start_rename(
                                                    rename_path.clone(),
                                                    current_title.clone(),
                                                    window,
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id(("ai-sess-del", idx))
                                            .size(px(22.))
                                            .rounded(px(5.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(gpui_kit::rgb(palette.muted))
                                            .hover(move |st| {
                                                st.bg(gpui_kit::rgb(palette.pressed))
                                                    .text_color(gpui_kit::rgb(palette.danger))
                                            })
                                            .child(
                                                crate::ui::icon(IconName::Trash)
                                                    .w(px(12.))
                                                    .h(px(12.)),
                                            )
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                pane.delete_session(delete_path.clone(), cx);
                                                cx.stop_propagation();
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .mt_0p5()
                            .pl_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(typo.xs)
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child(format!("{} 条", s.message_count))
                            .child(s.timestamp_label.clone()),
                    )
            }))
            .with_animation(
                ("ai-history-pop", self.history_anim_gen),
                Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mt(px((1. - t) * -6.)),
            )
            .into_any_element()
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
        let fraction = (pct as f32 / 100.0).clamp(0.0, 1.0);

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

                        if fraction > 0.01 {
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
                            .py_1p5()
                            .border_b_1()
                            .border_color(subtle_border(palette.border))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .flex_shrink_0()
                            .child(
                                crate::ui::icon(IconName::Search)
                                    .w(px(13.))
                                    .h(px(13.))
                                    .text_color(gpui_kit::rgb(palette.muted)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(typo.sm)
                                    .child(
                                        Input::new(&self.model_search_input)
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
}

impl Render for AiPane {
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
        let ui_font: SharedString = if self.settings.interface_font.is_empty() {
            "Segoe UI".into()
        } else {
            self.settings.interface_font.clone().into()
        };
        let workspace_title = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "工作区".to_owned());

        let session_label = self
            .session_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if self.messages.is_empty() {
                    "新会话".to_owned()
                } else {
                    "当前会话".to_owned()
                }
            });

        let model_label = self
            .current_model
            .as_ref()
            .map(|m| short_model_name(&m.name))
            .unwrap_or_else(|| "模型".to_owned());
        let selected_model = self.current_model.as_ref();
        let supports_thinking = self.current_model.as_ref().is_none_or(|m| m.reasoning);
        let model_effort_label = if supports_thinking {
            format!(
                "{model_label} · {}",
                thinking_short_label(&self.current_thinking)
            )
        } else {
            model_label
        };

        let subtle_btn = |cx: &App| {
            ButtonCustomVariant::new(cx)
                .foreground(gpui_kit::rgb(palette.muted).into())
                .hover(gpui_kit::rgb(palette.hover).into())
                .active(gpui_kit::rgb(palette.pressed).into())
        };

        let attached_chip = self.effective_attached_file().cloned();
        let has_chips = attached_chip.is_some()
            || self.quoted_selection.is_some()
            || !self.pasted_images.is_empty();
        let slash_items = self.current_slash_items(cx);
        let has_slash_items = !slash_items.is_empty();
        let can_send = !self.input.read(cx).value().trim().is_empty()
            || self.quoted_selection.is_some()
            || !self.pasted_images.is_empty();
        let input_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let pane_paste = cx.entity();
        let md_style = self.md_style(cx);
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
        let drag_tint = mix_color(
            palette.background,
            palette.accent,
            if palette.is_dark() { 0.12 } else { 0.06 },
        );
        let composer_border = if input_focused {
            mix_color(
                palette.border,
                palette.accent,
                if palette.is_dark() { 0.65 } else { 0.55 },
            )
        } else {
            subtle_border(palette.border)
        };
        let composer_shadow = if input_focused {
            vec![
                BoxShadow::new(
                    px(0.),
                    px(6.),
                    hsla(0., 0., 0., if palette.is_dark() { 0.38 } else { 0.08 }),
                )
                .blur_radius(px(18.)),
                BoxShadow::new(px(0.), px(0.), gpui_kit::rgb(palette.accent).into())
                    .blur_radius(px(0.))
                    .spread_radius(px(0.5)),
            ]
        } else {
            card_shadow(palette.is_dark())
        };

        div()
            .id("folio-ai-pane")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(gpui_kit::rgb(palette.background))
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
                    if pane.session_drawer_open {
                        pane.session_drawer_open = false;
                        pane.renaming_session = None;
                        changed = true;
                    }
                    if pane.model_popover_open {
                        pane.model_popover_open = false;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .h(px(36.))
                    .px_2p5()
                    .flex_shrink_0()
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_b_1()
                    .border_color(subtle_border(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .child(
                        div()
                            .id("ai-sessions-toggle")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .h(px(26.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .min_w_0()
                            .when(self.session_drawer_open, |el| {
                                el.bg(gpui_kit::rgb(palette.hover))
                            })
                            .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.session_drawer_open = !pane.session_drawer_open;
                                if pane.session_drawer_open {
                                    pane.history_anim_gen = pane.history_anim_gen.wrapping_add(1);
                                    pane.refresh_sessions();
                                } else {
                                    pane.renaming_session = None;
                                }
                                cx.notify();
                            }))
                            .child(
                                crate::ui::icon(IconName::Clock)
                                    .w(px(13.))
                                    .h(px(13.))
                                    .text_color(gpui_kit::rgb(if self.session_drawer_open {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(typo.sm)
                                    .font_medium()
                                    .text_ellipsis()
                                    .child(session_label),
                            )
                            .child(
                                crate::ui::icon(if self.session_drawer_open {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .w(px(12.))
                                .h(px(12.))
                                .text_color(gpui_kit::rgb(palette.muted)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_0p5()
                            .flex_shrink_0()
                            .child(
                                Button::new("ai-new-session")
                                    .custom(subtle_btn(cx))
                                    .compact()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .icon(crate::ui::icon(IconName::Plus))
                                    .tooltip("新建会话")
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.new_session(cx);
                                    })),
                            )
                            .child(
                                Button::new("ai-maximize")
                                    .custom(subtle_btn(cx))
                                    .compact()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .icon(crate::ui::icon(IconName::Maximize2))
                                    .tooltip("最大化 / 还原")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(AiPaneEvent::ToggleMaximize);
                                    })),
                            )
                            .child(
                                Button::new("ai-hide")
                                    .custom(subtle_btn(cx))
                                    .compact()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .icon(crate::ui::icon(IconName::PanelRightOpen))
                                    .tooltip("收起")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(AiPaneEvent::Hide);
                                    })),
                            ),
                    ),
            )
            .when(self.session_drawer_open, |root| {
                root.child(
                    deferred(
                        div()
                            .absolute()
                            .top(px(40.))
                            .left_2()
                            .child(self.render_sessions_popover(
                                palette,
                                ui_font.clone(),
                                cx,
                            )),
                    )
                    .with_priority(4),
                )
            })
            .child({
                if self.is_streaming && self.stick_to_bottom {
                    self.scroll.scroll_to_bottom();
                }
                let thumb_bg = if palette.is_dark() { 0x45474b } else { 0xd9d9d7 };
                let thumb_hover = if palette.is_dark() { 0x5c5f66 } else { 0xbcbcb9 };
                let thumb_active = if palette.is_dark() { 0x72767f } else { 0xa8a8a5 };

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
                            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, _| {
                                let delta_y = f32::from(event.delta.pixel_delta(px(20.)).y);
                                let max_y = f32::from(pane.scroll.max_offset().y);
                                let cur_y = -f32::from(pane.scroll.offset().y);
                                if delta_y > 0.5 {
                                    pane.stick_to_bottom = false;
                                } else if cur_y >= (max_y - 28.0).max(0.0) {
                                    pane.stick_to_bottom = true;
                                }
                            }))
                            .px_3p5()
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
                    .when(
                        self.messages.is_empty() && self.startup_error.is_none(),
                        |stream| {
                            let badge_bg = mix_color(
                                palette.surface,
                                palette.accent,
                                if palette.is_dark() { 0.16 } else { 0.08 },
                            );
                            let badge_border = mix_color(
                                palette.border,
                                palette.accent,
                                if palette.is_dark() { 0.35 } else { 0.22 },
                            );
                            stream.child(
                                div()
                                    .flex_1()
                                    .min_h(px(240.))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_2p5()
                                    .child(
                                        div()
                                            .size(px(42.))
                                            .rounded(px(13.))
                                            .bg(badge_bg)
                                            .border_1()
                                            .border_color(badge_border)
                                            .shadow(card_shadow(palette.is_dark()))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                crate::ui::icon(IconName::Sparkles)
                                                    .w(px(20.))
                                                    .h(px(20.))
                                                    .text_color(gpui_kit::rgb(palette.accent)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(typo.body)
                                            .font_semibold()
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .child(workspace_title.clone()),
                                    ),
                            )
                        },
                    )
                    .children(self.messages.iter().enumerate().map(|(msg_idx, msg)| {
                        match msg {
                            ChatMessage::User {
                                display_text,
                                attached_file,
                                mounted_skills,
                                quote,
                                images,
                            } => div()
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
                                        .when_some(quote.as_ref(), |b, q| {
                                            b.child(
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
                                                    )),
                                            )
                                        })
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
                            ChatMessage::Assistant {
                                parts,
                                error,
                                streaming,
                            } => {
                                let mut rendered_parts: Vec<AnyElement> = Vec::new();
                                let mut idx = 0usize;
                                let think_pill_bg =
                                    mix_color(palette.background, palette.surface, 0.72);
                                while idx < parts.len() {
                                    match &parts[idx] {
                                        AssistantPart::Thinking { text, collapsed } => {
                                            let is_collapsed = *collapsed;
                                            let part_idx = idx;
                                            rendered_parts.push(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div().flex().items_center().child(
                                                            div()
                                                                .id((
                                                                    "ai-think-toggle",
                                                                    msg_idx * 1000 + part_idx,
                                                                ))
                                                                .px_2p5()
                                                                .py_1()
                                                                .rounded(px(8.))
                                                                .bg(think_pill_bg)
                                                                .border_1()
                                                                .border_color(subtle_border(
                                                                    palette.border,
                                                                ))
                                                                .flex()
                                                                .items_center()
                                                                .gap_1p5()
                                                                .cursor_pointer()
                                                                .text_size(typo.sm)
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.muted,
                                                                ))
                                                                .hover(move |s| {
                                                                    s.bg(gpui_kit::rgb(
                                                                        palette.hover,
                                                                    ))
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.text,
                                                                    ))
                                                                })
                                                                .on_click(cx.listener(
                                                                    move |pane, _, _, cx| {
                                                                        if let Some(
                                                                            ChatMessage::Assistant {
                                                                                parts,
                                                                                ..
                                                                            },
                                                                        ) = pane
                                                                            .messages
                                                                            .get_mut(msg_idx)
                                                                            && let Some(
                                                                                AssistantPart::Thinking {
                                                                                    collapsed,
                                                                                    ..
                                                                                },
                                                                            ) = parts
                                                                                .get_mut(part_idx)
                                                                        {
                                                                            *collapsed =
                                                                                !*collapsed;
                                                                            cx.notify();
                                                                        }
                                                                    },
                                                                ))
                                                                .child(
                                                                    crate::ui::icon(IconName::Brain)
                                                                        .w(px(12.))
                                                                        .h(px(12.))
                                                                        .text_color(gpui_kit::rgb(
                                                                            palette.accent,
                                                                        )),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .font_medium()
                                                                        .child("思考过程"),
                                                                )
                                                                .child(
                                                                    crate::ui::icon(
                                                                        if is_collapsed {
                                                                            IconName::ChevronRight
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        },
                                                                    )
                                                                    .w(px(12.))
                                                                    .h(px(12.)),
                                                                ),
                                                        ),
                                                    )
                                                    .when(
                                                        !is_collapsed && !text.is_empty(),
                                                        |box_| {
                                                            box_.child(
                                                                div()
                                                                    .ml_2p5()
                                                                    .pl_3()
                                                                    .border_l_2()
                                                                    .border_color(subtle_border(
                                                                        palette.border,
                                                                    ))
                                                                    .text_size(typo.sm)
                                                                    .line_height(relative(1.5))
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.muted,
                                                                    ))
                                                                    .child(text.clone()),
                                                            )
                                                        },
                                                    )
                                                    .into_any_element(),
                                            );
                                            idx += 1;
                                        }
                                        AssistantPart::Text(text) => {
                                            let part_idx = idx;
                                            let normalized = markdown_editor::normalize_math(text);
                                            rendered_parts.push(
                                                div()
                                                    .font_family(ui_font.clone())
                                                    .text_size(typo.body)
                                                    .line_height(relative(1.6))
                                                    .child(
                                                        TextView::markdown(
                                                            SharedString::from(format!(
                                                                "ai-assistant-md-{msg_idx}-{part_idx}"
                                                            )),
                                                            normalized,
                                                        )
                                                        .selectable(true)
                                                            .markdown_extensions(
                                                                self.md_extensions.clone(),
                                                            )
                                                            .style(md_style.clone())
                                                            .font_family(ui_font.clone())
                                                            .text_size(typo.body)
                                                            .code_block_actions(move |block, _, _| {
                                                                let code = block.code().to_string();
                                                                let lang = block
                                                                    .lang()
                                                                    .filter(|l| !l.is_empty());
                                                                let span_start = block
                                                                    .span
                                                                    .as_ref()
                                                                    .map(|s| s.start)
                                                                    .unwrap_or(0);
                                                                div()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_1()
                                                                    .when_some(lang, |row, lang| {
                                                                        row.child(
                                                                            div()
                                                                                .pl_1p5()
                                                                                .text_size(typo.xs)
                                                                                .text_color(
                                                                                    gpui_kit::rgb(
                                                                                        palette
                                                                                            .muted,
                                                                                    ),
                                                                                )
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
                                                                let span_start = table
                                                                    .span
                                                                    .as_ref()
                                                                    .map(|s| s.start)
                                                                    .unwrap_or(0);
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
                                                    .into_any_element(),
                                            );
                                            idx += 1;
                                        }
                                        AssistantPart::Tool(_) => {
                                            let start = idx;
                                            let mut group: Vec<(usize, &ToolBlock)> = Vec::new();
                                            while idx < parts.len() {
                                                if let AssistantPart::Tool(b) = &parts[idx] {
                                                    group.push((idx, b));
                                                    idx += 1;
                                                } else {
                                                    break;
                                                }
                                            }
                                            rendered_parts.push(self.render_tool_group(
                                                msg_idx,
                                                start,
                                                &group,
                                                palette,
                                                cx,
                                            ));
                                        }
                                    }
                                }

                                let full_answer = parts
                                    .iter()
                                    .filter_map(|p| match p {
                                        AssistantPart::Text(t) if !t.trim().is_empty() => {
                                            Some(t.as_str())
                                        }
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n\n");

                                let is_last_msg = msg_idx + 1 == total_msgs;
                                let any_tool_running = parts
                                    .iter()
                                    .any(|p| matches!(p, AssistantPart::Tool(b) if !b.done));

                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .children(rendered_parts)
                                    .when(
                                        !*streaming
                                            && !any_tool_running
                                            && !(is_last_msg && self.is_streaming)
                                            && !full_answer.is_empty(),
                                        |box_| {
                                        box_.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_start()
                                                .pt_0p5()
                                                .child(ai_copy_button(
                                                    SharedString::from(format!(
                                                        "ai-copy-msg-{msg_idx}"
                                                    )),
                                                    "复制回答",
                                                    "已复制",
                                                    full_answer,
                                                )),
                                        )
                                    })
                                    .when(*streaming && parts.is_empty(), |box_| {
                                        box_.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1p5()
                                                .py_1()
                                                .child(
                                                    div()
                                                        .size(px(7.))
                                                        .rounded_full()
                                                        .bg(gpui_kit::rgb(palette.accent))
                                                        .with_animation(
                                                            ("ai-stream-pulse", msg_idx),
                                                            Animation::new(Duration::from_millis(
                                                                950,
                                                            ))
                                                            .repeat(),
                                                            |el, t| {
                                                                let a = 0.3
                                                                    + 0.7
                                                                        * (0.5
                                                                            - 0.5
                                                                                * (t
                                                                                    * std::f32::consts::TAU)
                                                                                    .cos());
                                                                el.opacity(a)
                                                            },
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(typo.sm)
                                                        .text_color(gpui_kit::rgb(palette.muted))
                                                        .child("思考中…"),
                                                ),
                                        )
                                    })
                                    .when_some(error.clone(), |box_, err| {
                                        box_.child(
                                            div()
                                                .p_2p5()
                                                .rounded(px(10.))
                                                .bg(gpui_kit::rgb(palette.surface))
                                                .border_1()
                                                .border_color(gpui_kit::rgb(palette.danger))
                                                .text_size(typo.sm)
                                                .text_color(gpui_kit::rgb(palette.danger))
                                                .child(err),
                                        )
                                    })
                                    .into_any_element()
                            }
                            ChatMessage::Notice { text, is_error } => div()
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
                        }
                    })),
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
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .px_3()
                    .pb_3()
                    .pt_1p5()
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
                    .capture_action::<IndentInline>(cx.listener(
                        |pane, _: &IndentInline, window, cx| {
                            let items = pane.current_slash_items(cx);
                            if !items.is_empty() {
                                let chosen =
                                    items[pane.slash_index.min(items.len() - 1)].clone();
                                cx.stop_propagation();
                                pane.execute_slash_item(chosen, window, cx);
                            }
                        },
                    ))
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
                            pane.dismissed_slash_text =
                                Some(pane.input.read(cx).value().to_string());
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
                                        .left_3()
                                        .right_3()
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
                        div()
                            .rounded(px(16.))
                            .border_1()
                            .border_color(composer_border)
                            .bg(gpui_kit::rgb(palette.surface))
                            .shadow(composer_shadow)
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
                                                            .on_click(cx.listener(
                                                                move |_, _, _, cx| {
                                                                    cx.emit(
                                                                        AiPaneEvent::OpenFile(
                                                                            open_path.clone(),
                                                                        ),
                                                                    );
                                                                },
                                                            ))
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
                                                            .text_color(gpui_kit::rgb(
                                                                palette.muted,
                                                            ))
                                                            .on_click(cx.listener(
                                                                |pane, _, _, cx| {
                                                                    pane.attach_active_file = false;
                                                                    cx.notify();
                                                                },
                                                            ))
                                                            .child(
                                                                crate::ui::icon(IconName::Close)
                                                                    .w(px(11.))
                                                                    .h(px(11.)),
                                                            ),
                                                    ),
                                            )
                                        })
                                        .when_some(self.quoted_selection.as_ref(), |row, q| {
                                            row.child(
                                                div()
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
                                                            .id("ai-clear-quote")
                                                            .cursor_pointer()
                                                            .on_click(cx.listener(
                                                                |pane, _, _, cx| {
                                                                    pane.quoted_selection = None;
                                                                    cx.notify();
                                                                },
                                                            ))
                                                            .child(
                                                                crate::ui::icon(IconName::Close)
                                                                    .w(px(11.))
                                                                    .h(px(11.)),
                                                            ),
                                                    ),
                                            )
                                        })
                                        .children(self.pasted_images.iter().enumerate().map(
                                            |(idx, p)| {
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
                                                            .on_click(cx.listener(
                                                                move |pane, _, _, cx| {
                                                                    pane.pasted_images
                                                                        .retain(|im| im.id != rm_id);
                                                                    cx.notify();
                                                                },
                                                            ))
                                                            .child(
                                                                crate::ui::icon(IconName::Close)
                                                                    .w(px(11.))
                                                                    .h(px(11.)),
                                                            ),
                                                    )
                                            },
                                        )),
                                )
                            })
                            .child(
                                Textarea::new(&self.input)
                                    .appearance(false)
                                    .bordered(false)
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
                                                    .on_click(cx.listener(
                                                        |pane, _, window, cx| {
                                                            pane.pick_files(window, cx);
                                                        },
                                                    )),
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
                                                        Tooltip::new(ctx_tip.clone())
                                                            .build(window, cx)
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
                                                            .text_color(gpui_kit::rgb(
                                                                if active_ctx {
                                                                    palette.accent
                                                                } else {
                                                                    palette.muted
                                                                },
                                                            )),
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
                                                    .hover(move |s| {
                                                        s.bg(gpui_kit::rgb(palette.hover))
                                                    })
                                                    .tooltip(|window, cx| {
                                                        Tooltip::new("切换 Shell 命令确认权限")
                                                            .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.auto_approve_shell =
                                                            !pane.auto_approve_shell;
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        crate::ui::icon(
                                                            if self.auto_approve_shell {
                                                                IconName::ShieldAlert
                                                            } else {
                                                                IconName::Shield
                                                            },
                                                        )
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
                                                let pill_bg = if open {
                                                    gpui_kit::rgb(palette.hover)
                                                } else {
                                                    mix_color(
                                                        palette.background,
                                                        palette.surface,
                                                        0.5,
                                                    )
                                                };
                                                div()
                                                    .id("ai-model-effort-pill")
                                                    .h(px(26.))
                                                    .px_2()
                                                    .rounded(px(8.))
                                                    .bg(pill_bg)
                                                    .border_1()
                                                    .border_color(if open {
                                                        mix_color(
                                                            palette.border,
                                                            palette.accent,
                                                            0.5,
                                                        )
                                                    } else {
                                                        subtle_border(palette.border)
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .cursor_pointer()
                                                    .text_size(typo.sm)
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .hover(move |s| {
                                                        s.bg(gpui_kit::rgb(palette.hover))
                                                    })
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(
                                                        |pane, _, window, cx| {
                                                            pane.model_popover_open =
                                                                !pane.model_popover_open;
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
                                                        },
                                                    ))
                                                    .child(render_model_icon(
                                                        selected_model,
                                                        px(13.),
                                                        palette.muted,
                                                    ))
                                                    .child(
                                                        div()
                                                            .max_w(px(160.))
                                                            .text_ellipsis()
                                                            .font_medium()
                                                            .child(model_effort_label),
                                                    )
                                                    .child(
                                                        crate::ui::icon(if open {
                                                            IconName::ChevronUp
                                                        } else {
                                                            IconName::ChevronDown
                                                        })
                                                        .w(px(12.))
                                                        .h(px(12.))
                                                        .text_color(gpui_kit::rgb(palette.muted)),
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
                                                    .on_click(cx.listener(
                                                        |pane, _, window, cx| {
                                                            pane.send_prompt(window, cx);
                                                        },
                                                    ))
                                                    .child(
                                                        crate::ui::icon(IconName::ArrowUp)
                                                            .w(px(14.))
                                                            .h(px(14.)),
                                                    )
                                                    .into_any_element()
                                            }),
                                    ),
                            ),
                    ),
            )
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
            || s.contains("tencent")
            || s == "hy"
            || s.starts_with("hy-")
            || s.starts_with("hy_")
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
        let feedback = window.use_keyed_state(self.id.clone(), cx, |_, _| AiCopyFeedback {
            copied: false,
            reset: Task::ready(()),
        });
        let copied = feedback.read(cx).copied;
        let text = self.text;
        Button::new(self.id)
            .ghost()
            .compact()
            .w(px(24.))
            .h(px(24.))
            .icon(
                crate::ui::icon(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .w(px(12.))
                .h(px(12.)),
            )
            .tooltip(if copied {
                self.copied_tip
            } else {
                self.idle_tip
            })
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
        _ => name.to_owned(),
    }
}

fn build_diffs_for_tool(name: &str, args: &Value, write_before: Option<&str>) -> Vec<FileDiff> {
    let path = extract_path_arg(args).unwrap_or_else(|| "file".to_owned());
    match name {
        "edit" => {
            let mut out = Vec::new();
            if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
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
                        out.push(compute_diff(path.clone(), old_t, new_t));
                    }
                }
            } else {
                let old_t = args
                    .get("oldText")
                    .or_else(|| args.get("old_text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new_t = args
                    .get("newText")
                    .or_else(|| args.get("new_text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !old_t.is_empty() || !new_t.is_empty() {
                    out.push(compute_diff(path, old_t, new_t));
                }
            }
            out
        }
        "write" => {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.is_empty() && write_before.is_none() {
                return Vec::new();
            }
            vec![compute_diff(path, write_before.unwrap_or(""), content)]
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

fn split_injected_prompt(
    raw: &str,
) -> (Option<String>, Vec<String>, Option<SelectionQuote>, String) {
    let mut lines = raw.lines().peekable();
    let mut skills = Vec::new();
    while let Some(first) = lines.peek() {
        if let Some(sk) = first.strip_prefix("/skill:") {
            let trimmed = sk.trim();
            if !trimmed.is_empty() && !trimmed.contains(' ') {
                skills.push(format!("skill:{trimmed}"));
                lines.next();
                continue;
            }
        }
        break;
    }
    while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
        lines.next();
    }
    let mut attached_file = None;
    if let Some(first) = lines.peek()
        && let Some(rest) = first.strip_prefix('@')
        && !rest.trim().is_empty()
        && !rest.contains(' ')
    {
        attached_file = Some(rest.trim().to_owned());
        lines.next();
        while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
            lines.next();
        }
    }
    let mut quote = None;
    if let Some(first) = lines.peek()
        && let Some(src) = first.strip_prefix("> 引用自 ")
    {
        let source = src.trim().to_owned();
        lines.next();
        let mut q_lines = Vec::new();
        while let Some(l) = lines.peek() {
            if let Some(body) = l.strip_prefix("> ") {
                q_lines.push(body.to_owned());
                lines.next();
            } else if *l == ">" {
                q_lines.push(String::new());
                lines.next();
            } else {
                break;
            }
        }
        while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
            lines.next();
        }
        quote = Some(SelectionQuote {
            source,
            text: q_lines.join("\n"),
        });
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    (attached_file, skills, quote, body)
}
