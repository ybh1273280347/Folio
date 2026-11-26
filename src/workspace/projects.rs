//! Project selection, workspace restoration and navigation persistence.

use super::*;
use super::documents::DocumentWatch;
use crate::model::{ProjectConfig, WorkspaceSession};

pub(super) struct ProjectMenu {
    pub(super) open: bool,
    pub(super) search: Entity<InputState>,
}

impl ProjectMenu {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::workspace::SEARCH_PROJECTS_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &search,
            window,
            |app, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    if !app.project_menu.open {
                        return;
                    }
                    let query = input.read(cx).value().trim().to_lowercase();
                    let matched_id = app
                        .projects
                        .registered()
                        .iter()
                        .find(|project| {
                            query.is_empty() || project.name.to_lowercase().contains(&query)
                        })
                        .map(|project| project.id.clone());
                    if let Some(id) = matched_id {
                        app.close_project_menu(cx);
                        app.trash.dismiss();
                        app.switch_project(&id, window, cx);
                    }
                }
                _ => {}
            },
        )
        .detach();
        Self { open: false, search }
    }

    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |input, cx| {
            input.set_placeholder(
                crate::i18n::workspace::SEARCH_PROJECTS_PLACEHOLDER.get(),
                window,
                cx,
            );
        });
    }
}

/// Project registration and saved navigation. The session key is set only after
/// selection preflight succeeds, so startup never saves an empty window into a session.
pub(super) struct Projects {
    default_workspace: Project,
    config: ProjectConfig,
    workspace_sessions: BTreeMap<String, WorkspaceSession>,
    active_session: Option<String>,
}

impl Projects {
    pub(super) fn load() -> (Self, WorkspaceLocation) {
        let config = storage::load_projects().expect(crate::i18n::workspace::ERR_LOAD_PROJECTS.get());
        let mut workspace_sessions =
            storage::load_workspace_sessions().expect(crate::i18n::workspace::ERR_LOAD_SESSIONS.get());
        // The old default session contained external paths, never default-space files.
        if let Some(session) = workspace_sessions.remove("default") {
            workspace_sessions.insert("loose".into(), session);
        }
        // On cold start, keep only the last active tab per workspace instead of inheriting all old tabs.
        for session in workspace_sessions.values_mut() {
            let keep = if !session.active.is_empty() && session.tabs.contains(&session.active) {
                Some(session.active.clone())
            } else {
                session.tabs.last().cloned()
            };
            session.tabs = keep.iter().cloned().collect();
            session.active = keep.unwrap_or_default();
        }
        let initial_location = config.active_workspace.clone().unwrap_or_else(|| {
            config.active_id.clone().map(WorkspaceLocation::Project).unwrap_or(
                if workspace_sessions.contains_key("loose") {
                    WorkspaceLocation::LooseFiles
                } else { WorkspaceLocation::Default }
            )
        });
        let default_workspace = Project {
            id: "__folio_default".into(),
            name: "默认空间".into(),
            folder: dirs::home_dir().expect("用户主目录存在").join(".Folio").to_string_lossy().into_owned(),
            main_file: String::new(),
            engine: Engine::Xelatex,
        };
        (Self { default_workspace, config, workspace_sessions, active_session: None }, initial_location)
    }

    pub(super) fn directory(&self) -> Option<&Project> {
        if self.is_default() {
            return Some(&self.default_workspace);
        }
        self.config
            .projects
            .iter()
            .find(|project| Some(&project.id) == self.config.active_id.as_ref())
    }

    pub(super) fn active_id(&self) -> Option<&str> {
        self.config.active_id.as_deref()
    }

    pub(super) fn is_default(&self) -> bool {
        self.config.active_workspace == Some(WorkspaceLocation::Default)
    }

    pub(super) fn registered(&self) -> &[Project] {
        &self.config.projects
    }

