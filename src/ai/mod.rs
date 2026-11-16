//! Native GPUI AI interface backed by `pi --mode rpc`.
mod diff;
mod rpc;
mod sessions;

use crate::model::AppSettings;
use crate::theme::{Palette, color as theme_color, palette as active_palette};
use diff::{DiffLine, FileDiff, compute_diff};
use gpui::{MouseButton, ScrollHandle, Subscription};
use gpui_kit::{
    assets::IconName,
    base::{
        StyledExt,
        text::{TextViewStyle, markdown as render_markdown},
    },
    component::{
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{DropdownMenu, PopupMenuItem},
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use rpc::{RpcClient, RpcInbound};
use serde_json::Value;
use sessions::SessionSummary;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
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

#[derive(Clone, Debug)]
struct McpServerEntry {
    name: String,
    transport: String,
    enabled: bool,
}

#[derive(Clone, Debug)]
struct ToolBlock {
    id: String,
    name: String,
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

#[derive(Clone, Debug)]
enum ChatMessage {
    User {
        display_text: String,
        attached_file: Option<String>,
        mounted_skills: Vec<String>,
        quote: Option<SelectionQuote>,
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
    Compact,
    McpStatus,
    Fork,
    MountSkill(String),
    InsertCommand(String),
}

#[derive(Clone, Debug)]
struct SlashItem {
    id: String,
    title: String,
    subtitle: String,
    icon: IconName,
    action: SlashAction,
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
    mounted_skills: Vec<String>,
    active_file: Option<AttachedFile>,
    detached_for_file: Option<PathBuf>,
    quoted_selection: Option<SelectionQuote>,
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
    plus_menu_open: bool,
    mcp_status_open: bool,
    mcp_servers: Vec<McpServerEntry>,
    slash_index: usize,
    dismissed_slash_text: Option<String>,
    expanded_tool_groups: HashSet<(usize, usize)>,
    write_snapshots: HashMap<String, String>,
    active_part_map: HashMap<usize, usize>,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl AiPane {
    pub fn new(
        cwd: PathBuf,
        settings: AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("随心输入，/ 呼出命令…")
                .submit_on_enter(true)
        });
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入会话名称"));
        let ext_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入内容…"));
        let ext_textarea = cx.new(|cx| TextareaState::new(window, cx).placeholder("编辑文本…"));

        let sub_input = cx.subscribe_in(
            &input,
            window,
            |pane, state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let val = state.read(cx).value().to_string();
                    if pane.dismissed_slash_text.as_deref() != Some(val.as_str()) {
                        pane.dismissed_slash_text = None;
                    }
                    pane.slash_index = 0;
                    if !val.is_empty() {
                        pane.plus_menu_open = false;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { shift: false, .. } => {
                    if !pane.slash_items().is_empty() && pane.slash_query(cx).is_some() {
                        return;
                    }
                    pane.send_prompt(window, cx);
                }
                _ => {}
            },
        );
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
            mounted_skills: Vec::new(),
            active_file: None,
            detached_for_file: None,
            quoted_selection: None,
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
            plus_menu_open: false,
            mcp_status_open: false,
            mcp_servers: Vec::new(),
            slash_index: 0,
            dismissed_slash_text: None,
            expanded_tool_groups: HashSet::new(),
            write_snapshots: HashMap::new(),
            active_part_map: HashMap::new(),
            input,
            scroll: ScrollHandle::new(),
            _subscriptions: vec![sub_input, sub_rename, sub_ext],
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
        self.active_file = file;
        self.detached_for_file = None;
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
            self.detached_for_file = None;
        }
        self.open(window, cx);
    }

    fn effective_attached_file(&self) -> Option<&AttachedFile> {
        let file = self.active_file.as_ref()?;
        if self.detached_for_file.as_ref() == Some(&file.path) {
            None
        } else {
            Some(file)
        }
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

                if let Some(block) = self.find_tool_mut(call_id) {
                    block.done = true;
                    block.is_error = is_err;
                    block.duration_ms = duration_ms;
                    if !output.is_empty() {
                        block.output = output;
                    }
                    if name == "write" && !is_err {
                        block.diffs = build_diffs_for_tool(
                            &block.name,
                            &Value::Null,
                            write_before.as_deref(),
                        );
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
            }
            "thinking_end" => {
                if let Some(content) = ame.get("content").and_then(|v| v.as_str())
                    && let Some(&idx) = self.active_part_map.get(&content_index)
                    && let Some(ChatMessage::Assistant { parts, .. }) = self.messages.last_mut()
                    && let Some(AssistantPart::Thinking { text, .. }) = parts.get_mut(idx)
                {
                    *text = content.to_owned();
                }
            }
            "text_start" => {
                let idx = self.push_assistant_part(AssistantPart::Text(String::new()));
                self.active_part_map.insert(content_index, idx);
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
                        block.summary = summary;
                        if !diffs.is_empty() {
                            block.diffs = diffs;
                        }
                    }
                }
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
            block.summary = summary;
            if !diffs.is_empty() {
                block.diffs = diffs;
            }
            return;
        }
        self.push_assistant_part(AssistantPart::Tool(ToolBlock {
            id: call_id.to_owned(),
            name: name.to_owned(),
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

    fn slash_query(&self, cx: &App) -> Option<String> {
        let val = self.input.read(cx).value().to_string();
        if self.dismissed_slash_text.as_deref() == Some(val.as_str()) {
            return None;
        }
        let first_line = val.lines().next()?;
        if !val.contains('\n') && first_line.starts_with('/') && !first_line.contains(' ') {
            Some(first_line[1..].to_ascii_lowercase())
        } else {
            None
        }
    }

    fn slash_items(&self) -> Vec<SlashItem> {
        let pct_label = self
            .context_usage
            .as_ref()
            .and_then(|cu| cu.percent)
            .map(|p| format!("压缩此聊天的上下文（已使用 {p}%）"))
            .unwrap_or_else(|| "压缩此聊天的上下文".to_owned());

        let mut items = vec![
            SlashItem {
                id: "compact".into(),
                title: "压缩".into(),
                subtitle: pct_label,
                icon: IconName::CircleDot,
                action: SlashAction::Compact,
            },
            SlashItem {
                id: "mcp".into(),
                title: "MCP".into(),
                subtitle: "显示 MCP 服务器状态".into(),
                icon: IconName::Paperclip,
                action: SlashAction::McpStatus,
            },
            SlashItem {
                id: "fork".into(),
                title: "创建聊天分支".into(),
                subtitle: "从此对话复制新分支".into(),
                icon: IconName::GitBranch,
                action: SlashAction::Fork,
            },
        ];

        for cmd in &self.commands {
            let is_skill = cmd.source == "skill";
            let display_name = if is_skill {
                cmd.name.strip_prefix("skill:").unwrap_or(&cmd.name).to_owned()
            } else {
                format!("/{}", cmd.name)
            };
            let action = if is_skill {
                SlashAction::MountSkill(cmd.name.clone())
            } else {
                SlashAction::InsertCommand(format!("/{} ", cmd.name))
            };
            items.push(SlashItem {
                id: cmd.name.clone(),
                title: display_name,
                subtitle: if cmd.description.is_empty() {
                    cmd.source.clone()
                } else {
                    cmd.description.clone()
                },
                icon: if is_skill {
                    IconName::Sparkles
                } else {
                    IconName::Terminal
                },
                action,
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
        match item.action {
            SlashAction::Compact => {
                self.input.update(cx, |input, cx| input.set_value(String::new(), window, cx));
                self.compact_context(cx);
            }
            SlashAction::McpStatus => {
                self.input.update(cx, |input, cx| input.set_value(String::new(), window, cx));
                self.refresh_mcp_status();
                self.mcp_status_open = true;
                cx.notify();
            }
            SlashAction::Fork => {
                self.input.update(cx, |input, cx| input.set_value(String::new(), window, cx));
                if let Some(client) = &self.client {
                    let _ = client.clone_session();
                }
            }
            SlashAction::MountSkill(skill) => {
                if !self.mounted_skills.contains(&skill) {
                    self.mounted_skills.push(skill);
                }
                self.input.update(cx, |input, cx| {
                    input.set_value(String::new(), window, cx);
                    input.focus(window, cx);
                });
            }
            SlashAction::InsertCommand(cmd) => {
                self.input.update(cx, |input, cx| {
                    input.set_value(cmd, window, cx);
                    input.focus(window, cx);
                });
            }
        }
        cx.notify();
    }

    fn refresh_mcp_status(&mut self) {
        let mut out = Vec::new();
        let mut paths = Vec::new();
        if let Some(agent_dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
            paths.push(PathBuf::from(agent_dir).join("mcp.json"));
        } else if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".pi").join("agent").join("mcp.json"));
        }
        paths.push(self.cwd.join(".pi").join("mcp.json"));

        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let servers = json
                .get("mcpServers")
                .or_else(|| json.get("servers"))
                .and_then(|v| v.as_object());
            if let Some(map) = servers {
                for (name, cfg) in map {
                    let enabled = !cfg
                        .get("disabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let transport = if cfg.get("url").is_some() {
                        "HTTP/SSE".to_owned()
                    } else if let Some(cmd) = cfg.get("command").and_then(|v| v.as_str()) {
                        cmd.to_owned()
                    } else {
                        "stdio".to_owned()
                    };
                    out.push(McpServerEntry {
                        name: name.clone(),
                        transport,
                        enabled,
                    });
                }
            }
        }
        self.mcp_servers = out;
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_streaming {
            return;
        }
        let raw_text = self.input.read(cx).value().to_string();
        let trimmed = raw_text.trim();
        if trimmed.is_empty() && self.quoted_selection.is_none() && self.mounted_skills.is_empty() {
            return;
        }

        match trimmed {
            "/compact" => {
                self.input.update(cx, |i, cx| i.set_value(String::new(), window, cx));
                self.compact_context(cx);
                return;
            }
            "/mcp" => {
                self.input.update(cx, |i, cx| i.set_value(String::new(), window, cx));
                self.refresh_mcp_status();
                self.mcp_status_open = true;
                cx.notify();
                return;
            }
            "/fork" => {
                self.input.update(cx, |i, cx| i.set_value(String::new(), window, cx));
                if let Some(client) = &self.client {
                    let _ = client.clone_session();
                }
                return;
            }
            _ => {}
        }

        if self.client.is_none() {
            self.start_rpc(cx);
        }
        let Some(client) = &self.client else {
            return;
        };

        let attached = self.effective_attached_file().cloned();
        let quote = self.quoted_selection.take();
        let skills = std::mem::take(&mut self.mounted_skills);

        let mut payload = String::new();
        for sk in &skills {
            payload.push('/');
            payload.push_str(sk);
            payload.push('\n');
        }
        if let Some(file) = &attached
            && !trimmed.starts_with('/')
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
        payload.push_str(trimmed);

        if let Err(err) = client.prompt(&payload) {
            self.show_toast(format!("发送失败：{err}"), true, cx);
            return;
        }

        self.messages.push(ChatMessage::User {
            display_text: trimmed.to_owned(),
            attached_file: attached.map(|f| f.display),
            mounted_skills: skills,
            quote,
        });
        self.input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.is_streaming = true;
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

    fn switch_session(&mut self, path: &Path, _cx: &mut Context<Self>) {
        if let Some(client) = &self.client {
            let _ = client.switch_session(&path.to_string_lossy());
        }
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
        self.scroll.scroll_to_bottom();
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
                                    .text_xs()
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
                            .text_xs()
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
                    .text_xs()
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

        div()
            .my_0p5()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id(("tool-group", msg_idx * 1000 + start_idx))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .py_0p5()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(gpui_kit::rgb(palette.muted))
                    .hover(move |s| s.text_color(gpui_kit::rgb(palette.text)))
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if !pane.expanded_tool_groups.remove(&group_key) {
                            pane.expanded_tool_groups.insert(group_key);
                        }
                        cx.notify();
                    }))
                    .child(crate::ui::icon(header_icon).w(px(12.)).h(px(12.)))
                    .child(group_label)
                    .child(
                        crate::ui::icon(if group_expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .w(px(12.))
                        .h(px(12.)),
                    ),
            )
            .when(group_expanded, |col| {
                col.child(
                    div()
                        .ml_1p5()
                        .pl_2p5()
                        .border_l_1()
                        .border_color(gpui_kit::rgb(palette.border))
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
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .py_0p5()
                                        .when(has_detail, |row| row.cursor_pointer())
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
                                                .text_xs()
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
                                                    .text_size(px(11.))
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
                                                .id(("tool-output", msg_idx * 1000 + part_idx))
                                                .p_2()
                                                .max_h(px(180.))
                                                .overflow_y_scroll()
                                                .rounded(px(5.))
                                                .border_1()
                                                .border_color(gpui_kit::rgb(palette.border))
                                                .bg(gpui_kit::rgb(palette.code_background))
                                                .font_family("Consolas")
                                                .text_xs()
                                                .text_color(gpui_kit::rgb(palette.text))
                                                .child(block.output.clone()),
                                        )
                                    })
                                })
                                .into_any_element()
                        })),
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
                    .text_xs()
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
                            .text_xs()
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
                                .text_xs()
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

    fn render_sessions_drawer(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().font_semibold().child("历史会话"))
                    .child(
                        Button::new("ai-sessions-back")
                            .ghost()
                            .compact()
                            .label("返回")
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.session_drawer_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("ai-sessions-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(self.sessions.is_empty(), |list| {
                        list.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(gpui_kit::rgb(palette.muted))
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
                            .rounded(px(6.))
                            .border_1()
                            .border_color(gpui_kit::rgb(if is_active {
                                palette.accent
                            } else {
                                palette.border
                            }))
                            .when(is_active, |row| row.bg(gpui_kit::rgb(palette.hover)))
                            .hover(move |st| st.bg(gpui_kit::rgb(palette.hover)))
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
                                            .text_xs()
                                            .font_medium()
                                            .text_ellipsis()
                                            .child(current_title.clone())
                                            .into_any_element()
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .flex_shrink_0()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .child(
                                                div()
                                                    .id(("ai-sess-ren", idx))
                                                    .size(px(22.))
                                                    .rounded(px(4.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .hover(move |st| {
                                                        st.bg(gpui_kit::rgb(palette.pressed))
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
                                                    .rounded(px(4.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .hover(move |st| {
                                                        st.bg(gpui_kit::rgb(palette.pressed))
                                                    })
                                                    .child(
                                                        crate::ui::icon(IconName::Trash)
                                                            .w(px(12.))
                                                            .h(px(12.)),
                                                    )
                                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                                        pane.delete_session(
                                                            delete_path.clone(),
                                                            cx,
                                                        );
                                                        cx.stop_propagation();
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_0p5()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(11.))
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child(format!("{} 条", s.message_count))
                                    .child(s.timestamp_label.clone()),
                            )
                    })),
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = items.len();
        let active_idx = self.slash_index.min(count.saturating_sub(1));
        div()
            .id("ai-slash-popover")
            .w_full()
            .max_h(px(260.))
            .overflow_y_scroll()
            .rounded(px(10.))
            .border_1()
            .border_color(gpui_kit::rgb(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow_lg()
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                let selected = idx == active_idx;
                let item_click = item.clone();
                div()
                    .id(("ai-slash-item", idx))
                    .px_2p5()
                    .py_1p5()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .when(selected, |row| row.bg(gpui_kit::rgb(palette.hover)))
                    .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
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
                                crate::ui::icon(item.icon)
                                    .w(px(13.))
                                    .h(px(13.))
                                    .text_color(gpui_kit::rgb(palette.muted)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .flex_shrink_0()
                                    .child(item.title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .text_ellipsis()
                                    .child(item.subtitle),
                            ),
                    )
            }))
            .into_any_element()
    }

    fn render_plus_popover(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let reattach = if self.effective_attached_file().is_none() {
            self.active_file.clone()
        } else {
            None
        };
        let skills: Vec<&PiCommandInfo> = self
            .commands
            .iter()
            .filter(|c| c.source == "skill")
            .collect();
        let has_skills = !skills.is_empty();

        div()
            .id("ai-plus-popover")
            .w(px(230.))
            .max_h(px(260.))
            .overflow_y_scroll()
            .rounded(px(10.))
            .border_1()
            .border_color(gpui_kit::rgb(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow_lg()
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when_some(reattach, |pop, file| {
                pop.child(
                    div()
                        .id("plus-reattach-file")
                        .px_2p5()
                        .py_1p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.detached_for_file = None;
                            pane.plus_menu_open = false;
                            cx.notify();
                        }))
                        .child(crate::ui::icon(IconName::FileText).w(px(13.)).h(px(13.)))
                        .child(format!("附加 @{}", file.display)),
                )
            })
            .when(has_skills, |pop| {
                pop.child(
                    div()
                        .px_2p5()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(gpui_kit::rgb(palette.muted))
                        .child("Skills"),
                )
                .children(skills.into_iter().enumerate().map(|(idx, sk)| {
                    let skill_name = sk.name.clone();
                    let label = sk.name.strip_prefix("skill:").unwrap_or(&sk.name).to_owned();
                    let mounted = self.mounted_skills.contains(&skill_name);
                    div()
                        .id(("plus-skill", idx))
                        .px_2p5()
                        .py_1p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .text_xs()
                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            if !pane.mounted_skills.contains(&skill_name) {
                                pane.mounted_skills.push(skill_name.clone());
                            }
                            pane.plus_menu_open = false;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    crate::ui::icon(IconName::Sparkles)
                                        .w(px(13.))
                                        .h(px(13.))
                                        .text_color(gpui_kit::rgb(palette.muted)),
                                )
                                .child(div().text_ellipsis().child(label)),
                        )
                        .when(mounted, |row| {
                            row.child(
                                crate::ui::icon(IconName::Check)
                                    .w(px(12.))
                                    .h(px(12.))
                                    .text_color(gpui_kit::rgb(palette.accent)),
                            )
                        })
                }))
            })
            .when(!has_skills && self.effective_attached_file().is_some(), |pop| {
                pop.child(
                    div()
                        .px_2p5()
                        .py_2()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .child("暂无可启用的 Skill"),
                )
            })
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

        let pane_entity = cx.entity();
        let current_model_id = self.current_model.as_ref().map(|m| m.id.clone());
        let model_label = self
            .current_model
            .as_ref()
            .map(|m| short_model_name(&m.name))
            .unwrap_or_else(|| "模型".to_owned());
        let models_for_menu = self.available_models.clone();

        let model_btn = Button::new("ai-model-select")
            .ghost()
            .compact()
            .label(model_label)
            .tooltip("切换模型")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let mut menu = menu.max_h(px(300.)).scrollable(true);
                if models_for_menu.is_empty() {
                    return menu.item(PopupMenuItem::label("暂无可用模型"));
                }
                for m in models_for_menu.iter().cloned() {
                    let pane = pane_entity.clone();
                    let checked = current_model_id.as_deref() == Some(m.id.as_str());
                    let provider = m.provider.clone();
                    let model_id = m.id.clone();
                    menu = menu.item(
                        PopupMenuItem::new(m.name.clone())
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                pane.update(cx, |pane, cx| {
                                    if let Some(client) = &pane.client {
                                        let _ = client.set_model(&provider, &model_id);
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });

        let supports_thinking = self.current_model.as_ref().is_none_or(|m| m.reasoning);
        let pane_think = cx.entity();
        let levels = self.thinking_levels.clone();
        let cur_think = self.current_thinking.clone();
        let think_btn = Button::new("ai-thinking-select")
            .ghost()
            .compact()
            .label(thinking_short_label(&self.current_thinking))
            .tooltip("推理深度")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let mut menu = menu;
                for lvl in levels.iter().cloned() {
                    let pane = pane_think.clone();
                    let checked = cur_think == lvl;
                    let lvl_set = lvl.clone();
                    menu = menu.item(
                        PopupMenuItem::new(thinking_short_label(&lvl))
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                pane.update(cx, |pane, cx| {
                                    pane.current_thinking = lvl_set.clone();
                                    if let Some(client) = &pane.client {
                                        let _ = client.set_thinking_level(&lvl_set);
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });

        let attached_chip = self.effective_attached_file().cloned();
        let has_chips = attached_chip.is_some()
            || self.quoted_selection.is_some()
            || !self.mounted_skills.is_empty();
        let slash_query = self.slash_query(cx);
        let slash_items = slash_query
            .as_deref()
            .map(|q| self.filtered_slash_items(q))
            .unwrap_or_default();
        let slash_items_for_keys = slash_items.clone();
        let has_slash_items = !slash_items.is_empty();
        let md_style = self.md_style(cx);
        let user_bubble_bg = gpui_kit::rgb(palette.background);
        let quote_bg = mix_color(palette.surface, palette.accent, 0.08);

        div()
            .id("folio-ai-pane")
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(gpui_kit::rgb(palette.surface))
            .text_color(gpui_kit::rgb(palette.text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, _, _, cx| {
                    if pane.plus_menu_open {
                        pane.plus_menu_open = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .h(px(34.))
                    .px_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
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
                            .rounded(px(5.))
                            .cursor_pointer()
                            .min_w_0()
                            .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.session_drawer_open = !pane.session_drawer_open;
                                if pane.session_drawer_open {
                                    pane.refresh_sessions();
                                }
                                cx.notify();
                            }))
                            .child(
                                crate::ui::icon(IconName::Clock)
                                    .w(px(13.))
                                    .h(px(13.))
                                    .text_color(gpui_kit::rgb(palette.muted)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_ellipsis()
                                    .child(session_label),
                            )
                            .child(
                                crate::ui::icon(IconName::ChevronDown)
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
                                    .ghost()
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
                                    .ghost()
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
                                    .ghost()
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
            .when_some(self.toast.clone(), |root, (msg, is_err)| {
                root.child(
                    div()
                        .mx_3()
                        .mt_1p5()
                        .px_2p5()
                        .py_1()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(gpui_kit::rgb(if is_err {
                            palette.danger
                        } else {
                            palette.border
                        }))
                        .bg(gpui_kit::rgb(palette.background))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .text_xs()
                        .text_color(gpui_kit::rgb(if is_err {
                            palette.danger
                        } else {
                            palette.muted
                        }))
                        .child(div().min_w_0().text_ellipsis().child(msg))
                        .child(
                            div()
                                .id("ai-toast-close")
                                .cursor_pointer()
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.toast = None;
                                    cx.notify();
                                }))
                                .child(crate::ui::icon(IconName::Close).w(px(11.)).h(px(11.))),
                        ),
                )
            })
            .when(self.mcp_status_open, |root| {
                root.child(
                    div()
                        .mx_3()
                        .mt_1p5()
                        .p_2p5()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(gpui_kit::rgb(palette.border))
                        .bg(gpui_kit::rgb(palette.background))
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .child("MCP 服务器状态"),
                                )
                                .child(
                                    div()
                                        .id("ai-mcp-close")
                                        .cursor_pointer()
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .on_click(cx.listener(|pane, _, _, cx| {
                                            pane.mcp_status_open = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            crate::ui::icon(IconName::Close)
                                                .w(px(12.))
                                                .h(px(12.)),
                                        ),
                                ),
                        )
                        .when(self.mcp_servers.is_empty(), |box_| {
                            box_.child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child("未在 mcp.json 中发现 MCP 服务器配置"),
                            )
                        })
                        .children(self.mcp_servers.iter().map(|srv| {
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .w(px(6.))
                                                .h(px(6.))
                                                .rounded_full()
                                                .bg(if srv.enabled {
                                                    theme_color(0x16a34a)
                                                } else {
                                                    gpui_kit::rgb(palette.muted)
                                                }),
                                        )
                                        .child(srv.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child(srv.transport.clone()),
                                )
                        })),
                )
            })
            .when(self.session_drawer_open, |root| {
                root.child(self.render_sessions_drawer(palette, cx))
            })
            .when(!self.session_drawer_open, |root| {
                root.child(
                    div()
                        .id("ai-message-stream")
                        .track_scroll(&self.scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_3()
                        .py_3()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .when_some(self.startup_error.clone(), |stream, err| {
                            stream.child(
                                div()
                                    .p_2p5()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(gpui_kit::rgb(palette.danger))
                                    .text_xs()
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
                                stream.child(
                                    div()
                                        .flex_1()
                                        .min_h(px(220.))
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            crate::ui::icon(IconName::Sparkles)
                                                .w(px(22.))
                                                .h(px(22.))
                                                .text_color(gpui_kit::rgb(palette.muted)),
                                        )
                                        .child(
                                            div()
                                                .text_base()
                                                .font_medium()
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
                                } => div()
                                    .px_3()
                                    .py_2()
                                    .rounded(px(10.))
                                    .bg(user_bubble_bg)
                                    .border_1()
                                    .border_color(gpui_kit::rgb(palette.border))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .when(
                                        attached_file.is_some() || !mounted_skills.is_empty(),
                                        |b| {
                                            b.child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .items_center()
                                                    .gap_1p5()
                                                    .when_some(attached_file.as_ref(), |r, f| {
                                                        r.child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_1()
                                                                .text_xs()
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
                                                    .children(mounted_skills.iter().map(|sk| {
                                                        let lbl = sk
                                                            .strip_prefix("skill:")
                                                            .unwrap_or(sk)
                                                            .to_owned();
                                                        div()
                                                            .px_1p5()
                                                            .py(px(1.))
                                                            .rounded(px(4.))
                                                            .bg(gpui_kit::rgb(palette.surface))
                                                            .border_1()
                                                            .border_color(gpui_kit::rgb(
                                                                palette.border,
                                                            ))
                                                            .text_size(px(11.))
                                                            .text_color(gpui_kit::rgb(palette.muted))
                                                            .child(lbl)
                                                    })),
                                            )
                                        },
                                    )
                                    .when_some(quote.as_ref(), |b, q| {
                                        b.child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .border_l_2()
                                                .border_color(gpui_kit::rgb(palette.accent))
                                                .text_xs()
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .child(format!(
                                                    "{}: {}",
                                                    q.source,
                                                    q.text.chars().take(160).collect::<String>()
                                                )),
                                        )
                                    })
                                    .when(!display_text.is_empty(), |b| {
                                        b.child(
                                            div()
                                                .text_sm()
                                                .line_height(relative(1.5))
                                                .child(display_text.clone()),
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
                                    while idx < parts.len() {
                                        match &parts[idx] {
                                            AssistantPart::Thinking { text, collapsed } => {
                                                let is_collapsed = *collapsed;
                                                let part_idx = idx;
                                                rendered_parts.push(
                                                    div()
                                                        .rounded(px(6.))
                                                        .px_2()
                                                        .py_1()
                                                        .bg(gpui_kit::rgb(palette.background))
                                                        .child(
                                                            div()
                                                                .id((
                                                                    "ai-think-toggle",
                                                                    msg_idx * 1000 + part_idx,
                                                                ))
                                                                .flex()
                                                                .items_center()
                                                                .gap_1()
                                                                .cursor_pointer()
                                                                .text_xs()
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.muted,
                                                                ))
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
                                                                    crate::ui::icon(
                                                                        if is_collapsed {
                                                                            IconName::ChevronRight
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        },
                                                                    )
                                                                    .w(px(12.))
                                                                    .h(px(12.)),
                                                                )
                                                                .child(
                                                                    crate::ui::icon(IconName::Brain)
                                                                        .w(px(12.))
                                                                        .h(px(12.)),
                                                                )
                                                                .child("思考过程"),
                                                        )
                                                        .when(
                                                            !is_collapsed && !text.is_empty(),
                                                            |box_| {
                                                                box_.child(
                                                                    div()
                                                                        .mt_1()
                                                                        .text_xs()
                                                                        .line_height(relative(1.45))
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
                                                rendered_parts.push(
                                                    div()
                                                        .text_sm()
                                                        .line_height(relative(1.55))
                                                        .child(
                                                            render_markdown(text.clone())
                                                                .style(md_style.clone()),
                                                        )
                                                        .into_any_element(),
                                                );
                                                idx += 1;
                                            }
                                            AssistantPart::Tool(_) => {
                                                let start = idx;
                                                let mut group: Vec<(usize, &ToolBlock)> =
                                                    Vec::new();
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

                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1p5()
                                        .children(rendered_parts)
                                        .when(*streaming && parts.is_empty(), |box_| {
                                            box_.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(gpui_kit::rgb(palette.muted))
                                                    .child("…"),
                                            )
                                        })
                                        .when_some(error.clone(), |box_, err| {
                                            box_.child(
                                                div()
                                                    .p_2()
                                                    .rounded(px(6.))
                                                    .border_1()
                                                    .border_color(gpui_kit::rgb(palette.danger))
                                                    .text_xs()
                                                    .text_color(gpui_kit::rgb(palette.danger))
                                                    .child(err),
                                            )
                                        })
                                        .into_any_element()
                                }
                                ChatMessage::Notice { text, is_error } => div()
                                    .px_2p5()
                                    .py_1p5()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(gpui_kit::rgb(if *is_error {
                                        palette.danger
                                    } else {
                                        palette.border
                                    }))
                                    .text_xs()
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
                .when_some(self.pending_ui.as_ref(), |root, pending| {
                    root.child(self.render_pending_ui(pending, palette, cx))
                })
                .when(!self.ext_widgets.is_empty(), |root| {
                    root.child(
                        div()
                            .mx_3()
                            .mb_1()
                            .p_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(gpui_kit::rgb(palette.border))
                            .text_xs()
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
                        .pt_1()
                        .capture_key_down(cx.listener(move |pane, event: &KeyDownEvent, window, cx| {
                            if slash_items_for_keys.is_empty() {
                                return;
                            }
                            let key = event.keystroke.key.to_ascii_lowercase();
                            let count = slash_items_for_keys.len();
                            match key.as_str() {
                                "up" => {
                                    pane.slash_index = (pane.slash_index + count - 1) % count;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                "down" => {
                                    pane.slash_index = (pane.slash_index + 1) % count;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                "escape" => {
                                    pane.dismissed_slash_text =
                                        Some(pane.input.read(cx).value().to_string());
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                "tab" | "enter" if !event.keystroke.modifiers.shift => {
                                    let chosen = slash_items_for_keys
                                        [pane.slash_index.min(count - 1)]
                                    .clone();
                                    pane.execute_slash_item(chosen, window, cx);
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        }))
                        .when(has_slash_items, |wrap| {
                            wrap.child(
                                deferred(
                                    div()
                                        .absolute()
                                        .left_3()
                                        .right_3()
                                        .bottom_full()
                                        .mb_1p5()
                                        .child(self.render_slash_popover(
                                            slash_items,
                                            palette,
                                            cx,
                                        )),
                                )
                                .with_priority(3),
                            )
                        })
                        .when(self.plus_menu_open && !has_slash_items, |wrap| {
                            wrap.child(
                                deferred(
                                    div()
                                        .absolute()
                                        .left_3()
                                        .bottom_full()
                                        .mb_1p5()
                                        .child(self.render_plus_popover(palette, cx)),
                                )
                                .with_priority(3),
                            )
                        })
                        .child(
                            div()
                                .rounded(px(14.))
                                .border_1()
                                .border_color(gpui_kit::rgb(palette.border))
                                .bg(gpui_kit::rgb(palette.surface))
                                .shadow_sm()
                                .px_2p5()
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
                                                let detach_path = file.path.clone();
                                                row.child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .px_2()
                                                        .py(px(2.))
                                                        .rounded(px(5.))
                                                        .bg(gpui_kit::rgb(palette.background))
                                                        .border_1()
                                                        .border_color(gpui_kit::rgb(palette.border))
                                                        .text_xs()
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
                                                                    crate::ui::icon(
                                                                        IconName::FileText,
                                                                    )
                                                                    .w(px(11.))
                                                                    .h(px(11.))
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.muted,
                                                                    )),
                                                                )
                                                                .child(format!(
                                                                    "@{}",
                                                                    file.display
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .id("ai-detach-file")
                                                                .cursor_pointer()
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.muted,
                                                                ))
                                                                .on_click(cx.listener(
                                                                    move |pane, _, _, cx| {
                                                                        pane.detached_for_file =
                                                                            Some(
                                                                                detach_path.clone(),
                                                                            );
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
                                            .children(self.mounted_skills.iter().enumerate().map(
                                                |(idx, sk)| {
                                                    let lbl = sk
                                                        .strip_prefix("skill:")
                                                        .unwrap_or(sk)
                                                        .to_owned();
                                                    let rm_sk = sk.clone();
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .px_2()
                                                        .py(px(2.))
                                                        .rounded(px(5.))
                                                        .bg(quote_bg)
                                                        .border_1()
                                                        .border_color(gpui_kit::rgb(palette.accent))
                                                        .text_xs()
                                                        .child(
                                                            crate::ui::icon(IconName::Sparkles)
                                                                .w(px(11.))
                                                                .h(px(11.))
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.accent,
                                                                )),
                                                        )
                                                        .child(lbl)
                                                        .child(
                                                            div()
                                                                .id(("ai-rm-skill", idx))
                                                                .cursor_pointer()
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.muted,
                                                                ))
                                                                .on_click(cx.listener(
                                                                    move |pane, _, _, cx| {
                                                                        pane.mounted_skills
                                                                            .retain(|s| s != &rm_sk);
                                                                        cx.notify();
                                                                    },
                                                                ))
                                                                .child(
                                                                    crate::ui::icon(
                                                                        IconName::Close,
                                                                    )
                                                                    .w(px(11.))
                                                                    .h(px(11.)),
                                                                ),
                                                        )
                                                },
                                            ))
                                            .when_some(self.quoted_selection.as_ref(), |row, q| {
                                                row.child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .max_w(px(240.))
                                                        .px_2()
                                                        .py(px(2.))
                                                        .rounded(px(5.))
                                                        .bg(quote_bg)
                                                        .border_1()
                                                        .border_color(gpui_kit::rgb(palette.accent))
                                                        .text_xs()
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
                                                                        pane.quoted_selection =
                                                                            None;
                                                                        cx.notify();
                                                                    },
                                                                ))
                                                                .child(
                                                                    crate::ui::icon(
                                                                        IconName::Close,
                                                                    )
                                                                    .w(px(11.))
                                                                    .h(px(11.)),
                                                                ),
                                                        ),
                                                )
                                            }),
                                    )
                                })
                                .child(
                                    Textarea::new(&self.input)
                                        .appearance(false)
                                        .bordered(false)
                                        .h(px(64.)),
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
                                                    Button::new("ai-plus-menu")
                                                        .ghost()
                                                        .compact()
                                                        .w(px(24.))
                                                        .h(px(24.))
                                                        .icon(crate::ui::icon(IconName::Plus))
                                                        .tooltip("启用 Skill / 附加文件")
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| cx.stop_propagation(),
                                                        )
                                                        .on_click(cx.listener(|pane, _, _, cx| {
                                                            pane.plus_menu_open =
                                                                !pane.plus_menu_open;
                                                            cx.notify();
                                                        })),
                                                )
                                                .child(
                                                    div()
                                                        .id("ai-perm-toggle")
                                                        .px_1p5()
                                                        .h(px(24.))
                                                        .rounded(px(5.))
                                                        .cursor_pointer()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .text_xs()
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
                                                            Tooltip::new(
                                                                "切换 Shell 命令确认权限",
                                                            )
                                                            .build(window, cx)
                                                        })
                                                        .on_click(cx.listener(
                                                            |pane, _, _, cx| {
                                                                pane.auto_approve_shell =
                                                                    !pane.auto_approve_shell;
                                                                cx.notify();
                                                            },
                                                        ))
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
                                                .child(model_btn)
                                                .when(supports_thinking, |row| row.child(think_btn))
                                                .child(if self.is_streaming {
                                                    div()
                                                        .id("ai-abort")
                                                        .size(px(26.))
                                                        .rounded_full()
                                                        .bg(gpui_kit::rgb(palette.danger))
                                                        .text_color(gpui_kit::rgb(
                                                            palette.on_accent,
                                                        ))
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
                                                        .size(px(26.))
                                                        .rounded_full()
                                                        .bg(gpui_kit::rgb(palette.accent))
                                                        .text_color(gpui_kit::rgb(
                                                            palette.on_accent,
                                                        ))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .cursor_pointer()
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
            })
    }
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
        "off" => "关".into(),
        "minimal" => "极低".into(),
        "low" => "低".into(),
        "medium" => "中".into(),
        "high" => "高".into(),
        "xhigh" => "极高".into(),
        "max" => "最大".into(),
        other => other.to_owned(),
    }
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
