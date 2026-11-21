//! Project-scoped tab ownership. Conversation entities stay alive while their tasks run off screen.
use super::*;
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
                    "新会话".into()
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
    views: HashMap<(String, String), Entity<ChatPane>>,
    subscriptions: HashMap<(String, String), Vec<Subscription>>,
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

impl AiPane {
    pub fn new(
        cwd: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索会话标题…"));
        let rename = cx.new(|cx| InputState::new(window, cx).placeholder("会话名称"));
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
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(35))
                    .await;
                if this
                    .update(cx, |pane: &mut Self, cx| pane.poll(cx))
                    .is_err()
                {
                    break;
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
            views: HashMap::new(),
            subscriptions: HashMap::new(),
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
            self.error = Some(format!("保存标签失败：{error}"));
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
    pub fn active_tasks(&self, cx: &App) -> Vec<(String, String, bool)> {
        let project = self.project();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        if let Some(entries) = self.histories.get(&project) {
            for entry in entries {
                let view = self
                    .views
                    .get(&(project.clone(), entry.id.clone()))
                    .map(|v| v.read(cx));
                let waiting = view.is_some_and(|v| v.pending_ui.is_some()) || entry.waiting;
                let running = view.is_some_and(|v| v.is_streaming) || entry.running;
                if running || waiting {
                    let title = view
                        .and_then(|v| v.session_name.clone())
                        .unwrap_or_else(|| entry.title());
                    seen.insert(entry.id.clone());
                    out.push((entry.id.clone(), title, waiting));
                }
            }
        }
        for ((proj, id), view) in &self.views {
            if proj != &project || seen.contains(id) {
                continue;
            }
            let v = view.read(cx);
            let waiting = v.pending_ui.is_some();
            if v.is_streaming || waiting {
                let title = v
                    .session_name
                    .clone()
                    .unwrap_or_else(|| "新会话".into());
                out.push((id.clone(), title, waiting));
            }
        }
        out
    }

    pub fn activate(&mut self, id: String, cx: &mut Context<Self>) {
        let project = self.project();
        if self.views.contains_key(&(project.clone(), id.clone())) {
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
            .views
            .get(&identity)
            .is_some_and(|view| view.read(cx).is_streaming || view.read(cx).pending_ui.is_some());
        if self.views.contains_key(&identity) {
            self.command(json!({"type":"release", "sessionId":id}), cx);
            if !running {
                self.views.remove(&identity);
                self.subscriptions.remove(&identity);
            }
        }
        self.save();
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
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
                                    self.views.retain(|(owner, id), _| {
                                        owner != &project || tabs.tabs.contains(id)
                                    });
                                    self.subscriptions.retain(|(owner, id), _| {
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
                            Err(error) => self.set_error(format!("会话列表格式错误：{error}"), cx),
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
                                self.views.remove(&(project.clone(), id.clone()));
                                self.subscriptions.remove(&(project.clone(), id));
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
                RpcInbound::Event(event) => {
                    let kind = event["type"].as_str().unwrap_or("");
                    // Project control routes stay alive even when the originating tab is hidden.
                    match kind {
                        "annotation_request" | "annotation_cancel" => cx.emit(AiPaneEvent::Annotation(event.clone())),
                        "tag_request" | "tag_cancel" => cx.emit(AiPaneEvent::TagRequest(event.clone())),
                        _ => {}
                    }
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
                            self.views.remove(&(project.clone(), id.clone()));
                            self.subscriptions.remove(&(project.clone(), id));
                        }
                    }
                }
                RpcInbound::Exited(_) => {
                    self.set_error(
                        "AI 宿主已退出；重新打开 AI 面板以恢复历史，任务不会自动重发。",
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
    pub fn respond_annotation(&self, request: &Value, result: Value) {
        let project = request["projectKey"].as_str().expect("host annotation project");
        if let Some(client) = self.controls.get(project) {
            let _ = client.send(json!({ "type": "annotation_response", "id": request["id"],
                "sessionId": request["sessionId"], "result": result }));
        }
    }
    pub fn respond_tag(&self, request: &Value, result: Value) {
        let project = request["projectKey"].as_str().expect("host tag project");
        if let Some(client) = self.controls.get(project) {
            let _ = client.send(json!({ "type": "tag_response", "id": request["id"],
                "sessionId": request["sessionId"], "result": result }));
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
            if let Some(view) = self.views.get(&(self.project(), id.clone())) {
                let focus = view.read(cx).input.read(cx).focus_handle(cx);
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
        for ((project, _), view) in &self.views {
            if project == &self.project_key {
                view.update(cx, |view, cx| view.set_project_name(name.clone(), cx));
            }
        }
        cx.notify();
    }
    pub fn sync_settings(&mut self, settings: &AppSettings, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        if self.host.is_some() {
            self.command(json!({"type":"configure", "settings":settings.ai}), cx);
        }
        for view in self.views.values() {
            view.update(cx, |view, cx| view.sync_settings(settings, cx));
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
            if let Some(view) = self.views.get(&(self.project(), id)) {
                view.update(cx, |view, cx| view.set_active_file(file, cx));
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
        if let Some(view) = self.views.get(&identity) {
            if !Arc::ptr_eq(&view.read(cx).host, &host) {
                view.update(cx, |view, cx| {
                    view.host = host;
                    view.client = None;
                    view.last_sequence = 0;
                    view.open(window, cx);
                });
            }
            if let Some((source, file, text)) = self.pending_selections.remove(&identity.0) {
                view.update(cx, |view, cx| {
                    view.attach_selection(source, file, text, window, cx)
                });
            }
            return;
        }
        let view = cx.new(|cx| {
            ChatPane::new(
                self.cwd.clone(),
                self.settings.clone(),
                self.math.clone(),
                host,
                id,
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
            AiPaneEvent::Annotation(_) | AiPaneEvent::TagRequest(_) =>
                unreachable!("workspace tool requests use the project control route"),
            AiPaneEvent::UndoTags(items) => cx.emit(AiPaneEvent::UndoTags(items.clone())),
            AiPaneEvent::OpenFile(path) => cx.emit(AiPaneEvent::OpenFile(path.clone())),
            AiPaneEvent::BackToHub => cx.emit(AiPaneEvent::BackToHub),
            AiPaneEvent::ToggleMaximize => cx.emit(AiPaneEvent::ToggleMaximize),
        });
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        self.subscriptions
            .insert(identity.clone(), vec![subscription, observe]);
        self.views.insert(identity, view);
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
        let font_size = (self.settings.ai_font_size as f32 - 2.).max(10.);
        let metadata_color = mix_color(palette.surface, palette.muted, 0.65);
        let pinned_style = ButtonCustomVariant::new(cx)
            .color(mix_color(palette.surface, palette.accent, 0.14).into())
            .foreground(gpui_kit::rgb(palette.accent).into())
            .hover(mix_color(palette.surface, palette.accent, 0.22).into())
            .active(gpui_kit::rgb(palette.pressed).into());
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
        div().w(px(300.)).max_h(px(340.)).p_1().rounded(px(10.)).border_1().text_size(px(font_size))
            .font_family(crate::ui::sans_font(&self.settings.ai_font))
            .border_color(subtle_border(palette.border)).bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark())).flex().flex_col().gap_1().occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().w_full().px_1().pt_0p5().child(Input::new(&self.search).xsmall().h(px(20.)).text_size(px(font_size))))
            .child(div().relative().min_h_0().child(div().id("ai-history-list").max_h(px(285.)).overflow_y_scroll().track_scroll(&self.history_scroll)
                .children(rows.into_iter().map(|(_, entry)| {
                    let id = entry.id.clone(); let open = id.clone(); let pin = id.clone(); let rename = id.clone(); let delete = id.clone();
                    let running = entry.running; let waiting = entry.waiting;
                    let title = entry.title(); let initial = title.clone(); let pinned = entry.pinned;
                    let status = if waiting { "等待操作" } else if running { "运行中" } else if entry.failed { "失败" } else { "" };
                    div().id(SharedString::from(format!("history-{id}"))).px_2().py_1p5().rounded(px(6.))
                        .when(pinned, |row| row.bg(mix_color(palette.surface, palette.accent, 0.045)))
                        .cursor_pointer().hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            if pane.renaming.as_ref() != Some(&open) { pane.activate(open.clone(), cx); }
                        }))
                        .child(div().flex().items_center().gap_1()
                            .child(if self.renaming.as_ref() == Some(&id) { div().flex_1().min_w_0().on_mouse_down(MouseButton::Left, |_,_,cx| cx.stop_propagation()).child(Input::new(&self.rename).small().text_size(px(font_size))).into_any_element() }
                                else { div().flex_1().min_w_0().text_ellipsis().child(title).into_any_element() })
                            .child(Button::new(SharedString::from(format!("pin-{id}"))).ghost().when(pinned, |button| button.custom(pinned_style)).compact().with_size(px(font_size / 0.75)).w(px(font_size + 6.)).h(px(font_size + 6.)).icon(crate::ui::icon(IconName::Pin)).tooltip(if pinned { "取消置顶" } else { "置顶" })
                                .on_click(cx.listener(move |pane, _, _, cx| { cx.stop_propagation(); pane.command(json!({"type":"set_pinned","sessionId":pin,"pinned":!pinned}), cx); })))
                            .child(Button::new(SharedString::from(format!("rename-{id}"))).ghost().compact().with_size(px(font_size / 0.75)).w(px(font_size + 6.)).h(px(font_size + 6.)).icon(crate::ui::icon(IconName::PencilLine)).tooltip("重命名")
                                .on_click(cx.listener(move |pane, _, window, cx| { cx.stop_propagation(); pane.renaming = Some(rename.clone()); pane.rename.update(cx, |input,cx| { input.set_value(initial.clone(),window,cx); input.focus(window,cx); }); cx.notify(); })))
                            .child(Button::new(SharedString::from(format!("delete-{id}"))).ghost().compact().with_size(px(font_size / 0.75)).w(px(font_size + 6.)).h(px(font_size + 6.)).icon(crate::ui::icon(IconName::Trash)).tooltip("删除会话")
                                .on_click(cx.listener(move |pane, _, _, cx| { cx.stop_propagation(); pane.pending_deletion = Some((delete.clone(), running || waiting)); cx.notify(); }))))
                        .child(div().flex().justify_between().text_size(px((font_size - 2.).max(9.))).text_color(metadata_color)
                            .child(format!("{}{} 条 {}", if pinned { "置顶 · " } else { "" }, entry.count, status))
                            .child(chrono::DateTime::parse_from_rfc3339(&entry.modified).map(|t| t.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_default()))
                })))
                .child(div().absolute().right_0().top_0().bottom_0().child(Scrollbar::vertical(&self.history_scroll).mode(ScrollbarMode::Always)
                    .styles(|styles| styles.thumb(|style| style.bg(gpui_kit::rgb(0xc5c8cc)).width(px(5.)))))))
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
            if let Some(view) = self.displayed_view.as_ref().and_then(|key| self.views.get(key)) {
                view.update(cx, |view, cx| view.dismiss_quote(cx));
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
                    .h(px(32.))
                    .px_1p5()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .bg(gpui_kit::rgb(header_bg))
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .child(
                        Button::new("ai-back-to-hub")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(22.))
                            .h(px(22.))
                            .icon(crate::ui::icon(IconName::ChevronLeft))
                            .tooltip("返回功能栏")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AiPaneEvent::BackToHub))),
                    )
                    .child(
                        Button::new("ai-history")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(22.))
                            .h(px(22.))
                            .icon(crate::ui::icon(IconName::Clock))
                            .tooltip("会话历史")
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
                                let view = self
                                    .views
                                    .get(&(project.clone(), id.clone()))
                                    .map(|v| v.read(cx));
                                let title = view
                                    .and_then(|v| v.session_name.clone())
                                    .or_else(|| entry.map(HistoryEntry::title))
                                    .unwrap_or_else(|| "新会话".into());
                                let waiting = view.is_some_and(|v| v.pending_ui.is_some())
                                    || entry.is_some_and(|e| e.waiting);
                                let running = view.is_some_and(|v| v.is_streaming)
                                    || entry.is_some_and(|e| e.running);
                                let current = active.as_ref() == Some(&id);
                                let open = id.clone();
                                let close = id.clone();
                                // Selected tab stays wider via flex_shrink_0; keep all tabs inside the clipped tab strip.
                                let tab_hover_bg = fg_alpha(palette.text, 0.07);
                                let tab = div()
                                    .id(SharedString::from(format!("tab-{id}")))
                                    .w(px(150.))
                                    .min_w_0()
                                    .flex_shrink(1.)
                                    .when(current, |tab| tab.flex_shrink_0().max_w(relative(0.65)))
                                    .h(px(26.))
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
                                            .text_xs()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(title),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("close-{id}")))
                                            .ghost()
                                            .compact()
                                            .with_size(px(16.))
                                            .w(px(18.))
                                            .h(px(18.))
                                            .flex_shrink_0()
                                            .icon(crate::ui::icon(IconName::Close))
                                            .tooltip("关闭标签，任务继续运行")
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                cx.stop_propagation();
                                                pane.close(&close, cx);
                                            })),
                                    );
                                tab.into_any_element()
                            }))
                            .child(
                                Button::new("ai-add-tab")
                                    .ghost()
                                    .compact()
                                    .with_size(px(16.))
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex_shrink_0()
                                    .ml_1()
                                    .icon(crate::ui::icon(IconName::Plus))
                                    .tooltip("新建会话")
                                    .on_click(cx.listener(|pane, _, _, cx| pane.create(cx))),
                            ),
                    )
                    .child(
                        Button::new("ai-maximize")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(22.))
                            .h(px(22.))
                            .icon(crate::ui::icon(IconName::Maximize2))
                            .tooltip("最大化 / 还原")
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
                                .tooltip("关闭")
                                .on_click(cx.listener(|pane, _, _, cx| pane.clear_error(cx))),
                        ),
                )
            })
            .when_some(
                active.and_then(|id| self.views.get(&(project, id)).cloned()),
                |root, view| root.child(div().flex_1().min_h_0().child(view)),
            )
            .when(self.drawer, |root| {
                root.child(
                    deferred(
                        div()
                            .absolute()
                            .top(px(40.))
                            .left_2()
                            .child(self.history(cx)),
                    )
                    .with_priority(5),
                )
            })
            .when_some(self.pending_deletion.clone(), |root, (id, active_task)| {
                let prompt = if active_task {
                    "停止任务并删除该会话？"
                } else {
                    "确认删除该会话？此操作不可恢复。"
                };
                let confirm_label = if active_task { "停止并删除" } else { "删除" };
                root.child(
                    deferred(
                        div()
                            .absolute()
                            .top(px(45.))
                            .left_2()
                            .p_3()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(subtle_border(palette.border))
                            .bg(gpui_kit::rgb(palette.surface))
                            .shadow(popover_shadow(palette.is_dark()))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_xs()
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(prompt)
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("cancel-ai-delete")
                                            .small()
                                            .label("取消")
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.pending_deletion = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("confirm-ai-delete")
                                            .small()
                                            .danger()
                                            .label(confirm_label)
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                pane.delete(id.clone(), active_task, cx)
                                            })),
                                    ),
                            ),
                    )
                    .with_priority(6),
                )
            })
    }
}