    /// Moves `id` to the front of recent projects and prunes IDs no longer registered.
    pub(super) fn touch_recent_project(&mut self, id: &str) {
        if !self.config.projects.iter().any(|project| project.id == id) {
            return;
        }
        self.config
            .recent_project_ids
            .retain(|existing| existing != id && self.config.projects.iter().any(|p| p.id == *existing));
        self.config.recent_project_ids.insert(0, id.to_owned());
    }

    /// Returns up to `limit` recently visited projects, backfilling from registration order.
    pub(super) fn recent_projects(&self, limit: usize) -> Vec<&Project> {
        if limit == 0 {
            return Vec::new();
        }
        let mut ordered = Vec::with_capacity(limit.min(self.config.projects.len()));
        for id in &self.config.recent_project_ids {
            if let Some(project) = self.config.projects.iter().find(|project| &project.id == id)
                && !ordered.iter().any(|added: &&Project| added.id == project.id)
            {
                ordered.push(project);
                if ordered.len() >= limit {
                    return ordered;
                }
            }
        }
        for project in &self.config.projects {
            if !ordered.iter().any(|added| added.id == project.id) {
                ordered.push(project);
                if ordered.len() >= limit {
                    break;
                }
            }
        }
        ordered
    }

    pub(super) fn loose_tab_count(&self) -> usize {
        self.workspace_sessions.get("loose").map_or(0, |session| session.tabs.len())
    }

    pub(super) fn save(&self) -> io::Result<()> {
        storage::save_projects(&self.config)
    }

    fn active_project_mut(&mut self) -> Option<&mut Project> {
        self.config.projects.iter_mut()
            .find(|project| Some(&project.id) == self.config.active_id.as_ref())
    }

    pub(super) fn rename(&mut self, id: &str, name: String) -> io::Result<()> {
        if name.is_empty() {
            return Err(io::Error::other(crate::i18n::workspace::ERR_PROJECT_NAME_EMPTY.get()));
        }
        self.config.projects.iter_mut().find(|project| project.id == id)
            .expect("menu project exists").name = name;
        self.save()
    }

    pub(super) fn relocate_main_file(&mut self, old: &str, new: &str) {
        if self.default_workspace.main_file == old
            && self.directory().is_some_and(|p| p.id == self.default_workspace.id)
        {
            self.default_workspace.main_file = new.to_owned();
        }
        if let Some(project) = self.active_project_mut()
            && project.main_file == old
        {
            project.main_file = if new.to_ascii_lowercase().ends_with(".tex") { new.to_owned() } else { String::new() };
        }
    }

    pub(super) fn remove_main_file(&mut self, id: &str) -> io::Result<()> {
        if id == self.default_workspace.id {
            self.default_workspace.main_file.clear();
        } else if let Some(project) = self.config.projects.iter_mut().find(|project| project.id == id) {
            project.main_file.clear();
        }
        self.save()
    }

    pub(super) fn restore_main_file(&mut self, id: &str, path: &str) -> io::Result<()> {
        if id == self.default_workspace.id && self.default_workspace.main_file.is_empty() {
            self.default_workspace.main_file = path.to_owned();
        } else if let Some(project) = self.config.projects.iter_mut().find(|project| project.id == id && project.main_file.is_empty()) {
            project.main_file = path.to_owned();
            self.save()?;
        }
        Ok(())
    }
}

