//! Project-scoped tab ownership. Conversation entities stay alive while their tasks run off screen.
use super::*;
use gpui_kit::base::Selectable;
use gpui_kit::component::Sizable;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Deserialize, Serialize)]
struct ProjectTabs {
    tabs: Vec<String>,
    active: Option<String>,
}
#[derive(Default, Deserialize, Serialize)]
struct TabStore {
    projects: BTreeMap<String, ProjectTabs>,
}

#[derive(Clone, Deserialize)]
struct HistoryEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "firstMessage")]
    preview: String,
    #[serde(default, rename = "messageCount")]
    count: usize,
    #[serde(default)]
    modified: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default, rename = "isStreaming")]
    running: bool,
    #[serde(default)]
    waiting: bool,
    #[serde(default)]
    failed: bool,
}
impl HistoryEntry {
    fn title(&self) -> String {
        self.name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                if self.preview.trim().is_empty() {
                    crate::i18n::ai::NEW_SESSION.get().into()
                } else {
                    self.preview.chars().take(48).collect()
                }
            })
    }
}

pub struct AiPane {
    cwd: PathBuf,
    project_key: String,
    project_name: Option<String>,
    settings: AppSettings,
    math: Entity<MathRenderer>,
    host: Option<Arc<rpc::Host>>,
    controls: HashMap<String, RpcClient>,
    sessions: HashMap<(String, String), SessionTab>,
    store: TabStore,
    histories: HashMap<String, Vec<HistoryEntry>>,
    search: Entity<InputState>,
    rename: Entity<InputState>,
    renaming: Option<String>,
    drawer: bool,
    history_scroll: ScrollHandle,
    pending_focus: Option<(String, String)>,
    displayed_view: Option<(String, String)>,
    error: Option<String>,
    error_task: Task<()>,
    creating: HashSet<String>,
    restoring: HashSet<String>,
    pending_deletion: Option<(String, bool)>,
    deleted: HashMap<String, String>,
    attached: Option<AttachedFile>,
    pending_selections: HashMap<String, (String, Option<AttachedFile>, String)>,
    opened: bool,
    _poll: Task<()>,
    _inputs: Vec<Subscription>,
}

/// The manager owns each runtime by project/session identity. Closing a
/// running tab retains its runtime and draft until the host reports settlement.
struct SessionTab {
    state: Rc<RefCell<Session>>,
    view: Entity<ChatPane>,
    _subscriptions: [Subscription; 2],
}

