//! Session-local RPC and transcript state. The pane consumes UI effects on the GPUI thread.

use super::*;

/// Pi message bodies keep their SDK shape. The snapshot envelope is shared by
/// initial attachment and the explicit state/history responses.
#[derive(Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Snapshot {
    sequence: u64,
    is_streaming: bool,
    is_compacting: bool,
    session_name: Option<String>,
    thinking_level: Option<String>,
    model: Option<Value>,
    messages: Option<Vec<Value>>,
    partial: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
enum SessionEvent {
    AgentStart,
    AgentSettled,
    UserEntry { entry_id: String, timestamp: i64 },
    ThinkingLevelChanged { #[serde(rename = "level")] thinking_level: Option<String> },
    SessionInfoChanged { name: Option<String> },
    TitleError { error: Option<String> },
    CompactionStart,
    CompactionEnd { error_message: Option<String> },
    ExtensionError { error: Option<String> },
    MessageStart { #[serde(default)] message: Value },
    MessageUpdate { assistant_message_event: Option<Value> },
    MessageEnd { #[serde(default)] message: Value },
    ToolExecutionStart {
        #[serde(default)] tool_call_id: String,
        #[serde(default)] tool_name: String,
        #[serde(default)] args: Value,
        write_before: Option<String>,
    },
    ToolExecutionUpdate {
        #[serde(default)] tool_call_id: String,
        #[serde(default)] partial_result: Value,
    },
    ToolExecutionEnd {
        #[serde(default)] tool_call_id: String,
        #[serde(default)] tool_name: String,
        #[serde(default)] is_error: bool,
        duration_ms: Option<u64>,
        #[serde(default)] result: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Default)]
pub(super) struct SessionChanges {
    pub(super) reset_transcript: bool,
    pub(super) follow_output: bool,
    pub(super) toast: Option<(String, bool)>,
    pub(super) extension_prefill: Option<(String, bool)>,
    pub(super) composer_text: Option<String>,
    pub(super) rewind_draft: Option<Value>,
}

pub(super) struct Session {
    pub(super) host: Arc<rpc::Host>,
    pub(super) runtime_id: String,
    pub(super) last_sequence: u64,
    pub(super) cwd: PathBuf,
    pub(super) client: Option<RpcClient>,
    pub(super) startup_error: Option<String>,
    pub(super) messages: Vec<ChatMessage>,
    pub(super) is_streaming: bool,
    pub(super) is_compacting: bool,
    pub(super) is_rewinding: bool,
    pub(super) session_name: Option<String>,
    pub(super) available_models: Vec<ModelInfo>,
    pub(super) current_model: Option<ModelInfo>,
    pub(super) thinking_levels: Vec<String>,
    pub(super) current_thinking: String,
    pub(super) context_usage: Option<ContextUsage>,
    pub(super) commands: Vec<PiCommandInfo>,
    pub(super) auto_approve_shell: bool,
    pub(super) pending_ui: Option<PendingExtensionUi>,
    pub(super) ext_statuses: BTreeMap<String, String>,
    pub(super) ext_widgets: BTreeMap<String, Vec<String>>,
    pub(super) active_run_start: Option<std::time::Instant>,
    pub(super) active_thinking_start: Option<std::time::Instant>,
    pub(super) write_snapshots: HashMap<String, String>,
    pub(super) active_part_map: HashMap<usize, usize>,
    changes: SessionChanges,
}

impl Session {
    pub(super) fn new(host: Arc<rpc::Host>, cwd: PathBuf, runtime_id: String) -> Self {
        Self {
            host,
            runtime_id,
            last_sequence: 0,
            cwd,
            client: None,
            startup_error: None,
            messages: Vec::new(),
            is_streaming: false,
            is_compacting: false,
            is_rewinding: false,
            session_name: None,
            available_models: Vec::new(),
            current_model: None,
            thinking_levels: vec!["off".into(), "low".into(), "medium".into(), "high".into()],
            current_thinking: "high".into(),
            context_usage: None,
            commands: Vec::new(),
            auto_approve_shell: false,
            pending_ui: None,
            ext_statuses: BTreeMap::new(),
            ext_widgets: BTreeMap::new(),
            active_run_start: None,
            active_thinking_start: None,
            write_snapshots: HashMap::new(),
            active_part_map: HashMap::new(),
            changes: SessionChanges::default(),
        }
    }

    pub(super) fn connect(&mut self) {
        if self.client.is_some() {
            return;
        }
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
    }

    pub(super) fn reconnect(&mut self, host: Arc<rpc::Host>) -> bool {
        if Arc::ptr_eq(&self.host, &host) {
            return false;
        }
        self.host = host;
        self.client = None;
        self.last_sequence = 0;
        self.connect();
        true
    }

    pub(super) fn submit_prompt(
        &mut self,
        payload: &str,
        images: &[RpcImage],
        title_source: &str,
        message: ChatMessage,
    ) -> Result<bool, String> {
        self.connect();
        let Some(client) = &self.client else {
            return Ok(false);
        };
        client.prompt_with_images(payload, images, title_source)?;
        self.messages.push(message);
        self.is_streaming = true;
        self.active_run_start = Some(std::time::Instant::now());
        Ok(true)
    }

    pub(super) fn compact(&mut self) {
        if let Some(client) = &self.client && !self.is_compacting && !self.is_rewinding {
            self.is_compacting = true;
            let _ = client.compact();
        }
    }

    pub(super) fn can_rewind(&self) -> bool {
        self.client.is_some() && !self.is_streaming && !self.is_compacting
            && !self.is_rewinding && self.pending_ui.is_none()
    }

    pub(super) fn rewind(&mut self, entry_id: &str, edit: bool) -> Result<(), String> {
        if !self.can_rewind() {
            return Ok(());
        }
        self.client.as_ref().expect("rewind requires a connected session").send(
            serde_json::json!({"type":"rewind", "entryId":entry_id, "edit":edit}),
        )?;
        self.is_rewinding = true;
        Ok(())
    }

    pub(super) fn set_thinking(&mut self, level: &str) {
        self.current_thinking = level.to_owned();
        if let Some(client) = &self.client {
            let _ = client.set_thinking_level(level);
        }
    }

    pub(super) fn undo_tag_approval(&mut self, id: &str) -> Option<crate::tags::agent::TagConfirmation> {
        let approval = self.find_tool_mut(id)?.approval.as_mut()?;
        if approval.state != ApprovalState::Applied {
            return None;
        }
        approval.state = ApprovalState::Undone;
        Some(approval.preview.clone())
    }

    pub(super) fn respond_confirm(&mut self, id: &str, confirmed: bool, auto_allow_shell: bool) {
        // A click from an old render must not confirm the next queued request.
        let Some(pending) = self.pending_ui.take_if(|pending| pending.id == id) else {
            return;
        };
        self.auto_approve_shell |= auto_allow_shell;
        if let Some(client) = &self.client {
            let _ = client.respond_extension_confirm(&pending.id, confirmed);
        }
    }

    pub(super) fn respond_extension(&mut self, value: Option<&str>) {
        if let Some(pending) = self.pending_ui.take() && let Some(client) = &self.client {
            match value {
                Some(value) => { let _ = client.respond_extension_value(&pending.id, value); }
                None => { let _ = client.respond_extension_cancelled(&pending.id); }
            }
        }
    }

    pub(super) fn poll(&mut self) -> Option<SessionChanges> {
        let mut batch = Vec::new();
        if let Some(client) = &self.client {
            while let Ok(item) = client.inbound.try_recv() {
                batch.push(item);
            }
        }
        if batch.is_empty() {
            return None;
        }
        for item in batch {
            let sequence = match &item {
                RpcInbound::Event(ev) | RpcInbound::ExtensionUiRequest(ev) => ev["sequence"].as_u64(),
                RpcInbound::WorkspaceTool(request) => request.sequence,
                _ => None,
            };
            if let Some(sequence) = sequence {
                    if sequence <= self.last_sequence { continue; }
                    self.last_sequence = sequence;
            }
            self.handle_inbound(item);
        }
        Some(std::mem::take(&mut self.changes))
    }

    fn scroll_to_bottom(&mut self) {
        self.changes.follow_output = true;
    }

    fn show_toast(&mut self, text: String, is_error: bool) {
        self.changes.toast = Some((text, is_error));
    }

    fn handle_inbound(&mut self, item: RpcInbound) {
        match item {
            RpcInbound::Response {
                command,
                success,
                data,
                error,
            } => {
                if command == "rewind" {
                    self.is_rewinding = false;
                    if !success {
                        if let Some(err) = error {
                            self.show_toast(crate::i18n::ai::command_failed(
                                crate::i18n::ai::REWIND_MESSAGE.get(), &err,
                            ), true);
                        }
                        return;
                    }
                }
                if !success {
                    if let Some(err) = error {
                        self.messages.push(ChatMessage::Notice {
                            text: crate::i18n::ai::command_failed(&command, &err),
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
                let Some(mut data) = data else {
                    return;
                };
                match command.as_str() {
                    "snapshot" | "get_state" | "get_messages" => self.apply_snapshot(&command, data),
                    "rewind" => {
                        self.changes.rewind_draft = data.get_mut("draft").map(Value::take);
                        self.apply_snapshot(&command, data);
                        self.write_snapshots.clear();
                        self.scroll_to_bottom();
                        if let Some(client) = &self.client {
                            let _ = client.get_session_stats();
                        }
                    }
                    "get_available_models" => self.apply_models(&data),
                    "get_available_thinking_levels" => self.apply_thinking_levels(&data),
                    "get_session_stats" => self.apply_stats(&data),
                    "get_commands" => self.apply_commands(&data),
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
                        self.show_toast(crate::i18n::ai::CONTEXT_COMPACTED.get().into(), false);
                        if let Some(client) = &self.client {
                            let _ = client.get_session_stats();
                        }
                    }
                    _ => {}
                }
            }
            RpcInbound::Event(ev) if ev["type"].as_str() == Some("message_update") => {
                if let Some(event) = ev.get("assistantMessageEvent") {
                    self.handle_assistant_delta(event);
                }
            }
            RpcInbound::Event(ev) => match serde_json::from_value::<SessionEvent>(ev) {
                Ok(event) => {
                    let event = serde_json::to_value(event).expect("typed session event is serializable");
                    self.handle_session_event(&event);
                }
                Err(error) => self.show_toast(crate::i18n::ai::ext_error(&error.to_string()), true),
            },
            RpcInbound::ExtensionUiRequest(req) => self.handle_ext_ui_request(&req),
            RpcInbound::WorkspaceTool(_) => {}
            RpcInbound::Stderr(line) => {
                if line.to_ascii_lowercase().contains("error") && self.messages.is_empty() {
                    self.startup_error = Some(line);
                }
            }
            RpcInbound::Exited(code) => {
                self.client = None;
                self.is_streaming = false;
                self.is_compacting = false;
                self.is_rewinding = false;
                self.pending_ui = None;
                let detail = code.map(|c| format!(" (code {c})")).unwrap_or_default();
                self.messages.push(ChatMessage::Notice {
                    text: crate::i18n::ai::host_exited(&detail),
                    is_error: true,
                });
            }
        }
    }

    fn apply_snapshot(&mut self, command: &str, data: Value) {
        let snapshot: Snapshot = match serde_json::from_value(data) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.show_toast(crate::i18n::ai::command_failed(command, &error.to_string()), true);
                return;
            }
        };
        // A snapshot started before the initial prompt cannot erase that prompt.
        let submitted_during_startup = command == "snapshot" && self.is_streaming && !snapshot.is_streaming;
        if !submitted_during_startup && command != "get_state" {
            self.apply_messages(&snapshot);
        }
        if command == "get_messages" {
            return;
        }
        self.is_streaming = snapshot.is_streaming || submitted_during_startup;
        self.is_compacting = snapshot.is_compacting;
        self.session_name = snapshot.session_name;
        if let Some(level) = snapshot.thinking_level {
            self.current_thinking = level;
        }
        if let Some(model) = snapshot.model.as_ref().and_then(parse_model) {
            self.current_model = Some(model);
        }
        if matches!(command, "snapshot" | "rewind") {
            self.last_sequence = snapshot.sequence;
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

    fn apply_messages(&mut self, snapshot: &Snapshot) {
        let Some(arr) = &snapshot.messages else {
            return;
        };
        self.messages.clear();
        self.changes.reset_transcript = true;
        self.active_run_start = None;
        self.active_thinking_start = None;
        self.active_part_map.clear();
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
                        entry_id: msg["entryId"].as_str().map(str::to_owned),
                        display_text,
                        timestamp_ms: msg.get("timestamp").and_then(Value::as_i64),
                        attached_file,
                        mounted_skills,
                        quotes,
                        images: msg.get("content").and_then(Value::as_array).into_iter().flatten()
                            .filter_map(|block| PastedImage::from_pi(0, block).map(|image| image.image))
                            .collect(),
                    });
                }
                Some("assistant") => {
                    // A live snapshot must continue the same content blocks when subsequent deltas arrive.
                    let active = snapshot.partial && message_index + 1 == arr.len();
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
                                    let done = !snapshot.is_streaming || pending_tool_results.contains_key(&id);
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

    fn handle_session_event(&mut self, ev: &Value) {
        let kind = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "user_entry" => {
                // The panel adds its outgoing prompt optimistically; Pi supplies its durable ID.
                if let Some(ChatMessage::User { entry_id, timestamp_ms, .. }) =
                    self.messages.iter_mut().rfind(|message| matches!(message, ChatMessage::User { .. }))
                    && entry_id.is_none()
                {
                    *entry_id = ev["entryId"].as_str().map(str::to_owned);
                    *timestamp_ms = ev["timestamp"].as_i64();
                }
            }
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
                                streaming: p_stream,
                                duration_ms: p_dur,
                                ..
                            } = p
                            {
                                *p_stream = false;
                                if p_dur.is_none() {
                                    *p_dur = think_elapsed;
                                }
                            } else if let AssistantPart::Tool(block) = p {
                                // Aborting argument generation produces no tool_execution_end event.
                                if !block.done {
                                    block.done = true;
                                    block.is_error = true;
                                    block.summary = crate::i18n::ai::TOOL_INCOMPLETE.get().into();
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
                self.show_toast(
                    crate::i18n::ai::title_gen_failed(
                        ev["error"].as_str().unwrap_or(crate::i18n::ai::REQUEST_FAILED.get()),
                    ),
                    true,
                );
            }
            "compaction_start" => {
                self.is_compacting = true;
            }
            "compaction_end" => {
                self.is_compacting = false;
                if let Some(err) = ev.get("errorMessage").and_then(|v| v.as_str()) {
                    self.show_toast(crate::i18n::ai::compact_failed(&err), true);
                } else {
                    self.show_toast(crate::i18n::ai::CONTEXT_COMPACTED.get().into(), false);
                }
                if let Some(client) = &self.client {
                    let _ = client.get_session_stats();
                }
            }
            "extension_error" => {
                let err = ev
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or(crate::i18n::ai::EXT_RUNTIME_ERROR.get());
                self.show_toast(crate::i18n::ai::ext_error(err), true);
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
                            streaming: p_stream,
                            duration_ms,
                            ..
                        } = p
                        {
                            *p_stream = false;
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
                        block.summary = crate::i18n::ai::edited_diff(
                            &name_only,
                            first_diff.added,
                            first_diff.removed,
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
                        duration_ms,
                        streaming,
                    }) = parts.get_mut(idx)
                {
                    if let Some(content) = ame.get("content").and_then(|v| v.as_str()) {
                        *text = content.to_owned();
                    }
                    *streaming = false;
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
                // The host forwards the active tool call separately from Pi's cumulative snapshot.
                let tool_call = ame.get("toolCall");
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
                        diffs: Vec::new(), approval: None,
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

    fn handle_ext_ui_request(&mut self, req: &Value) {
        let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        match method {
            "confirm" => {
                let title = req
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(crate::i18n::ai::PROMPT_CONFIRM.get())
                    .to_owned();
                let message = req
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let command_approval = req["commandApproval"].as_bool() == Some(true)
                    || title.contains("bash")
                    || title.contains("powershell");
                let content = if command_approval {
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
                    .unwrap_or(crate::i18n::ai::PROMPT_SELECT.get())
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
                    .unwrap_or(crate::i18n::ai::PROMPT_INPUT.get())
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
                self.changes.extension_prefill = Some((String::new(), false));
                self.scroll_to_bottom();
            }
            "editor" => {
                let title = req
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(crate::i18n::ai::PROMPT_EDIT.get())
                    .to_owned();
                let prefill = req
                    .get("prefill")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                self.changes.extension_prefill = Some((prefill.clone(), true));
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
                    self.show_toast(message, true);
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
                    self.changes.composer_text = Some(text.to_owned());
                }
            }
            _ => {}
        }
    }

    pub(super) fn resolve_tool_path(&self, raw: &str) -> PathBuf {
        let p = Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }

}