impl Workspace {
    pub(super) fn set_engine(&mut self, engine: Engine, cx: &mut Context<Self>) {
        if self.build.is_running() {
            return;
        }
        if let Some(project) = self.projects.active_project_mut() {
            project.engine = engine;
            if let Err(error) = self.projects.save() {
                self.report(error, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn set_main_file(&mut self, file: &str, cx: &mut Context<Self>) {
        if self.build.is_running() {
            return;
        }
        self.projects.active_project_mut().expect("main file belongs to project").main_file = file.to_owned();
        if let Err(error) = self.projects.save() {
            self.report(error, cx);
        }
        cx.notify();
    }

    pub(super) fn active_project_name(&self) -> Option<String> {
        self.projects.directory()
            .filter(|project| project.id != "__folio_default")
            .map(|project| project.name.clone())
    }

    pub(super) fn terminal_directory(&self) -> PathBuf {
        self.projects.directory().map(|project| PathBuf::from(&project.folder))
            .or_else(|| (!self.documents.active().is_empty()).then(|| self.document_path(self.documents.active())).and_then(|path| path.parent().map(Path::to_path_buf)))
            .unwrap_or_else(|| dirs::home_dir().expect("user home"))
    }

    // Save only navigation state; documents themselves remain authoritative on disk.
    pub(super) fn remember_session(&mut self) -> io::Result<()> {
        let Some(key) = self.projects.active_session.clone() else {
            return Ok(());
        };
        self.projects.workspace_sessions.insert(
            key,
            WorkspaceSession {
                tabs: self.documents.tabs().to_vec(),
                active: self.documents.active().to_owned(),
                reading_panes: self.documents.reading_panes().to_vec(),
                collapsed_folders: Some(self.tree.collapsed_folders().clone()),
            },
        );
        storage::save_workspace_sessions(&self.projects.workspace_sessions)
    }

    pub(super) fn select_loose_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.build.is_busy() {
            return Err(io::Error::other(crate::i18n::workspace::ERR_WAIT_BUILD_SWITCH_WORKSPACE.get()));
        }
        self.remember_session()?;
        let watcher = DocumentWatch::new(None)?;
        self.trash.dismiss();
        self.tags.reset();
        self.tool_requests.clear_tag_proposals();
        self.documents.replace_watch(watcher);
        self.projects.config.active_id = None;
        self.projects.config.active_workspace = Some(WorkspaceLocation::LooseFiles);
        self.projects.active_session = Some("loose".into());
        self.tree.clear_directory();
        self.build.reset_workspace(window, cx);
        self.clear_open_documents(window, cx);

        let session = self.projects.workspace_sessions
            .get("loose")
            .cloned()
            .unwrap_or_default();
        for path in &session.tabs {
            if Path::new(path).is_file() {
                self.open_standalone_file(Path::new(path), window, cx)?;
            }
        }
        let active = if self.documents.tabs().contains(&session.active) {
            session.active
        } else {
            self.documents.tabs().last().cloned().unwrap_or_default()
        };
        for path in &session.reading_panes {
            self.open_reading_key(path, window, cx)?;
        }
        self.documents.restore_active_tab(&active);
        self.refresh_tags(cx);
        self.projects.save()?;
        self.remember_session()?;
        self.reset_rag_sync_on_project_switch(cx);
        cx.notify();
        Ok(())
    }

    pub(super) fn select_project(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.build.is_busy() {
            return Err(io::Error::other(crate::i18n::workspace::ERR_WAIT_BUILD_SWITCH_WORKSPACE.get()));
        }
        self.remember_session()?;
        let default_space = id == "__folio_default";
        let project = if default_space {
            std::fs::create_dir_all(&self.projects.default_workspace.folder)?;
            self.projects.default_workspace.clone()
        } else {
            self.projects.config.projects.iter().find(|project| project.id == id)
                .expect("selected project exists").clone()
        };
        let root = Path::new(&project.folder);
        // Keep the active project's documents and watcher usable until the new root
        // has a working watcher and a complete directory listing.
        let watcher = DocumentWatch::new(Some(root))?;
        let files = files::list_files(root)?;
        let folders = files::list_folders(root)?;
        let previous_pdf = storage::data_directory()?
            .join("builds")
            .join(&project.id)
            .join("preview.pdf");

        self.trash.dismiss();
        self.tags.reset();
        self.tool_requests.clear_tag_proposals();
        self.projects.active_session = Some(id.to_owned());
        if default_space {
            self.projects.config.active_id = None;
            self.projects.config.active_workspace = Some(WorkspaceLocation::Default);
        } else {
            self.projects.config.active_id = Some(id.to_owned());
            self.projects.config.active_workspace = Some(WorkspaceLocation::Project(id.to_owned()));
            self.projects.touch_recent_project(id);
        }
        self.documents.replace_watch(watcher);
        self.tree.replace_listing(files, folders);
        self.refresh_tags(cx);
        self.notice = None;
        self.build.reset_workspace(window, cx);
        self.clear_open_documents(window, cx);
        self.tree.reset_selection();
        if previous_pdf.exists() {
            self.publish_build_preview(previous_pdf, window, cx);
        }
        let session = self.projects.workspace_sessions.get(id).cloned().unwrap_or_default();
        self.tree.restore_collapsed_folders(session.collapsed_folders);
        for path in &session.tabs {
            if let Some(file) = self.tree.files().iter().find(|file| &file.path == path).cloned() {
                self.open_file(&file, window, cx)?;
            }
        }
        if self.documents.tabs().is_empty() {
            let first = self
                .tree.files()
                .iter()
                .find(|file| file.path == project.main_file)
                .or_else(|| self.tree.files().iter().find(|file| file.kind == FileKind::Text))
                .cloned();
            if let Some(first) = first {
                self.open_file(&first, window, cx)?;
            }
        }
        for path in &session.reading_panes {
            if let Some(file) = self.tree.files().iter().find(|file| &file.path == path).cloned() {
                self.open_reading_pane(&file, window, cx);
            }
        }
        self.documents.restore_active_tab(&session.active);
        self.queue_math_preload(
            self.tree.files()
                .iter()
                .filter(|file| file.path.to_lowercase().ends_with(".md"))
                .map(|file| Path::new(&project.folder).join(&file.path))
                .collect(),
            cx,
        );
        self.projects.save()?;
        self.remember_session()?;
        self.reset_rag_sync_on_project_switch(cx);
        cx.notify();
        Ok(())
    }
    pub(super) fn switch_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let retry_id = id.to_owned();
        if self.build.is_busy()
            || !self.confirm_drafts(cx, move |app, window, cx| {
                app.switch_project(&retry_id, window, cx)
            })
        {
            return;
        }
        if let Err(error) = self.select_project(id, window, cx) {
            self.report(error, cx);
        }
    }
    pub(super) fn switch_to_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.projects.is_default() {
            return;
        }
        if self.build.is_busy()
            || !self.confirm_drafts(cx, |app, window, cx| {
                app.switch_to_default(window, cx)
            })
        {
            return;
        }
        if let Err(error) = self.select_project("__folio_default", window, cx) {
            self.report(error, cx);
        }
    }
    pub(super) fn add_folder(&mut self, folder: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let previous_id = self.projects.config.active_id.clone();
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
            return;
        }
        match storage::add_project(&mut self.projects.config, folder) {
            Ok(project) => {
                self.projects.config.active_id = previous_id;
                if let Err(error) = self.select_project(&project.id, window, cx) {
                    self.report(error, cx);
                }
            }
            Err(error) => self.report(error, cx),
        }
    }
    pub(super) fn add_project(&mut self, _: &OpenProject, window: &mut Window, cx: &mut Context<Self>) {
        if self.build.is_busy()
            || !self.confirm_drafts(cx, |app, window, cx| {
                app.add_project(&OpenProject, window, cx)
            })
        {
            return;
        }
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(crate::i18n::workspace::DIALOG_SELECT_PROJECT_FOLDER.get());
        self.dialogs.set_native_open(true);
        cx.spawn_in(window, async move |this, cx| {
            let folder = dialog.pick_folder().await;
            let _ = this.update_in(cx, |app, window, cx| {
                app.dialogs.set_native_open(false);
                if let Some(folder) = folder {
                    app.add_folder(folder.path(), window, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn open_file_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(crate::i18n::workspace::DIALOG_OPEN_DOCUMENT.get())
            .add_filter(
                crate::i18n::workspace::DIALOG_SUPPORTED_FILES.get(),
                &[
                    "pdf", "md", "tex", "ltx", "bib", "sty", "cls", "html", "htm", "drawio",
                ],
            );
        self.dialogs.set_native_open(true);
        cx.spawn_in(window, async move |this, cx| {
            let selected = dialog.pick_file().await;
            let _ = this.update_in(cx, |app, window, cx| {
                app.dialogs.set_native_open(false);
                if let Some(file) = selected
                    && let Err(error) = app.open_path(file.path(), window, cx)
                {
                    app.report(error, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let paths = if paths
            .first()
            .is_some_and(|path| path.to_string_lossy() == "--open")
        {
            paths.into_iter().skip(1).collect()
        } else {
            paths
        };
        let paths: Result<Vec<_>, _> = paths
            .into_iter()
            .map(|path| dunce::canonicalize(path))
            .collect();
        let paths = match paths {
            Ok(paths) => paths,
            Err(error) => {
                self.report(error, cx);
                return;
            }
        };
        if paths.len() == 1 && paths[0].is_dir() {
            self.add_folder(&paths[0], window, cx);
            return;
        }
        if paths.iter().any(|path| path.is_dir()) {
            self.report(crate::i18n::workspace::ERR_SINGLE_PROJECT_OR_DOCS.get(), cx);
            return;
        }
        let mut project_files = Vec::new();
        let mut default_space_files = Vec::new();
        let mut default_files = Vec::new();
        let project_root = self.projects.directory()
            .map(|project| dunce::canonicalize(&project.folder));
        for path in paths {
            if !files::is_relevant(&path) {
                self.report(
                    crate::i18n::workspace::err_unsupported_file_type(&path),
                    cx,
                );
                return;
            }
            if let Some(Ok(root)) = &project_root
                && let Ok(relative) = path.strip_prefix(root)
            {
                let relative = relative.to_string_lossy().replace('\\', "/");
                project_files.push(ProjectFile {
                    kind: Self::file_kind(&path),
                    path: relative,
                });
            } else if path.starts_with(&self.projects.default_workspace.folder) {
                default_space_files.push(path);
            } else {
                default_files.push(path);
            }
        }
        for file in project_files {
            if let Err(error) = self.open_file(&file, window, cx) {
                self.report(error, cx);
                return;
            }
        }
        if !default_space_files.is_empty() {
            let retry_paths = default_space_files.clone();
            if !self.confirm_drafts(cx, move |app, window, cx| app.open_paths(retry_paths.clone(), window, cx)) {
                return;
            }
            if let Err(error) = self.select_project("__folio_default", window, cx) {
                self.report(error, cx);
                return;
            }
            for path in default_space_files {
                let relative = path.strip_prefix(&self.projects.default_workspace.folder).unwrap().to_string_lossy().replace('\\', "/");
                let file = ProjectFile { kind: Self::file_kind(&path), path: relative };
                if let Err(error) = self.open_file(&file, window, cx) { self.report(error, cx); return; }
            }
        }
        if default_files.is_empty() {
            return;
        }
        if self.projects.directory().is_some() {
            let retry_paths = default_files.clone();
            if !self.confirm_drafts(cx, move |app, window, cx| {
                app.open_paths(retry_paths.clone(), window, cx);
            }) {
                return;
            }
            if let Err(error) = self.select_loose_files(window, cx) {
                self.report(error, cx);
                return;
            }
        }
        for path in default_files {
            if let Err(error) = self.open_standalone_file(&path, window, cx) {
                self.report(error, cx);
                return;
            }
        }
        self.refresh_tags(cx);
    }

    pub(super) fn open_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.open_paths(vec![path.to_path_buf()], window, cx);
        Ok(())
    }

    pub(super) fn remove_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let retry_id = id.to_owned();
        let removing = self.dialogs.has_response(Dialog::RemoveProject);
        if self.build.is_busy()
            || (!removing
                && !self.confirm_drafts(cx, move |app, window, cx| {
                    app.remove_project(&retry_id, window, cx)
                }))
        {
            return;
        }
        let retry_id = id.to_owned();
        if self.ask(
            Dialog::RemoveProject,
            crate::i18n::workspace::DIALOG_REMOVE_PROJECT_TITLE.get().into(),
            crate::i18n::workspace::DIALOG_REMOVE_PROJECT_BODY.get().into(),
            DialogButtons::Confirm(crate::i18n::workspace::DIALOG_REMOVE_PROJECT_CONFIRM.get().into()),
            cx,
            move |app, window, cx| app.remove_project(&retry_id, window, cx),
        ) != Some(rfd::MessageDialogResult::Ok)
        {
            return;
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
            return;
        }
        self.projects.config.projects.retain(|project| project.id != id);
        self.projects.config.recent_project_ids.retain(|recent_id| recent_id != id);
        self.projects.workspace_sessions.remove(id);
        if self.projects.config.active_id.as_deref() == Some(id) {
            self.projects.config.active_id = None;
            self.projects.active_session = None;
            if let Some(project) = self.projects.config.projects.first() {
                let id = project.id.clone();
                if let Err(error) = self.select_project(&id, window, cx) {
                    self.report(error, cx);
                }
            } else if let Err(error) = self.select_loose_files(window, cx) {
                self.report(error, cx);
            }
        }
        if let Err(error) = self.projects.save() {
            self.report(error, cx);
        }
        cx.notify();
    }

    pub(super) fn is_project_menu_open(&self) -> bool {
        self.project_menu.open
    }

    pub(super) fn toggle_project_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_menu.open {
            self.close_project_menu(cx);
        } else {
            self.open_project_menu(window, cx);
        }
    }

    pub(super) fn open_project_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_menu.open = true;
        let search = self.project_menu.search.clone();
        search.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        window.defer(cx, move |window, cx| {
            search.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }

    pub(super) fn close_project_menu(&mut self, cx: &mut Context<Self>) {
        if self.project_menu.open {
            self.project_menu.open = false;
            cx.notify();
        }
    }

    pub(super) fn render_project_menu_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.project_menu.open || self.zen_mode {
            return None;
        }
        let palette = self.settings.theme.palette;
        let query = self
            .project_menu
            .search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let all_projects = self.projects.registered();
        let filtered: Vec<&Project> = all_projects
            .iter()
            .filter(|project| query.is_empty() || project.name.to_lowercase().contains(&query))
            .collect();
        let active_id = if self.trash.is_open() {
            None
        } else {
            self.projects.active_id()
        };

        let hover_bg = gpui_kit::rgb(palette.hover);
        let active_bg = gpui_kit::rgb(palette.selection);
        let muted = gpui_kit::rgb(palette.muted);
        let text = gpui_kit::rgb(palette.text);
        let accent = gpui_kit::rgb(palette.accent);
        let danger = gpui_kit::rgb(palette.danger);

        let list_body = if all_projects.is_empty() {
            div()
                .px_3()
                .py_4()
                .text_xs()
                .text_color(muted)
                .child(crate::i18n::workspace::NO_PROJECTS_YET.get())
                .into_any_element()
        } else if filtered.is_empty() {
            div()
                .px_3()
                .py_4()
                .text_xs()
                .text_color(muted)
                .child(crate::i18n::workspace::NO_MATCHING_PROJECTS.get())
                .into_any_element()
        } else {
            div()
                .id("project-menu-scroll")
                .max_h(px(280.))
                .overflow_y_scroll()
                .px_1p5()
                .py_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(filtered.into_iter().map(|project| {
                    let id = project.id.clone();
                    let rename_id = id.clone();
                    let remove_id = id.clone();
                    let name = project.name.clone();
                    let rename_name = name.clone();
                    let root = PathBuf::from(&project.folder);
                    let is_active = active_id == Some(project.id.as_str());

                    let action_btn = |btn_id: SharedString, icon: IconName, hover_fg: Rgba| {
                        div()
                            .id(btn_id)
                            .size(px(22.))
                            .rounded(px(4.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(muted)
                            .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                            .child(crate::ui::icon(icon).size(px(12.5)))
                    };

                    div()
                        .id(SharedString::from(format!("project-menu-item-{}", project.id)))
                        .h(px(32.))
                        .px_2()
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .when(is_active, |row| row.bg(active_bg))
                        .hover(move |s| s.bg(hover_bg))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.close_project_menu(cx);
                            app.trash.dismiss();
                            app.switch_project(&id, window, cx);
                        }))
                        .child(
                            div()
                                .w(px(14.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_active, |slot| {
                                    slot.child(
                                        crate::ui::icon(IconName::Check)
                                            .size(px(12.))
                                            .text_color(accent),
                                    )
                                }),
                        )
                        .child(
                            crate::ui::icon(IconName::Folder)
                                .size(px(14.))
                                .flex_shrink_0()
                                .text_color(if is_active { text } else { muted }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .when(is_active, |el| el.font_medium())
                                .text_color(text)
                                .text_ellipsis()
                                .child(name),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_0p5()
                                .flex_shrink_0()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    action_btn(
                                        SharedString::from(format!("pm-reveal-{}", project.id)),
                                        IconName::ExternalLink,
                                        text,
                                    )
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            crate::i18n::workspace::MENU_REVEAL_IN_EXPLORER.get(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        cx.stop_propagation();
                                        app.close_project_menu(cx);
                                        reveal_path(&root, cx);
                                    })),
                                )
                                .child(
                                    action_btn(
                                        SharedString::from(format!("pm-rename-{}", project.id)),
                                        IconName::PencilLine,
                                        text,
                                    )
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            crate::i18n::workspace::MENU_RENAME_PROJECT.get(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        cx.stop_propagation();
                                        app.close_project_menu(cx);
                                        app.start_rename_project(
                                            rename_id.clone(),
                                            &rename_name,
                                            window,
                                            cx,
                                        );
                                    })),
                                )
                                .child(
                                    action_btn(
                                        SharedString::from(format!("pm-remove-{}", project.id)),
                                        IconName::Trash,
                                        danger,
                                    )
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            crate::i18n::workspace::MENU_REMOVE_PROJECT.get(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        cx.stop_propagation();
                                        app.close_project_menu(cx);
                                        app.remove_project(&remove_id, window, cx);
                                    })),
                                ),
                        )
                }))
                .into_any_element()
        };

        Some(
            deferred(
                div()
                    .id("project-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, _, _, cx| {
                            cx.stop_propagation();
                            app.close_project_menu(cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|app, _, _, cx| {
                            cx.stop_propagation();
                            app.close_project_menu(cx);
                        }),
                    )
                    .child(
                        div()
                            .id("project-menu-popover")
                            .absolute()
                            .top(px(crate::ui::TITLEBAR_HEIGHT + 2.))
                            .left(px(6.))
                            .w(px(284.))
                            .max_h(px(420.))
                            .rounded(px(10.))
                            .bg(gpui_kit::rgb(palette.surface))
                            .border_1()
                            .border_color(gpui_kit::rgb(palette.border))
                            .shadow_lg()
                            .occlude()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .px_2p5()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(gpui_kit::rgb(palette.border))
                                    .child(
                                        div()
                                            .h(px(28.))
                                            .px_2()
                                            .rounded(px(6.))
                                            .bg(gpui_kit::rgb(palette.background))
                                            .border_1()
                                            .border_color(gpui_kit::rgb(palette.border))
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(
                                                crate::ui::icon(IconName::Search)
                                                    .size(px(13.))
                                                    .flex_shrink_0()
                                                    .text_color(muted),
                                            )
                                            .child(
                                                Input::new(&self.project_menu.search)
                                                    .appearance(false)
                                                    .bordered(false)
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px_0()
                                                    .py_0()
                                                    .text_size(px(12.5))
                                                    .text_color(text),
                                            ),
                                    ),
                            )
                            .child(list_body)
                            .child(
                                div()
                                    .border_t_1()
                                    .border_color(gpui_kit::rgb(palette.border))
                                    .p_1p5()
                                    .child(
                                        div()
                                            .id("project-menu-add")
                                            .h(px(30.))
                                            .px_2()
                                            .rounded(px(6.))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(muted)
                                            .hover(move |s| s.bg(hover_bg).text_color(text))
                                            .on_click(cx.listener(|app, _, window, cx| {
                                                app.close_project_menu(cx);
                                                app.add_project(&OpenProject, window, cx);
                                            }))
                                            .child(crate::ui::icon(IconName::Plus).size(px(13.)))
                                            .child(
                                                crate::i18n::workspace::ADD_LOCAL_PROJECT.get(),
                                            ),
                                    ),
                            ),
                    ),
            )
            .with_priority(8)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Projects;
    use crate::model::{Engine, Project, ProjectConfig};
    use std::collections::BTreeMap;

    fn sample_project(id: &str) -> Project {
        Project {
            id: id.to_owned(),
            name: format!("Project {id}"),
            folder: format!("/tmp/{id}"),
            main_file: String::new(),
            engine: Engine::default(),
        }
    }

    fn sample_projects(ids: &[&str], recent: &[&str]) -> Projects {
        Projects {
            default_workspace: sample_project("__folio_default"),
            config: ProjectConfig {
                projects: ids.iter().map(|id| sample_project(id)).collect(),
                active_id: ids.first().map(|id| (*id).to_owned()),
                active_workspace: None,
                recent_project_ids: recent.iter().map(|id| (*id).to_owned()).collect(),
            },
            workspace_sessions: BTreeMap::new(),
            active_session: None,
        }
    }

    #[test]
    fn touch_recent_project_moves_to_front_and_prunes_stale_ids() {
        let mut projects = sample_projects(&["a", "b", "c"], &["b", "stale", "a"]);
        projects.touch_recent_project("c");
        assert_eq!(projects.config.recent_project_ids, vec!["c", "b", "a"]);

        projects.touch_recent_project("a");
        assert_eq!(projects.config.recent_project_ids, vec!["a", "c", "b"]);

        projects.touch_recent_project("missing");
        assert_eq!(projects.config.recent_project_ids, vec!["a", "c", "b"]);
    }

    #[test]
    fn recent_projects_backfills_legacy_config_and_respects_limit() {
        let projects = sample_projects(&["a", "b", "c", "d"], &[]);
        let top2: Vec<&str> = projects
            .recent_projects(2)
            .into_iter()
            .map(|project| project.id.as_str())
            .collect();
        assert_eq!(top2, vec!["a", "b"]);

        let partial = sample_projects(&["a", "b", "c", "d"], &["c"]);
        let top3: Vec<&str> = partial
            .recent_projects(3)
            .into_iter()
            .map(|project| project.id.as_str())
            .collect();
        assert_eq!(top3, vec!["c", "a", "b"]);
    }

    #[test]
    fn recent_projects_ignores_removed_ids() {
        let mut projects = sample_projects(&["a", "b", "c"], &["c", "b", "a"]);
        projects.config.projects.retain(|project| project.id != "b");
        projects.config.recent_project_ids.retain(|id| id != "b");
        let ids: Vec<&str> = projects
            .recent_projects(4)
            .into_iter()
            .map(|project| project.id.as_str())
            .collect();
        assert_eq!(ids, vec!["c", "a"]);
    }
}