impl AiPane {
    pub fn new(
        cwd: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::ai::SEARCH_SESSIONS_PLACEHOLDER.get()));
        let rename = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::ai::SESSION_NAME_PLACEHOLDER.get()));
        let search_sub = cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify());
        let rename_sub = cx.subscribe(&rename, |pane, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                if let Some(id) = pane.renaming.take() {
                    let name = input.read(cx).value().trim().to_owned();
                    if !name.is_empty() {
                        pane.command(
                            json!({"type":"set_session_name", "sessionId":id, "name":name}),
                            cx,
                        );
                    }
                }
            }
        });
        let location = crate::storage::data_directory()
            .expect("application data directory")
            .join("ai-tabs.json");
        let (mut store, error): (TabStore, Option<String>) = match std::fs::read(&location) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(store) => (store, None),
                Err(e) => (TabStore::default(), Some(e.to_string())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (TabStore::default(), None),
            Err(e) => (TabStore::default(), Some(e.to_string())),
        };
        // On cold start, keep only the last active AI tab per project instead of inheriting all old tabs.
        for tabs in store.projects.values_mut() {
            let keep = tabs
                .active
                .as_ref()
                .filter(|id| tabs.tabs.contains(id))
                .cloned()
                .or_else(|| tabs.tabs.last().cloned());
            tabs.tabs = keep.iter().cloned().collect();
            tabs.active = keep;
        }
        let restoring = store.projects.keys().cloned().collect();
        let poll = cx.spawn(async move |this, cx| {
            let mut interval = Duration::from_millis(35);
            loop {
                cx.background_executor().timer(interval).await;
                match this.update(cx, |pane: &mut Self, cx| {
                    pane.poll(cx);
                    interval = pane.poll_interval();
                }) {
                    Ok(()) => {}
                    Err(_) => break,
                }
            }
        });
        let project_key = rpc::project_key(&cwd);
        Self {
            cwd,
            project_key,
            project_name: None,
            settings,
            math,
            host: None,
            controls: HashMap::new(),
            sessions: HashMap::new(),
            store,
            histories: HashMap::new(),
            search,
            rename,
            renaming: None,
            drawer: false,
            history_scroll: ScrollHandle::new(),
            pending_focus: None,
            displayed_view: None,
            error,
            error_task: Task::ready(()),
            creating: HashSet::new(),
            restoring,
            pending_deletion: None,
            deleted: HashMap::new(),
            attached: None,
            pending_selections: HashMap::new(),
            opened: false,
            _poll: poll,
            _inputs: vec![search_sub, rename_sub],
        }
    }

    fn project(&self) -> String {
        self.project_key.clone()
    }
    fn save(&mut self) {
        let result = (|| -> std::io::Result<()> {
            let location = crate::storage::data_directory()?.join("ai-tabs.json");
            std::fs::create_dir_all(location.parent().expect("data parent"))?;
            let temp = location.with_extension("json.tmp");
            std::fs::write(&temp, serde_json::to_vec_pretty(&self.store)?)?;
            std::fs::rename(temp, location)
        })();
        if let Err(error) = result {
            self.error = Some(crate::i18n::ai::err_save_tags(&error));
        }
    }

    fn control(&mut self) -> Result<&RpcClient, String> {
        let project = self.project();
        if !self.controls.contains_key(&project) {
            let host = match &self.host {
                Some(host) => host.clone(),
                None => rpc::Host::shared(&self.settings.ai)?,
            };
            self.host = Some(host.clone());
            self.controls
                .insert(project.clone(), RpcClient::connect(host, &self.cwd, None));
            self.error = None;
        }
        Ok(&self.controls[&project])
    }
    fn set_error(&mut self, error: impl Into<String>, cx: &mut Context<Self>) {
        let message = error.into();
        if self.error.as_deref() == Some(message.as_str()) {
            return;
        }
        self.error = Some(message);
        self.error_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(5))
                .await;
            let _ = this.update(cx, |pane, cx| {
                pane.error = None;
                cx.notify();
            });
        });
    }

    fn clear_error(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.error_task = Task::ready(());
        cx.notify();
    }

    fn command(&mut self, value: Value, cx: &mut Context<Self>) {
        match self.control().and_then(|client| client.send(value)) {
            Ok(_) => {}
            Err(error) => self.set_error(error, cx),
        }
        cx.notify();
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.command(json!({"type":"list_sessions"}), cx);
    }
    fn create(&mut self, cx: &mut Context<Self>) {
        self.drawer = false;
        self.renaming = None;
        if self.creating.insert(self.project()) {
            self.command(json!({"type":"create_session"}), cx);
        }
    }
    pub fn active_tasks(&self) -> Vec<(String, String, bool)> {
        let project = self.project();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        if let Some(entries) = self.histories.get(&project) {
            for entry in entries {
                let session = self
                    .sessions
                    .get(&(project.clone(), entry.id.clone()))
                    .map(|tab| tab.state.borrow());
                let waiting = session.as_ref().is_some_and(|v| v.pending_ui.is_some()) || entry.waiting;
                let running = session.as_ref().is_some_and(|v| v.is_streaming) || entry.running;
                if running || waiting {
                    let title = session
                        .and_then(|v| v.session_name.clone())
                        .unwrap_or_else(|| entry.title());
                    seen.insert(entry.id.clone());
                    out.push((entry.id.clone(), title, waiting));
                }
            }
        }
        for ((proj, id), tab) in &self.sessions {
            if proj != &project || seen.contains(id) {
                continue;
            }
            let v = tab.state.borrow();
            let waiting = v.pending_ui.is_some();
            if v.is_streaming || waiting {
                let title = v
                    .session_name
                    .clone()
                    .unwrap_or_else(|| crate::i18n::ai::NEW_SESSION.get().into());
                out.push((id.clone(), title, waiting));
            }
        }
        out
    }

    pub fn activate(&mut self, id: String, cx: &mut Context<Self>) {
        let project = self.project();
        if self.sessions.contains_key(&(project.clone(), id.clone())) {
            self.command(json!({"type":"retain", "sessionId":id}), cx);
        }
        let tabs = self.store.projects.entry(project).or_default();
        if !tabs.tabs.contains(&id) {
            tabs.tabs.push(id.clone());
        }
        tabs.active = Some(id);
        self.drawer = false;
        self.renaming = None;
        self.save();
        cx.notify();
    }
    fn close(&mut self, id: &str, cx: &mut Context<Self>) {
        let project = self.project();
        let tabs = self.store.projects.entry(project.clone()).or_default();
        let index = tabs.tabs.iter().position(|tab| tab == id).unwrap_or(0);
        tabs.tabs.retain(|tab| tab != id);
        if tabs.active.as_deref() == Some(id) {
            tabs.active = tabs
                .tabs
                .get(index.min(tabs.tabs.len().saturating_sub(1)))
                .cloned();
        }
        let identity = (project, id.to_owned());
        let running = self
            .sessions
            .get(&identity)
            .is_some_and(|tab| {
                let session = tab.state.borrow();
                session.is_streaming || session.pending_ui.is_some()
            });
        if self.sessions.contains_key(&identity) {
            self.command(json!({"type":"release", "sessionId":id}), cx);
            if !running {
                self.sessions.remove(&identity);
            }
        }
        self.save();
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // Drain conversation routes before control-route settlement can dispose
        // of a closed tab. Background sessions run on the same scheduler.
        for tab in self.sessions.values() {
            let changes = tab.state.borrow_mut().poll();
            if let Some(changes) = changes {
                tab.view.update(cx, |view, cx| view.apply_session_changes(changes, cx));
            }
        }
        let batch: Vec<_> = self
            .controls
            .iter()
            .flat_map(|(project, client)| {
                client
                    .inbound
                    .try_iter()
                    .map(|item| (project.clone(), item))
                    .collect::<Vec<_>>()
            })
            .collect();
        if batch.is_empty() {
            return;
        }
        for (project, item) in batch {
            match item {
                RpcInbound::Response {
                    command,
                    success,
                    data,
                    error,
                } => {
                    if !success {
                        if let Some(error) = error {
                            self.set_error(error, cx);
                        }
                        if command == "create_session" {
                            self.creating.remove(&project);
                        }
                        continue;
                    }
                    let data = data.expect("successful host response data");
                    match command.as_str() {
                        "list_sessions" => match serde_json::from_value::<Vec<HistoryEntry>>(
                            data["sessions"].clone(),
                        ) {
                            Ok(history) => {
                                // Blank sessions have no JSONL until their first message; do not restore vanished tabs.
                                if self.restoring.remove(&project) {
                                    let tabs =
                                        self.store.projects.entry(project.clone()).or_default();
                                    tabs.tabs
                                        .retain(|id| history.iter().any(|entry| &entry.id == id));
                                    if !tabs
                                        .active
                                        .as_ref()
                                        .is_some_and(|id| tabs.tabs.contains(id))
                                    {
                                        tabs.active = tabs.tabs.last().cloned();
                                    }
                                    self.sessions.retain(|(owner, id), _| {
                                        owner != &project || tabs.tabs.contains(id)
                                    });
                                    let empty = tabs.tabs.is_empty();
                                    self.save();
                                    if empty && project == self.project() && self.opened {
                                        self.create(cx);
                                    }
                                }
                                self.histories.insert(project.clone(), history);
                            }
                            Err(error) => self.set_error(crate::i18n::ai::err_sessions_json(&error), cx),
                        },
                        "create_session" => {
                            self.creating.remove(&project);
                            let id = data["sessionId"]
                                .as_str()
                                .expect("created session id")
                                .to_owned();
                            let tabs = self.store.projects.entry(project.clone()).or_default();
                            tabs.tabs.push(id.clone());
                            tabs.active = Some(id.clone());
                            self.pending_focus = Some((project.clone(), id));
                            self.save();
                            if let Some(client) = self.controls.get(&project) {
                                let _ = client.send(json!({"type":"list_sessions"}));
                            }
                        }
                        "delete_session" => {
                            if let Some(id) = self.deleted.remove(&project) {
                                let tabs = self.store.projects.entry(project.clone()).or_default();
                                tabs.tabs.retain(|tab| tab != &id);
                                if tabs.active.as_ref() == Some(&id) {
                                    tabs.active = tabs.tabs.last().cloned();
                                }
                                self.sessions.remove(&(project.clone(), id));
                                self.save();
                            }
                            if let Some(client) = self.controls.get(&project) {
                                let _ = client.send(json!({"type":"list_sessions"}));
                            }
                        }
                        "set_pinned" | "set_session_name" => {
                            if let Some(client) = self.controls.get(&project) {
                                let _ = client.send(json!({"type":"list_sessions"}));
                            }
                        }
                        _ => {}
                    }
                }
                RpcInbound::WorkspaceTool(request) => {
                    // Project control routes stay alive even when the originating tab is hidden.
                    cx.emit(AiPaneEvent::WorkspaceTool(request));
                }
                RpcInbound::Event(event) => {
                    let kind = event["type"].as_str().unwrap_or("");
                    if matches!(
                        kind,
                        "session_status" | "session_info_changed" | "agent_settled" | "title_error"
                    ) {
                        if let Some(client) = self.controls.get(&project) {
                            let _ = client.send(json!({"type":"list_sessions"}));
                        }
                    }
                    if kind == "agent_settled" {
                        let id = event["sessionId"]
                            .as_str()
                            .expect("session event id")
                            .to_owned();
                        if !self
                            .store
                            .projects
                            .get(&project)
                            .is_some_and(|tabs| tabs.tabs.contains(&id))
                        {
                            self.sessions.remove(&(project.clone(), id));
                        }
                    }
                }
                RpcInbound::Exited(_) => {
                    self.set_error(
                        crate::i18n::ai::HOST_EXITED_REOPEN_BANNER.get(),
                        cx,
                    );
                    self.controls.clear();
                    self.host = None;
                }
                _ => {}
            }
        }
        cx.notify();
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.opened = true;
        self.refresh(cx);
        if !self
            .store
            .projects
            .get(&self.project())
            .is_some_and(|tabs| !tabs.tabs.is_empty())
        {
            self.create(cx);
        }
        self.ensure_view(window, cx);
        self.focus(window, cx);
    }
    pub fn respond_workspace_tool(&self, request: &WorkspaceToolRequest, result: Value) {
        if request.action == WorkspaceToolAction::Cancel {
            return;
        }
        if let Some(client) = self.controls.get(&request.project_key) {
            let _ = client.send(json!({ "type": "workspace_tool_response", "channel": request.channel,
                "id": request.id, "sessionId": request.session_id, "result": result }));
        }
    }

    fn poll_interval(&self) -> Duration {
        if self.sessions.values().any(|tab| {
            let session = tab.state.borrow();
            session.is_streaming || session.pending_ui.is_some()
        }) {
            Duration::from_millis(35)
        } else {
            Duration::from_millis(70)
        }
    }
    pub fn dismiss_history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.drawer {
            return false;
        }
        self.drawer = false;
        self.renaming = None;
        self.pending_deletion = None;
        self.focus(window, cx);
        cx.notify();
        true
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(id) = self
            .store
            .projects
            .get(&self.project())
            .and_then(|tabs| tabs.active.as_ref())
        {
            if let Some(tab) = self.sessions.get(&(self.project(), id.clone())) {
                let focus = tab.view.read(cx).input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
        }
    }
    pub fn set_directory(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        // Workspace synchronizes these setters on render; avoid filesystem work and notifications then.
        if self.cwd == cwd {
            return;
        }
        let project_key = rpc::project_key(&cwd);
        if project_key == self.project_key {
            return;
        }
        self.cwd = cwd;
        self.project_key = project_key;
        self.drawer = false;
        self.renaming = None;
        self.pending_deletion = None;
        if self.opened {
            self.refresh(cx);
            if !self
                .store
                .projects
                .get(&self.project())
                .is_some_and(|tabs| !tabs.tabs.is_empty())
            {
                self.create(cx);
            }
        }
        cx.notify();
    }
    pub fn set_project_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.project_name == name {
            return;
        }
        self.project_name = name.clone();
        for ((project, _), tab) in &self.sessions {
            if project == &self.project_key {
                tab.view.update(cx, |view, cx| view.set_project_name(name.clone(), cx));
            }
        }
        cx.notify();
    }

    pub fn refresh_locale(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::ai::SEARCH_SESSIONS_PLACEHOLDER.get(), window, cx);
        });
        self.rename.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::ai::SESSION_NAME_PLACEHOLDER.get(), window, cx);
        });
        for tab in self.sessions.values() {
            tab.view.update(cx, |view, cx| view.refresh_locale(window, cx));
        }
    }

    pub fn sync_settings(&mut self, settings: &AppSettings, cx: &mut Context<Self>) {
        let host_changed = self.settings.ai != settings.ai;
        let view_changed = self.settings.ai_font != settings.ai_font
            || self.settings.ai_font_size != settings.ai_font_size
            || self.settings.tool_execution_view != settings.tool_execution_view
            || self.settings.block_code_font != settings.block_code_font
            || self.settings.block_code_scale != settings.block_code_scale
            || self.settings.inline_code_font != settings.inline_code_font
            || self.settings.inline_code_scale != settings.inline_code_scale;
        self.settings = settings.clone();
        if host_changed && self.host.is_some() {
            self.command(json!({"type":"configure", "settings":settings.ai}), cx);
        }
        if view_changed {
            for tab in self.sessions.values() {
                tab.view.update(cx, |view, cx| view.sync_settings(settings, cx));
            }
            cx.notify();
        }
    }
    pub fn set_active_file(&mut self, file: Option<AttachedFile>, cx: &mut Context<Self>) {
        self.attached = file.clone();
        if let Some(id) = self
            .store
            .projects
            .get(&self.project())
            .and_then(|tabs| tabs.active.clone())
        {
            if let Some(tab) = self.sessions.get(&(self.project(), id)) {
                tab.view.update(cx, |view, cx| view.set_active_file(file, cx));
            }
        }
    }
    pub fn attach_selection(
        &mut self,
        source: String,
        file: Option<AttachedFile>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // SDK initialization is asynchronous; a project switch must not move its queued quote elsewhere.
        self.pending_selections
            .insert(self.project(), (source, file, text));
        self.open(window, cx);
    }
    fn ensure_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.project();
        let Some(id) = self
            .store
            .projects
            .get(&project)
            .and_then(|tabs| tabs.active.clone())
        else {
            return;
        };
        let identity = (project, id.clone());
        if let Err(error) = self.control() {
            self.error = Some(error);
            return;
        }
        let host = self.host.clone().expect("connected host");
        if let Some(tab) = self.sessions.get(&identity) {
            let restarted = tab.state.borrow_mut().reconnect(host);
            if restarted {
                tab.view.update(cx, |view, cx| view.open(window, cx));
            }
            if let Some((source, file, text)) = self.pending_selections.remove(&identity.0) {
                tab.view.update(cx, |view, cx| {
                    view.attach_selection(source, file, text, window, cx)
                });
            }
            return;
        }
        let state = Rc::new(RefCell::new(Session::new(host, self.cwd.clone(), id)));
        state.borrow_mut().connect();
        let view = cx.new(|cx| {
            ChatPane::new(
                state.clone(),
                self.settings.clone(),
                self.math.clone(),
                window,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.set_project_name(self.project_name.clone(), cx);
            view.set_active_file(self.attached.clone(), cx);
            view.open(window, cx);
        });
        if let Some((source, file, text)) = self.pending_selections.remove(&identity.0) {
            view.update(cx, |view, cx| {
                view.attach_selection(source, file, text, window, cx)
            });
        }
        let subscription = cx.subscribe(&view, |_, _, event: &AiPaneEvent, cx| match event {
            AiPaneEvent::WorkspaceTool(_) =>
                unreachable!("workspace tool requests use the project control route"),
            AiPaneEvent::UndoTags(items) => cx.emit(AiPaneEvent::UndoTags(items.clone())),
            AiPaneEvent::OpenFile(path) => cx.emit(AiPaneEvent::OpenFile(path.clone())),
            AiPaneEvent::BackToHub => cx.emit(AiPaneEvent::BackToHub),
            AiPaneEvent::ToggleMaximize => cx.emit(AiPaneEvent::ToggleMaximize),
            AiPaneEvent::OpenMermaidLightbox { tree, image } => {
                cx.emit(AiPaneEvent::OpenMermaidLightbox {
                    tree: Arc::clone(tree),
                    image: Arc::clone(image),
                });
            }
        });
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        self.sessions.insert(identity, SessionTab { view, state, _subscriptions: [subscription, observe] });
    }

    fn delete(&mut self, id: String, confirmed: bool, cx: &mut Context<Self>) {
        self.deleted.insert(self.project(), id.clone());
        self.command(
            json!({"type":"delete_session", "sessionId":id, "confirmed":confirmed}),
            cx,
        );
        self.pending_deletion = None;
    }

    fn history(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = active_palette();
        let ui_font = crate::ui::sans_font(&self.settings.ai_font);
        let font_size = (self.settings.ai_font_size as f32 - 2.).clamp(11., 13.5);
        let meta_size = (font_size - 1.5).max(10.);
        let active_id = self
            .store
            .projects
            .get(&self.project())
            .and_then(|tabs| tabs.active.as_deref());
        let query = self.search.read(cx).value().to_lowercase();
        let mut rows: Vec<_> = self
            .histories
            .get(&self.project())
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                fuzzy_rank(&entry.title(), &query).map(|rank| (rank, entry.clone()))
            })
            .collect();
        rows.sort_by_key(|(rank, _)| *rank);

        let text_color = gpui_kit::rgb(palette.text);
        let muted_color = gpui_kit::rgb(palette.muted);
        let hover_bg = gpui_kit::rgb(palette.hover);
        let pressed_bg = gpui_kit::rgb(palette.pressed);
        let accent_color = gpui_kit::rgb(palette.accent);
        let danger_fg = gpui_kit::rgb(palette.danger);
        let danger_bg = palette.danger_surface();
        let pin_tint = mix_color(palette.surface, palette.accent, 0.045);

        crate::ui::popover_card("ai-history-popover")
            .w(px(316.))
            .max_h(px(368.))
            .font_family(ui_font.clone())
            .text_size(px(font_size))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .px_2p5()
                    .py_2()
                    .border_b_1()
                    .border_color(palette.divider())
                    .child(
                        crate::ui::SearchField::new("ai-history-search", &self.search)
                            .text_size(px(font_size))
                            .font_family(ui_font.clone()),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("ai-history-list")
                            .max_h(px(310.))
                            .p_1p5()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .overflow_y_scroll()
                            .track_scroll(&self.history_scroll)
                            .children(rows.into_iter().map(|(_, entry)| {
                                let id = entry.id.clone();
                                let open = id.clone();
                                let pin = id.clone();
                                let rename = id.clone();
                                let delete = id.clone();
                                let running = entry.running;
                                let waiting = entry.waiting;
                                let failed = entry.failed;
                                let title = entry.title();
                                let initial = title.clone();
                                let pinned = entry.pinned;
                                let is_active = active_id == Some(id.as_str());
                                let is_renaming = self.renaming.as_ref() == Some(&id);
                                let row_group: SharedString = format!("history-row-{id}").into();
                                let status = if waiting {
                                    crate::i18n::ai::SESSION_STATUS_WAITING.get()
                                } else if running {
                                    crate::i18n::ai::SESSION_STATUS_RUNNING.get()
                                } else if failed {
                                    crate::i18n::ai::SESSION_STATUS_FAILED.get()
                                } else {
                                    ""
                                };
                                let status_dot = if waiting {
                                    Some(gpui_kit::rgb(0xd97706))
                                } else if running {
                                    Some(accent_color)
                                } else if failed {
                                    Some(danger_fg)
                                } else {
                                    None
                                };
                                let pin_tip = if pinned {
                                    crate::i18n::ai::UNPIN.get()
                                } else {
                                    crate::i18n::ai::PIN.get()
                                };
                                let modified = chrono::DateTime::parse_from_rfc3339(&entry.modified)
                                    .map(|t| t.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
                                    .unwrap_or_default();

                                div()
                                    .id(SharedString::from(format!("history-{id}")))
                                    .group(row_group.clone())
                                    .px_2()
                                    .py_1p5()
                                    .rounded(px(6.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .when(is_active, |row| row.bg(gpui_kit::rgb(palette.selection)))
                                    .when(!is_active && pinned, |row| row.bg(pin_tint))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover_bg))
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        if pane.renaming.as_ref() != Some(&open) {
                                            pane.activate(open.clone(), cx);
                                        }
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(if is_renaming {
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                    .child(
                                                        crate::ui::TextField::new(&self.rename)
                                                            .compact()
                                                            .surface_bg()
                                                            .text_size(px(font_size)),
                                                    )
                                                    .into_any_element()
                                            } else {
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_ellipsis()
                                                    .font_medium()
                                                    .text_color(text_color)
                                                    .child(title)
                                                    .into_any_element()
                                            })
                                            .when(!is_renaming, |top| {
                                                top.child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(2.))
                                                        .child(
                                                            div()
                                                                .id(SharedString::from(format!("pin-{id}")))
                                                                .size(px(22.))
                                                                .rounded(px(5.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .cursor_pointer()
                                                                .text_color(if pinned { accent_color } else { muted_color })
                                                                .when(!pinned, |btn| {
                                                                    btn.opacity(0.0).group_hover(row_group.clone(), |s| s.opacity(1.0))
                                                                })
                                                                .hover(move |s| s.bg(pressed_bg).text_color(if pinned { accent_color } else { text_color }))
                                                                .tooltip(move |w, cx| crate::ui::tooltip(pin_tip, w, cx))
                                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                                .on_click(cx.listener(move |pane, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    pane.command(json!({"type":"set_pinned","sessionId":pin,"pinned":!pinned}), cx);
                                                                }))
                                                                .child(crate::ui::icon(IconName::Pin).size(px(12.5))),
                                                        )
                                                        .child(
                                                            div()
                                                                .id(SharedString::from(format!("rename-{id}")))
                                                                .size(px(22.))
                                                                .rounded(px(5.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .cursor_pointer()
                                                                .text_color(muted_color)
                                                                .opacity(0.0)
                                                                .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                                                .hover(move |s| s.bg(pressed_bg).text_color(text_color))
                                                                .tooltip(|w, cx| crate::ui::tooltip(crate::i18n::ai::RENAME.get(), w, cx))
                                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                                .on_click(cx.listener(move |pane, _, window, cx| {
                                                                    cx.stop_propagation();
                                                                    pane.renaming = Some(rename.clone());
                                                                    pane.rename.update(cx, |input, cx| {
                                                                        input.set_value(initial.clone(), window, cx);
                                                                        input.focus(window, cx);
                                                                    });
                                                                    cx.notify();
                                                                }))
                                                                .child(crate::ui::icon(IconName::PencilLine).size(px(12.5))),
                                                        )
                                                        .child(
                                                            div()
                                                                .id(SharedString::from(format!("delete-{id}")))
                                                                .size(px(22.))
                                                                .rounded(px(5.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .cursor_pointer()
                                                                .text_color(muted_color)
                                                                .opacity(0.0)
                                                                .group_hover(row_group, |s| s.opacity(1.0))
                                                                .hover(move |s| s.bg(danger_bg).text_color(danger_fg))
                                                                .tooltip(|w, cx| crate::ui::tooltip(crate::i18n::ai::DELETE_SESSION.get(), w, cx))
                                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                                .on_click(cx.listener(move |pane, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    pane.pending_deletion = Some((delete.clone(), running || waiting));
                                                                    cx.notify();
                                                                }))
                                                                .child(crate::ui::icon(IconName::Trash).size(px(12.5))),
                                                        ),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .text_size(px(meta_size))
                                            .text_color(muted_color)
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .when_some(status_dot, |meta, dot| {
                                                        meta.child(
                                                            div()
                                                                .w(px(5.))
                                                                .h(px(5.))
                                                                .rounded_full()
                                                                .bg(dot),
                                                        )
                                                    })
                                                    .child(crate::i18n::ai::session_meta_line(pinned, entry.count, status)),
                                            )
                                            .child(modified),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .top(px(4.))
                            .bottom(px(4.))
                            .child(crate::ui::scrollbar_vertical(&self.history_scroll)),
                    ),
            )
            .into_any_element()
    }
}

// Substrings rank above ordered-character matches; stable sorting preserves pin/activity order.
fn fuzzy_rank(title: &str, query: &str) -> Option<usize> {
    let title = title.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if let Some(index) = title.find(query) {
        return Some(index);
    }
    let mut chars = title.chars();
    for ch in query.chars() {
        if !chars.by_ref().any(|candidate| candidate == ch) {
            return None;
        }
    }
    Some(title.len() + 1000)
}

impl Render for AiPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_view(window, cx);
        if let Some((project, id)) = self.pending_focus.take() {
            // Apply focus after the new textarea mounts; the toolbar click must not reclaim it.
            let this = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = this.update(cx, |pane, cx| {
                    if pane.project_key == project
                        && pane
                            .store
                            .projects
                            .get(&project)
                            .and_then(|tabs| tabs.active.as_ref())
                            == Some(&id)
                    {
                        pane.focus(window, cx);
                    }
                });
            });
        }
        let project = self.project();
        let palette = active_palette();
        let tabs = self
            .store
            .projects
            .get(&project)
            .map(|tabs| tabs.tabs.clone())
            .unwrap_or_default();
        let active = self
            .store
            .projects
            .get(&project)
            .and_then(|tabs| tabs.active.clone());
        let displayed_view = active.as_ref().map(|id| (project.clone(), id.clone()));
        if self.displayed_view != displayed_view {
            // A background conversation keeps its draft, but must retire its transient selection UI.
            if let Some(tab) = self.displayed_view.as_ref().and_then(|key| self.sessions.get(key)) {
                tab.view.update(cx, |view, cx| view.dismiss_quote(cx));
            }
            self.displayed_view = displayed_view;
        }
        let header_bg = if palette.is_dark() {
            palette.background
        } else {
            palette.hover
        };
        div()
            .id("ai-manager")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(gpui_kit::rgb(palette.surface))
            .text_color(gpui_kit::rgb(palette.text))
            .font_family(crate::ui::sans_font(&self.settings.ai_font))
            .text_size(px(self.settings.ai_font_size as f32))
            .child(
                div()
                    .h(px(crate::ui::PANE_HEADER_HEIGHT))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .bg(gpui_kit::rgb(header_bg))
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .child(
                        crate::ui::panel_toolbar_button("ai-back-to-hub", palette, cx)
                            .icon(crate::ui::icon(IconName::ChevronLeft).size(px(15.)))
                            .tooltip(crate::i18n::common::BACK_TO_HUB.get())
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AiPaneEvent::BackToHub))),
                    )
                    .child(
                        crate::ui::panel_toolbar_button("ai-history", palette, cx)
                            .selected(self.drawer)
                            .toggled(self.drawer)
                            .icon(crate::ui::icon(IconName::Clock).size(px(15.)))
                            .tooltip(crate::i18n::ai::SESSION_HISTORY.get())
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.drawer = !pane.drawer;
                                if pane.drawer {
                                    pane.refresh(cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("ai-tabs")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_hidden()
                            .children(tabs.into_iter().map(|id| {
                                let entry = self
                                    .histories
                                    .get(&project)
                                    .into_iter()
                                    .flatten()
                                    .find(|entry| entry.id == id);
                                let session = self
                                    .sessions
                                    .get(&(project.clone(), id.clone()))
                                    .map(|tab| tab.state.borrow());
                                let title = session.as_ref()
                                    .and_then(|v| v.session_name.clone())
                                    .or_else(|| entry.map(HistoryEntry::title))
                                    .unwrap_or_else(|| crate::i18n::ai::NEW_SESSION.get().into());
                                let waiting = session.as_ref().is_some_and(|v| v.pending_ui.is_some())
                                    || entry.is_some_and(|e| e.waiting);
                                let running = session.as_ref().is_some_and(|v| v.is_streaming)
                                    || entry.is_some_and(|e| e.running);
                                let current = active.as_ref() == Some(&id);
                                let open = id.clone();
                                let close = id.clone();
                                // Selected tab stays wider via flex_shrink_0; keep all tabs inside the clipped tab strip.
                                let tab_hover_bg = fg_alpha(palette.text, 0.07);
                                let tab = div()
                                    .id(SharedString::from(format!("tab-{id}")))
                                    .w(px(156.))
                                    .min_w_0()
                                    .flex_shrink(1.)
                                    .when(current, |tab| tab.flex_shrink_0().max_w(relative(0.65)))
                                    .h(px(crate::ui::TAB_HEIGHT))
                                    .px_2()
                                    .rounded(px(6.))
                                    .flex()
                                    .overflow_hidden()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .text_color(fg_alpha(palette.text, 0.78))
                                    .when(current, |s| {
                                        s.bg(gpui_kit::rgb(palette.surface))
                                            .text_color(gpui_kit::rgb(palette.text))
                                    })
                                    .when(!current, |s| {
                                        s.hover(move |h| {
                                            h.bg(tab_hover_bg)
                                                .text_color(gpui_kit::rgb(palette.text))
                                        })
                                    })
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        pane.activate(open.clone(), cx)
                                    }))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(gpui_kit::rgb(if waiting {
                                                palette.danger
                                            } else {
                                                palette.accent
                                            }))
                                            .child(if waiting {
                                                "!"
                                            } else if running {
                                                "●"
                                            } else {
                                                ""
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.5))
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(title),
                                    )
                                    .child(
                                        crate::ui::panel_toolbar_button(
                                            SharedString::from(format!("close-{id}")),
                                            palette,
                                            cx,
                                        )
                                            .w(px(18.))
                                            .h(px(18.))
                                            .flex_shrink_0()
                                            .icon(crate::ui::icon(IconName::Close).size(px(12.)))
                                            .tooltip(crate::i18n::ai::CLOSE_TAB_KEEP_RUNNING.get())
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                cx.stop_propagation();
                                                pane.close(&close, cx);
                                            })),
                                    );
                                tab.into_any_element()
                            }))
                            .child(
                                crate::ui::panel_toolbar_button("ai-add-tab", palette, cx)
                                    .flex_shrink_0()
                                    .ml_0p5()
                                    .icon(crate::ui::icon(IconName::Plus).size(px(15.)))
                                    .tooltip(crate::i18n::ai::NEW_SESSION.get())
                                    .on_click(cx.listener(|pane, _, _, cx| pane.create(cx))),
                            ),
                    )
                    .child(
                        crate::ui::panel_toolbar_button("ai-maximize", palette, cx)
                            .icon(crate::ui::icon(IconName::Maximize2).size(px(15.)))
                            .tooltip(crate::i18n::ai::MAXIMIZE_RESTORE.get())
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(AiPaneEvent::ToggleMaximize)),
                            ),
                    ),
            )
            .when_some(self.error.clone(), |root, error| {
                root.child(
                    div()
                        .px_3()
                        .py_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .bg(mix_color(palette.surface, palette.danger, 0.1))
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.danger))
                        .child(div().flex_1().min_w_0().child(error))
                        .child(
                            Button::new("dismiss-ai-error")
                                .ghost()
                                .compact()
                                .with_size(px(14.))
                                .w(px(18.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .icon(crate::ui::icon(IconName::Close))
                                .tooltip(crate::i18n::common::CLOSE.get())
                                .on_click(cx.listener(|pane, _, _, cx| pane.clear_error(cx))),
                        ),
                )
            })
            .when_some(
                active.and_then(|id| self.sessions.get(&(project, id)).map(|tab| tab.view.clone())),
                |root, view| root.child(div().flex_1().min_h_0().child(view)),
            )
            .when(self.drawer, |root| {
                let history = div()
                    .absolute()
                    .top(px(42.))
                    .left_2()
                    .child(self.history(cx));
                let history = if cx.reduce_motion() {
                    history.into_any_element()
                } else {
                    // Keep the animation state across history refreshes while the drawer is mounted.
                    history
                        .with_animation(
                            "ai-history-open",
                            Animation::new(Duration::from_millis(140))
                                .with_easing(ease_out_quint()),
                            |el, t| el.top(px(38. + 4. * t)).opacity(t),
                        )
                        .into_any_element()
                };
                root.child(deferred(history).with_priority(5))
            })
            .when_some(self.pending_deletion.clone(), |root, (id, active_task)| {
                let prompt = if active_task {
                    crate::i18n::ai::DELETE_ACTIVE_SESSION_MSG.get()
                } else {
                    crate::i18n::ai::DELETE_SESSION_MSG.get()
                };
                let confirm_label = if active_task {
                    crate::i18n::ai::STOP_AND_DELETE.get()
                } else {
                    crate::i18n::common::DELETE.get()
                };
                let confirm_card = crate::ui::popover_card("ai-delete-confirm")
                    .w(px(284.))
                    .p_3p5()
                    .font_family(crate::ui::sans_font(&self.settings.ai_font))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_size(px(12.5))
                    .child(div().text_color(gpui_kit::rgb(palette.text)).child(prompt))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                crate::ui::button_secondary(
                                    "cancel-ai-delete",
                                    crate::i18n::common::CANCEL.get(),
                                )
                                .text_size(px(12.))
                                .px_2p5()
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.pending_deletion = None;
                                    cx.notify();
                                })),
                            )
                            .child(
                                crate::ui::button_danger("confirm-ai-delete", confirm_label)
                                    .text_size(px(12.))
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        pane.delete(id.clone(), active_task, cx)
                                    })),
                            ),
                    );
                root.child(
                    deferred(
                        div()
                            .absolute()
                            .top(px(44.))
                            .left_2()
                            .child(crate::ui::popover_enter(
                                confirm_card,
                                "ai-delete-confirm-enter",
                                crate::ui::PopoverDirection::Down,
                            )),
                    )
                    .with_priority(6),
                )
            })
    }
}
