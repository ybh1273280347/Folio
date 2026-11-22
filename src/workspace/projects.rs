//! Project selection, workspace restoration and navigation persistence.

use super::*;

impl Workspace {
    pub(super) fn directory_workspace(&self) -> Option<&Project> {
        if self.config.active_workspace == Some(WorkspaceLocation::Default) {
            return Some(&self.default_workspace);
        }
        self.config
            .projects
            .iter()
            .find(|project| Some(&project.id) == self.config.active_id.as_ref())
    }

    pub(super) fn active_project_name(&self) -> Option<String> {
        self.directory_workspace()
            .filter(|project| project.id != "__folio_default")
            .map(|project| project.name.clone())
    }

    pub(super) fn terminal_directory(&self) -> PathBuf {
        self.directory_workspace().map(|project| PathBuf::from(&project.folder))
            .or_else(|| (!self.documents.active.is_empty()).then(|| self.document_path(&self.documents.active)).and_then(|path| path.parent().map(Path::to_path_buf)))
            .unwrap_or_else(|| dirs::home_dir().expect("user home"))
    }

    // Save only navigation state; documents themselves remain authoritative on disk.
    pub(super) fn remember_session(&mut self) -> io::Result<()> {
        let Some(key) = self.active_session.clone() else {
            return Ok(());
        };
        self.workspace_sessions.insert(
            key,
            WorkspaceSession {
                tabs: self.documents.tabs.clone(),
                active: self.documents.active.clone(),
                reading_panes: self.documents.reading_panes.clone(),
                collapsed_folders: Some(self.collapsed_folders.clone()),
            },
        );
        storage::save_workspace_sessions(&self.workspace_sessions)
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
        self.show_trash = false;
        self.tags.reset();
        self.tag_proposals.clear();
        self.documents.replace_watch(watcher);
        self.config.active_id = None;
        self.config.active_workspace = Some(WorkspaceLocation::LooseFiles);
        self.active_session = Some("loose".into());
        self.files.clear();
        self.folders.clear();
        self.collapsed_folders.clear();
        self.focused_folder = None;
        self.selected_files.clear();
        self.build.clear_result();
        self.clear_open_documents(window, cx);

        let session = self
            .workspace_sessions
            .get("loose")
            .cloned()
            .unwrap_or_default();
        for path in &session.tabs {
            if Path::new(path).is_file() {
                self.open_standalone_file(Path::new(path), window, cx)?;
            }
        }
        let active = if self.documents.tabs.contains(&session.active) {
            session.active
        } else {
            self.documents.tabs.last().cloned().unwrap_or_default()
        };
        for path in &session.reading_panes {
            self.open_reading_key(path, window, cx)?;
        }
        self.documents.restore_active_tab(&active);
        self.refresh_tags(cx);
        storage::save_projects(&self.config)?;
        self.remember_session()?;
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
            std::fs::create_dir_all(&self.default_workspace.folder)?;
            self.default_workspace.clone()
        } else {
            self.config.projects.iter().find(|project| project.id == id)
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

        self.show_trash = false;
        self.tags.reset();
        self.tag_proposals.clear();
        self.active_session = Some(id.to_owned());
        if default_space {
            self.config.active_id = None;
            self.config.active_workspace = Some(WorkspaceLocation::Default);
        } else {
            self.config.active_id = Some(id.to_owned());
            self.config.active_workspace = Some(WorkspaceLocation::Project(id.to_owned()));
        }
        self.documents.replace_watch(watcher);
        self.files = files;
        self.folders = folders;
        self.refresh_tags(cx);
        self.build.clear_result();
        self.notice = None;
        self.preview = None;
        self.clear_open_documents(window, cx);
        self.focused_folder = None;
        self.selected_files.clear();
        if previous_pdf.exists() {
            self.build_pdf = Some(self.new_pdf_pane(previous_pdf.clone(), window, cx));
            self.preview = Some(previous_pdf);
        }
        let session = self.workspace_sessions.get(id).cloned().unwrap_or_default();
        self.collapsed_folders = session
            .collapsed_folders
            .clone()
            .unwrap_or_else(|| self.folders.iter().cloned().collect());
        for path in &session.tabs {
            if let Some(file) = self.files.iter().find(|file| &file.path == path).cloned() {
                self.open_file(&file, window, cx)?;
            }
        }
        if self.documents.tabs.is_empty() {
            let first = self
                .files
                .iter()
                .find(|file| file.path == project.main_file)
                .or_else(|| self.files.iter().find(|file| file.kind == FileKind::Text))
                .cloned();
            if let Some(first) = first {
                self.open_file(&first, window, cx)?;
            }
        }
        for path in &session.reading_panes {
            if let Some(file) = self.files.iter().find(|file| &file.path == path).cloned() {
                self.open_reading_pane(&file, window, cx);
            }
        }
        self.documents.restore_active_tab(&session.active);
        self.queue_math_preload(
            self.files
                .iter()
                .filter(|file| file.path.to_lowercase().ends_with(".md"))
                .map(|file| Path::new(&project.folder).join(&file.path))
                .collect(),
            cx,
        );
        storage::save_projects(&self.config)?;
        self.remember_session()?;
        cx.notify();
        Ok(())
    }
    pub(super) fn switch_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let retry_id = id.to_owned();
        if self.build.is_busy()
            || !self.confirm_drafts(window, cx, move |app, window, cx| {
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
        if self.config.active_workspace == Some(WorkspaceLocation::Default) {
            return;
        }
        if self.build.is_busy()
            || !self.confirm_drafts(window, cx, |app, window, cx| {
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
        let previous_id = self.config.active_id.clone();
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
            return;
        }
        match storage::add_project(&mut self.config, folder) {
            Ok(project) => {
                self.config.active_id = previous_id;
                if let Err(error) = self.select_project(&project.id, window, cx) {
                    self.report(error, cx);
                }
            }
            Err(error) => self.report(error, cx),
        }
    }
    pub(super) fn add_project(&mut self, _: &OpenProject, window: &mut Window, cx: &mut Context<Self>) {
        if self.build.is_busy()
            || !self.confirm_drafts(window, cx, |app, window, cx| {
                app.add_project(&OpenProject, window, cx)
            })
        {
            return;
        }
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(crate::i18n::workspace::DIALOG_SELECT_PROJECT_FOLDER.get());
        self.dialog_open = true;
        cx.spawn_in(window, async move |this, cx| {
            let folder = dialog.pick_folder().await;
            let _ = this.update_in(cx, |app, window, cx| {
                app.dialog_open = false;
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
        self.dialog_open = true;
        cx.spawn_in(window, async move |this, cx| {
            let selected = dialog.pick_file().await;
            let _ = this.update_in(cx, |app, window, cx| {
                app.dialog_open = false;
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
        let project_root = self
            .directory_workspace()
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
            } else if path.starts_with(&self.default_workspace.folder) {
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
            if !self.confirm_drafts(window, cx, move |app, window, cx| app.open_paths(retry_paths.clone(), window, cx)) {
                return;
            }
            if let Err(error) = self.select_project("__folio_default", window, cx) {
                self.report(error, cx);
                return;
            }
            for path in default_space_files {
                let relative = path.strip_prefix(&self.default_workspace.folder).unwrap().to_string_lossy().replace('\\', "/");
                let file = ProjectFile { kind: Self::file_kind(&path), path: relative };
                if let Err(error) = self.open_file(&file, window, cx) { self.report(error, cx); return; }
            }
        }
        if default_files.is_empty() {
            return;
        }
        if self.directory_workspace().is_some() {
            let retry_paths = default_files.clone();
            if !self.confirm_drafts(window, cx, move |app, window, cx| {
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
        let removing = self
            .dialog_response
            .as_ref()
            .is_some_and(|(kind, _)| *kind == Dialog::RemoveProject);
        if self.build.is_busy()
            || (!removing
                && !self.confirm_drafts(window, cx, move |app, window, cx| {
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
            window,
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
        self.config.projects.retain(|project| project.id != id);
        self.workspace_sessions.remove(id);
        if self.config.active_id.as_deref() == Some(id) {
            self.config.active_id = None;
            self.active_session = None;
            if let Some(project) = self.config.projects.first() {
                let id = project.id.clone();
                if let Err(error) = self.select_project(&id, window, cx) {
                    self.report(error, cx);
                }
            } else if let Err(error) = self.select_loose_files(window, cx) {
                self.report(error, cx);
            }
        }
        if let Err(error) = storage::save_projects(&self.config) {
            self.report(error, cx);
        }
        cx.notify();
    }
}
