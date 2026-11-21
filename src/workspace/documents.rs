//! Live document buffers and their editor/reader projections.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PaneRole {
    Tab,
    Reading,
}

/// A pane's role is independent of the path persisted in workspace sessions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PaneKey {
    pub(super) path: String,
    pub(super) role: PaneRole,
}

impl PaneKey {
    pub(super) fn new(path: &str, role: PaneRole) -> Self {
        Self { path: path.to_owned(), role }
    }

    pub(super) fn tab(path: &str) -> Self {
        Self::new(path, PaneRole::Tab)
    }

    pub(super) fn reading(path: &str) -> Self {
        Self::new(path, PaneRole::Reading)
    }
}

pub(super) struct OpenDocument {
    pub(super) path: PathBuf,
    pub(super) buffer: DocumentBuffer,
    pub(super) editor: Entity<EditorState>,
    pub(super) last_edit: Instant,
    pub(super) subscription: Subscription,
}

/// The open buffer is shared by all panes for the same document. Navigation
/// stores paths only, so workspace-session files remain independent of GPUI.
#[derive(Default)]
pub(super) struct Documents {
    pub(super) open: BTreeMap<String, OpenDocument>,
    pub(super) tabs: Vec<String>,
    pub(super) active: String,
    pub(super) reading_panes: Vec<String>,
    pub(super) focused_markdown: Option<String>,
    pub(super) markdown_panes: BTreeMap<PaneKey, Entity<MarkdownPane>>,
    pub(super) pdf_panes: BTreeMap<PaneKey, Entity<PdfPane>>,
    pub(super) html_panes: BTreeMap<PaneKey, Entity<HtmlPane>>,
    pub(super) drawio_panes: BTreeMap<PaneKey, Entity<DrawioPane>>,
    pub(super) image_panes: BTreeMap<PaneKey, Entity<crate::image::ImagePane>>,
    pub(super) markdown_changed: BTreeSet<String>,
    pub(super) source_version: u64,
    pub(super) document_watch: DocumentWatch,
}

/// One watcher and its pending changes belong to the same workspace selection.
#[derive(Default)]
pub(super) struct DocumentWatch {
    transport: Option<(RecommendedWatcher, mpsc::Receiver<notify::Result<notify::Event>>)>,
    parents: BTreeSet<PathBuf>,
    changed_at: Option<Instant>,
    paths: BTreeSet<PathBuf>,
    tree: bool,
}

impl DocumentWatch {
    pub(super) fn new(root: Option<&Path>) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        }).map_err(io::Error::other)?;
        if let Some(root) = root {
            watcher.watch(root, RecursiveMode::Recursive).map_err(io::Error::other)?;
        }
        Ok(Self { transport: Some((watcher, receiver)), ..Self::default() })
    }

    pub(super) fn invalidate_tree(&mut self) {
        self.paths.clear();
        self.tree = true;
    }

    pub(super) fn refresh_now(&mut self) {
        self.invalidate_tree();
        self.changed_at = None;
    }
}

impl Workspace {
    /// Both editable Markdown projections and the plain editor submit the same draft.
    pub(super) fn edit_document(&mut self, path: &str, text: String) {
        let document = self.documents.open.get_mut(path).expect("editor owns an open document");
        if document.buffer.text != text {
            document.buffer.set_text(text);
            document.last_edit = Instant::now();
            self.documents.markdown_changed.insert(path.to_owned());
            self.documents.source_version += 1;
        }
    }
}

impl Workspace {
    pub(super) fn file_kind(path: &Path) -> FileKind {
        if files::is_drawio(path) {
            return FileKind::Asset;
        }
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("pdf" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "html" | "htm" | "drawio") => {
                FileKind::Asset
            }
            _ => FileKind::Text,
        }
    }

    pub(super) fn is_drawio_path(path: &str) -> bool {
        let lower = path.to_lowercase();
        lower.ends_with(".drawio") || lower.ends_with(".drawio.svg")
    }

    pub(super) fn is_html_path(path: &str) -> bool {
        let lower = path.to_lowercase();
        lower.ends_with(".html") || lower.ends_with(".htm")
    }

    pub(super) fn is_image_path(path: &str) -> bool {
        if Self::is_drawio_path(path) {
            return false;
        }
        matches!(
            Path::new(path)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "svg")
        )
    }

    fn html_root_dir(&self, full: &Path) -> PathBuf {
        self.directory_workspace()
            .map(|project| PathBuf::from(&project.folder))
            .unwrap_or_else(|| {
                full.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."))
            })
    }

    fn add_html_pane(
        &mut self,
        key: PaneKey,
        file_path: PathBuf,
        root_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_reading = key.role == PaneRole::Reading;
        let pane = cx.new(|cx| HtmlPane::new(file_path, root_dir, is_reading, cx));
        let close_key = key.path.clone();
        cx.subscribe_in(
            &pane,
            window,
            move |app, _, event: &HtmlPaneEvent, window, cx| match event {
                HtmlPaneEvent::Close => {
                    if is_reading {
                        app.close_reading_pane(&close_key, window, cx);
                    } else {
                        app.close_tab(&close_key, window, cx);
                    }
                }
                HtmlPaneEvent::OpenExternal(url) => {
                    cx.open_url(url);
                }
            },
        )
        .detach();
        self.documents.html_panes.insert(key, pane);
    }

    pub(super) fn release_html_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.documents.html_panes.remove(key) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn release_all_html_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, pane) in std::mem::take(&mut self.documents.html_panes) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    fn add_drawio_pane(
        &mut self,
        key: PaneKey,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_reading = key.role == PaneRole::Reading;
        let pane = cx.new(|cx| DrawioPane::new(file_path, is_reading, cx));
        let close_key = key.path.clone();
        cx.subscribe_in(
            &pane,
            window,
            move |app, _, event: &DrawioPaneEvent, window, cx| match event {
                DrawioPaneEvent::Close => {
                    if is_reading {
                        app.close_reading_pane(&close_key, window, cx);
                    } else {
                        app.close_tab(&close_key, window, cx);
                    }
                }
                DrawioPaneEvent::OpenExternal(url) => cx.open_url(url),
                DrawioPaneEvent::Notice(message) => app.report(message.clone(), cx),
                DrawioPaneEvent::Error(message) => app.report(message.clone(), cx),
                DrawioPaneEvent::ToolResult { request_id, result } => {
                    app.drawio_tool_result(request_id, result.clone(), cx)
                }
            },
        )
        .detach();
        self.documents.drawio_panes.insert(key, pane);
    }

    pub(super) fn release_drawio_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.documents.drawio_panes.remove(key) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn release_all_drawio_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, pane) in std::mem::take(&mut self.documents.drawio_panes) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn release_pdf_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.documents.pdf_panes.remove(key) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn release_all_pdf_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, pane) in std::mem::take(&mut self.documents.pdf_panes) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn new_pdf_pane(
        &self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PdfPane> {
        let pane = cx.new(|cx| PdfPane::new(path, window, cx));
        cx.subscribe_in(&pane, window, |app, _, event: &PdfPaneEvent, window, cx| {
            let PdfPaneEvent::AskAi { path, text } = event;
            let source = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "PDF".to_owned());
            app.open_ai_with_selection(source, Some(path.clone()), text.clone(), window, cx);
        })
        .detach();
        pane
    }

    pub(super) fn document_path(&self, key: &str) -> PathBuf {
        self.directory_workspace()
            .map(|project| PathBuf::from(&project.folder).join(key))
            .unwrap_or_else(|| PathBuf::from(key))
    }

    pub(super) fn clear_open_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_build_pdf(window, cx);
        self.release_all_pdf_panes(window, cx);
        self.release_all_html_panes(window, cx);
        self.release_all_drawio_panes(window, cx);
        self.documents.image_panes.clear();
        self.documents.open.clear();
        self.documents.tabs.clear();
        self.documents.active.clear();
        self.documents.reading_panes.clear();
        self.documents.markdown_panes.clear();
        self.documents.focused_markdown = None;
        self.documents.markdown_changed.clear();
        self.show_build_preview = false;
        self.preview = None;
    }

    fn prepare_standalone_file(&mut self, path: &Path) -> io::Result<ProjectFile> {
        let full = dunce::canonicalize(path)?;
        if !full.is_file() || !files::is_relevant(&full) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                crate::i18n::workspace::ERR_SELECT_SUPPORTED_LOCAL_FILE.get(),
            ));
        }
        self.watch_standalone_parent(full.parent().expect("canonical file has a parent"))?;
        let key = full.to_string_lossy().into_owned();
        Ok(ProjectFile {
            kind: Self::file_kind(&full),
            path: key,
        })
    }

    pub(super) fn watch_standalone_parent(&mut self, parent: &Path) -> io::Result<()> {
        if self.documents.document_watch.transport.is_none() {
            self.documents.document_watch = DocumentWatch::new(None)?;
        }
        if !self.documents.document_watch.parents.contains(parent) {
            self.documents.document_watch.transport
                .as_mut()
                .expect("independent files have a watcher")
                .0
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(io::Error::other)?;
            self.documents.document_watch.parents.insert(parent.to_owned());
        }
        Ok(())
    }

    pub(super) fn open_standalone_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let file = self.prepare_standalone_file(path)?;
        self.open_file(&file, window, cx)
    }

    pub(super) fn supports_split_pane(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        lower.ends_with(".md")
            || lower.ends_with(".pdf")
            || Self::is_html_path(&lower)
            || Self::is_drawio_path(&lower)
    }

    fn open_internal_link_file(
        &mut self,
        file: &ProjectFile,
        anchor: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.settings.internal_link_open_target == InternalLinkOpenTarget::SplitPane
            && Self::supports_split_pane(&file.path)
        {
            self.open_reading_pane(file, window, cx);
            let reading_key = PaneKey::reading(&file.path);
            if let Some(pane) = self.documents.markdown_panes.get(&reading_key).cloned() {
                pane.update(cx, |pane, cx| pane.focus(window, cx));
            }
            self.jump_markdown_anchor(&reading_key, anchor, window, cx);
            return Ok(());
        }
        self.open_file(file, window, cx)?;
        self.jump_markdown_anchor(&PaneKey::tab(&file.path), anchor, window, cx);
        Ok(())
    }

    fn open_markdown_link(
        &mut self,
        source_file: &Path,
        raw_url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_root = self
            .directory_workspace()
            .map(|project| PathBuf::from(&project.folder));
        let target = match classify_markdown_link(source_file, workspace_root.as_deref(), raw_url) {
            MarkdownLinkTarget::External(url) => {
                cx.open_url(&url);
                return;
            }
            MarkdownLinkTarget::Local { path, anchor } => (path, anchor),
            MarkdownLinkTarget::Invalid(message) => {
                self.report(message, cx);
                return;
            }
        };
        let (candidate, anchor) = target;
        let Ok(resolved) = dunce::canonicalize(&candidate) else {
            self.report(
                crate::i18n::workspace::err_link_target_not_found(&candidate),
                cx,
            );
            return;
        };
        if resolved.is_dir() {
            match url::Url::from_directory_path(&resolved) {
                Ok(dir_url) => cx.open_url(dir_url.as_str()),
                Err(()) => self.report(crate::i18n::workspace::err_open_dir(&resolved), cx),
            }
            return;
        }
        if let Some(root) = workspace_root
            && let Ok(canonical_root) = dunce::canonicalize(&root)
            && let Ok(relative) = resolved.strip_prefix(&canonical_root)
            && files::is_relevant(&resolved)
        {
            let key = relative.to_string_lossy().replace('\\', "/");
            let file = ProjectFile {
                kind: Self::file_kind(&resolved),
                path: key,
            };
            if let Err(error) =
                self.open_internal_link_file(&file, anchor.as_deref(), window, cx)
            {
                self.report(error, cx);
            }
            return;
        }
        if self.directory_workspace().is_none() && files::is_relevant(&resolved) {
            match self.prepare_standalone_file(&resolved).and_then(|file| {
                self.open_internal_link_file(&file, anchor.as_deref(), window, cx)
            }) {
                Ok(()) => self.refresh_tags(cx),
                Err(error) => self.report(error, cx),
            }
            return;
        }
        match url::Url::from_file_path(&resolved) {
            Ok(file_url) => cx.open_url(file_url.as_str()),
            Err(()) => self.report(crate::i18n::workspace::err_open_file(&resolved), cx),
        }
    }

    fn jump_markdown_anchor(
        &mut self,
        key: &PaneKey,
        anchor: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = anchor.filter(|a| !a.is_empty()) else {
            return;
        };
        if !key.path.to_ascii_lowercase().ends_with(".md") {
            return;
        }
        if let Some(pane) = self.documents.markdown_panes.get(key).cloned() {
            pane.update(cx, |pane, cx| pane.navigate_to_anchor(anchor, window, cx));
        }
    }

    pub(super) fn open_reading_key(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let full = self.document_path(path);
        let file = ProjectFile {
            path: path.to_owned(),
            kind: Self::file_kind(&full),
        };
        self.open_reading_pane(&file, window, cx);
        Ok(())
    }
    fn subscribe_editor(
        &mut self,
        path: String,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe(editor, move |app, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Focus) {
                app.documents.focused_markdown = None;
            }
            if matches!(event, InputEvent::Change) {
                app.edit_document(&path, state.read(cx).value().to_string());
                cx.notify();
            }
        })
    }
    pub(super) fn open_file(
        &mut self,
        file: &ProjectFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.open_file_with_focus(file, true, window, cx)
    }

    pub(super) fn open_file_with_focus(
        &mut self,
        file: &ProjectFile,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.documents.focused_markdown = None;
        let tab = PaneKey::tab(&file.path);
        self.mount_document_pane(&tab, file.kind, window, cx)?;
        if !self.documents.tabs.contains(&file.path) {
            self.documents.tabs.push(file.path.clone());
        }
        self.documents.active = file.path.clone();
        if focus_editor {
            if let Some(pane) = self.documents.markdown_panes.get(&tab) {
                pane.update(cx, |pane, cx| pane.focus(window, cx));
            } else if let Some(document) = self.documents.open.get(&file.path) {
                document.editor.update(cx, |editor, cx| editor.focus(window, cx));
            }
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
        Ok(())
    }

    fn mount_document_pane(
        &mut self,
        key: &PaneKey,
        kind: FileKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let path = &key.path;
        let full = self.document_path(path);
        if path.to_ascii_lowercase().ends_with(".pdf") && !self.documents.pdf_panes.contains_key(key) {
            self.documents.pdf_panes.insert(
                key.clone(),
                self.new_pdf_pane(full.clone(), window, cx),
            );
        }
        if Self::is_html_path(path) && !self.documents.html_panes.contains_key(key) {
            let root_dir = self.html_root_dir(&full);
            self.add_html_pane(key.clone(), full.clone(), root_dir, window, cx);
        }
        if Self::is_drawio_path(path) && !self.documents.drawio_panes.contains_key(key) {
            self.add_drawio_pane(key.clone(), full.clone(), window, cx);
        }
        if Self::is_image_path(path) && !self.documents.image_panes.contains_key(key) {
            self.documents.image_panes.insert(
                key.clone(),
                cx.new(|cx| crate::image::ImagePane::new(full.clone(), cx)),
            );
        }
        if kind == FileKind::Text && !self.documents.open.contains_key(path) {
            let disk = if self.directory_workspace().is_some() {
                files::read_document(
                    Path::new(&self.directory_workspace().expect("project was checked").folder),
                    path,
                )?
            } else {
                files::read_document_at(&full)?
            };
            let extension = Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase);
            let language = match extension.as_deref() {
                Some("tex" | "ltx" | "sty" | "cls" | "bib") => "latex",
                Some("md") => "markdown",
                Some("json") => "json",
                Some("yaml" | "yml") => "yaml",
                Some("toml") => "toml",
                _ => "plaintext",
            };
            let editor = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(language)
                    .line_number(self.settings.show_line_numbers)
                    .searchable(true)
                    .default_value(if path.to_ascii_lowercase().ends_with(".md") {
                        String::new()
                    } else {
                        disk.text.clone()
                    })
            });
            let subscription = self.subscribe_editor(path.clone(), &editor, cx);
            self.documents.open.insert(
                path.clone(),
                OpenDocument {
                    path: full.clone(),
                    buffer: DocumentBuffer::from_disk(disk),
                    editor,
                    last_edit: Instant::now(),
                    subscription,
                },
            );
        }
        if path.to_ascii_lowercase().ends_with(".md")
            && !self.documents.markdown_panes.contains_key(key)
        {
            self.add_markdown_pane(path, key.role == PaneRole::Reading, window, cx);
        }
        Ok(())
    }

    fn add_markdown_pane(
        &mut self,
        path: &str,
        reading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.documents.open[path].buffer.text.clone();
        let full = self.documents.open[path].path.clone();
        let settings = self.settings.clone();
        let math = self.math.clone();
        let pane = cx.new(|cx| MarkdownPane::new(text, full, settings, math, reading, window, cx));
        if self.has_visible_background() {
            pane.update(cx, |pane, cx| pane.set_backdrop_visible(true, cx));
        }
        if self.zen_mode {
            pane.update(cx, |pane, cx| pane.set_zen_mode(true, cx));
        }
        cx.subscribe_in(&pane, window, move |app, pane, event: &MarkdownEvent, window, cx| {
            // Resolve identity from the live pane; a rename keeps this subscription alive.
            let full = pane.read(cx).path().to_path_buf();
            let file = app.documents.open.iter().find(|(_, doc)| doc.path == full)
                .expect("Markdown pane owns open document").0.clone();
            match event {
                MarkdownEvent::RenameTitle(title) => {
                    let name = format!("{title}.md");
                    if title.is_empty() {
                        app.report(crate::i18n::workspace::ERR_EMPTY_TITLE.get(), cx);
                    } else if let Err(error) = app.rename_document(&file, &name, window, cx) {
                        app.report(error, cx);
                    } else {
                        pane.update(cx, |pane, cx| pane.focus(window, cx));
                    }
                }
                MarkdownEvent::Change(text) => {
                    app.edit_document(&file, text.clone());
                }
                MarkdownEvent::Focus => app.documents.focused_markdown = reading.then(|| file.clone()),
                MarkdownEvent::Zoom(size) => {
                    let mut settings = app.settings.clone();
                    settings.markdown_font_size = *size;
                    app.apply_settings(settings, window, cx);
                }
                MarkdownEvent::OpenLink(url) => {
                    app.open_markdown_link(&full, url, window, cx);
                }
                MarkdownEvent::AskAi(text) => {
                    let source = full
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| file.clone());
                    app.open_ai_with_selection(source, Some(full.clone()), text.clone(), window, cx);
                }
                MarkdownEvent::FilesChanged => {
                    if let Err(error) = app.refresh_files(window, cx) {
                        app.report(error, cx);
                    }
                }
            }
            cx.notify();
        })
        .detach();
        self.documents.markdown_panes.insert(
            PaneKey::new(path, if reading { PaneRole::Reading } else { PaneRole::Tab }),
            pane,
        );
    }
    pub(super) fn save_file(
        &mut self,
        path: &str,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) -> io::Result<bool> {
        let root = self.directory_workspace().map(|project| PathBuf::from(&project.folder));
        let document = self.documents.open
            .get_mut(path)
            .expect("save only opened text files");
        if document.buffer.deleted {
            return Err(io::Error::other(
                crate::i18n::workspace::ERR_FILE_DELETED_RESTORE_FIRST.get(),
            ));
        }
        if document.buffer.conflict.is_some() && !overwrite {
            return Ok(false);
        }
        let revision = if overwrite {
            &document
                .buffer
                .conflict
                .as_ref()
                .expect("overwrite needs conflict")
                .revision
        } else {
            &document.buffer.revision
        };
        let result = if let Some(root) = root {
            files::save_document(&root, path, &document.buffer.text, revision)?
        } else {
            files::save_document_at(&document.path, &document.buffer.text, revision)?
        };
        match result {
            Ok(saved) => {
                document.buffer.saved_text = saved.text;
                document.buffer.dirty = false;
                document.buffer.revision = saved.revision;
                document.buffer.conflict = None;
                cx.notify();
                Ok(true)
            }
            Err(disk) => {
                document.buffer.conflict = Some(disk);
                self.documents.active = path.to_owned();
                cx.notify();
                Ok(false)
            }
        }
    }
    pub(super) fn save_active(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let path = self.documents.focused_markdown
            .clone()
            .unwrap_or_else(|| self.documents.active.clone());
        if self.documents.open.contains_key(&path) {
            if let Err(error) = self.save_file(&path, false, cx) {
                self.report(error, cx);
            }
        } else if let Some(pane) = self.documents.drawio_panes.get(&PaneKey::tab(&path)).cloned() {
            pane.update(cx, |pane, cx| pane.save(cx));
        }
    }
    pub(super) fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        let paths: Vec<_> = self.documents.open
            .iter()
            .filter(|(_, document)| {
                document.buffer.is_dirty() || document.buffer.conflict.is_some()
            })
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            match self.save_file(&path, false, cx) {
                Ok(true) => {}
                Ok(false) => return false,
                Err(error) => {
                    self.report(error, cx);
                    return false;
                }
            }
        }
        true
    }
    /// Store the retry action while the in-window prompt is open; selecting a
    /// button re-enters the same operation with its original response.

    pub(super) fn activate_tab(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.documents.active == path {
            return;
        }
        self.documents.focused_markdown = None;
        self.documents.active = path.to_owned();
        if let Some(pane) = self.documents.markdown_panes.get(&PaneKey::tab(path)) {
            pane.update(cx, |pane, cx| pane.focus(window, cx));
        } else if let Some(document) = self.documents.open.get(path) {
            document
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.documents.reading_panes.iter().any(|pane| pane == path)
            && (self.documents.open
                .get(path)
                .is_some_and(|document| document.buffer.is_dirty())
                || self
                    .dialog_response
                    .as_ref()
                    .is_some_and(|(kind, _)| *kind == Dialog::CloseTab))
        {
            let retry_path = path.to_owned();
            match self.ask(
                Dialog::CloseTab,
                crate::i18n::workspace::dialog_close_unsaved_title(path),
                crate::i18n::workspace::DIALOG_UNSAVED_FILE_BODY.get().into(),
                DialogButtons::SaveDiscardCancel,
                window,
                cx,
                move |app, window, cx| {
                    if app.documents.reading_panes.contains(&retry_path) && !app.documents.tabs.contains(&retry_path) {
                        app.close_reading_pane(&retry_path, window, cx);
                    } else {
                        app.close_tab(&retry_path, window, cx);
                    }
                },
            ) {
                Some(rfd::MessageDialogResult::Yes) => match self.save_file(path, false, cx) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        self.report(error, cx);
                        return;
                    }
                },
                Some(rfd::MessageDialogResult::No) => {}
                _ => return,
            }
        }
        self.documents.tabs.retain(|tab| tab != path);
        let tab = PaneKey::tab(path);
        self.release_pdf_pane(&tab, window, cx);
        self.release_html_pane(&tab, window, cx);
        self.release_drawio_pane(&tab, window, cx);
        self.documents.image_panes.remove(&tab);
        self.documents.markdown_panes.remove(&tab);
        if !self.documents.reading_panes.iter().any(|pane| pane == path) {
            self.documents.open.remove(path);
        }
        if self.documents.focused_markdown.as_deref() == Some(path) {
            self.documents.focused_markdown = None;
        }
        if self.documents.active == path {
            self.documents.active = self.documents.tabs.last().cloned().unwrap_or_default();
            if let Some(document) = self.documents.open.get(&self.documents.active) {
                document
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
            }
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }
    pub(super) fn reload_disk(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let disk = if let Some(project) = self.directory_workspace() {
            files::read_document(Path::new(&project.folder), path)
        } else {
            files::read_document_at(&self.documents.open[path].path)
        };
        match disk {
            Ok(disk) => {
                let document = self.documents.open
                    .get_mut(path)
                    .expect("reload only open documents");
                document.editor.update(cx, |editor, cx| {
                    editor.set_value(disk.text.clone(), window, cx)
                });
                document.buffer.replace_from_disk(disk);
                self.documents.markdown_changed.insert(path.to_owned());
                cx.notify();
            }
            Err(error) => self.report(error, cx),
        }
    }
    pub(super) fn queue_math_preload(&self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let task = self
            .math
            .read(cx)
            .prewarm_paths(paths, self.math.clone(), cx);
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                this.update(cx, |app, cx| app.report(error, cx))
                    .expect("workspace is alive while preloading");
            }
        })
        .detach();
    }
    pub(super) fn refresh_files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let root = self.directory_workspace().map(|project| PathBuf::from(&project.folder));
        let previous: BTreeSet<_> = self.files.iter().map(|file| file.path.clone()).collect();
        let refresh_tree = self.documents.document_watch.paths.is_empty() || self.documents.document_watch.tree;
        if let Some(root) = &root {
            if refresh_tree {
                self.files = files::list_files(root)?;
                self.folders = files::list_folders(root).unwrap_or_default();
                if self
                    .focused_folder
                    .as_ref()
                    .is_some_and(|folder| !self.folders.contains(folder))
                {
                    self.focused_folder = None;
                }
            }
        }
        if refresh_tree || self.documents.document_watch.paths.iter().any(|path| {
            let key = root.as_ref().and_then(|root| path.strip_prefix(root).ok())
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            self.tags.snapshot.assigned.contains_key(&key)
        }) {
            self.refresh_tags(cx);
        }
        let changed = std::mem::take(&mut self.documents.document_watch.paths);
        self.queue_math_preload(
            self.files
                .iter()
                .filter(|file| {
                    file.path.to_lowercase().ends_with(".md")
                        && (!previous.contains(&file.path)
                            || root
                                .as_ref()
                                .is_some_and(|root| changed.contains(&root.join(&file.path))))
                })
                .filter_map(|file| root.as_ref().map(|root| root.join(&file.path)))
                .collect(),
            cx,
        );
        self.documents.document_watch.tree = false;
        for (path, document) in &mut self.documents.open {
            // notify gives absolute paths; read only drafts touched by this event.
            if !changed.is_empty() && !changed.contains(&document.path) {
                continue;
            }
            let disk = if let Some(root) = &root {
                files::read_document(root, path)
            } else {
                files::read_document_at(&document.path)
            };
            match disk {
                Ok(disk) => {
                    document.buffer.deleted = false;
                    if disk.revision == document.buffer.revision {
                        continue;
                    }
                    if document.buffer.is_dirty() {
                        document.buffer.conflict = Some(disk);
                    } else {
                        if !path.to_lowercase().ends_with(".md") {
                            document.editor.update(cx, |editor, cx| {
                                editor.set_value(disk.text.clone(), window, cx)
                            });
                        }
                        document.buffer.replace_from_disk(disk);
                        self.documents.markdown_changed.insert(path.clone());
                        self.documents.source_version += 1;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    document.buffer.deleted = true
                }
                Err(error) => return Err(error),
            }
        }
        let pdf_keys: Vec<_> = self.documents.pdf_panes
            .keys()
            .filter(|key| {
                let file = &key.path;
                let path = root
                    .as_ref()
                    .map(|root| root.join(file))
                    .unwrap_or_else(|| PathBuf::from(file));
                changed.contains(&path)
            })
            .cloned()
            .collect();
        for key in pdf_keys {
            let file = &key.path;
            let full = root
                .as_ref()
                .map(|root| root.join(file))
                .unwrap_or_else(|| PathBuf::from(file));
            self.release_pdf_pane(&key, window, cx);
            self.documents.pdf_panes
                .insert(key, self.new_pdf_pane(full, window, cx));
        }
        if !changed.is_empty() {
            for pane in self.documents.html_panes.values() {
                let file_path = pane.read(cx).file_path().to_path_buf();
                if changed.contains(&file_path) {
                    pane.update(cx, |pane, cx| pane.reload(window, cx));
                }
            }
            for pane in self.documents.drawio_panes.values() {
                let file_path = pane.read(cx).file_path().to_path_buf();
                if changed.contains(&file_path) {
                    pane.update(cx, |pane, cx| pane.reload_if_external(cx));
                }
            }
            for pane in self.documents.image_panes.values() {
                let file_path = pane.read(cx).file_path().to_path_buf();
                if changed.contains(&file_path) {
                    pane.update(cx, |pane, cx| pane.reload(cx));
                }
            }
        }
        cx.notify();
        Ok(())
    }

    pub(super) fn poll_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for path in std::mem::take(&mut self.documents.markdown_changed) {
            if let Some(document) = self.documents.open.get(&path) {
                for role in [PaneRole::Tab, PaneRole::Reading] {
                    if let Some(pane) = self.documents.markdown_panes.get(&PaneKey::new(&path, role)) {
                        pane.update(cx, |pane, cx| {
                            pane.sync(&document.buffer.text, window, cx)
                        });
                    }
                }
            }
        }
        let events: Vec<_> = self.documents.document_watch.transport
            .iter()
            .flat_map(|(_, receiver)| receiver.try_iter())
            .collect();
        for event in events {
            match event {
                Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                    let directory_event = matches!(
                        event.kind,
                        notify::EventKind::Create(notify::event::CreateKind::Folder)
                            | notify::EventKind::Remove(notify::event::RemoveKind::Folder)
                    );
                    let tree = matches!(
                        event.kind,
                        notify::EventKind::Create(_)
                            | notify::EventKind::Remove(_)
                            | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                    );
                    let root = self.directory_workspace().map(|project| PathBuf::from(&project.folder));
                    let paths: Vec<_> = event
                        .paths
                        .into_iter()
                        .filter(|path| {
                            if let Some(root) = &root {
                                path.strip_prefix(root).is_ok_and(|relative| {
                                    !relative.components().any(|part| {
                                        files::EXCLUDED_DIRECTORIES
                                            .iter()
                                            .any(|name| part.as_os_str() == *name)
                                    })
                                    && (directory_event || path.is_dir() || files::is_relevant(path)
                                        || (tree && self.folders.iter().any(|folder| root.join(folder) == *path)))
                                })
                            } else {
                                files::is_relevant(path)
                                && ((tree && path.parent().is_some_and(|parent| self.documents.document_watch.parents.contains(parent))) || self.documents.open
                                        .values()
                                        .any(|document| document.path == *path)
                                        || self.documents.pdf_panes.keys().any(|key| {
                                            Path::new(&key.path) == path
                                        })
                                        || self.documents.html_panes.values().any(|pane| {
                                            pane.read(cx).file_path() == *path
                                        })
                                        || self.documents.drawio_panes.values().any(|pane| {
                                            pane.read(cx).file_path() == *path
                                        })
                                        || self.documents.image_panes.values().any(|pane| {
                                            pane.read(cx).file_path() == *path
                                        }))
                            }
                        })
                        .collect();
                    if !paths.is_empty() {
                        self.documents.document_watch.paths.extend(paths);
                        self.documents.document_watch.tree |= tree;
                        self.documents.document_watch.changed_at = Some(Instant::now());
                    }
                }
                Ok(_) => {}
                Err(error) => self.report(error, cx),
            }
        }
        if self.documents.document_watch.changed_at
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(200))
        {
            self.documents.document_watch.changed_at = None;
            if let Err(error) = self.refresh_files(window, cx) {
                self.report(error, cx);
            }
        }
        // Autosave only idle, conflict-free drafts. Manual saves and builds share this revision path.
        if self.name_action.is_none() && !self.dialog_open {
            let paths: Vec<_> = self.documents.open
                .iter()
                .filter(|(_, document)| {
                    document.buffer.is_dirty()
                        && !document.buffer.deleted
                        && document.buffer.conflict.is_none()
                        && document.last_edit.elapsed() >= Duration::from_millis(800)
                })
                .map(|(path, _)| path.clone())
                .collect();
            for path in paths {
                if let Err(error) = self.save_file(&path, false, cx) {
                    self.documents.open
                        .get_mut(&path)
                        .expect("autosave document exists")
                        .last_edit = Instant::now();
                    self.report(error, cx);
                }
            }
        }
    }

    pub(super) fn rename_document(&mut self, key: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let old = self.document_path(key);
        if old.file_name().unwrap() == name { return Ok(()); }
        let parent = old.parent().unwrap();
        files::rename_file(parent, &old.file_name().unwrap().to_string_lossy(), name)?;
        let full = parent.join(name);
        let renamed = if let Some(workspace) = self.directory_workspace() {
            full.strip_prefix(&workspace.folder).expect("same workspace").to_string_lossy().replace('\\', "/")
        } else { full.to_string_lossy().into_owned() };
        self.relocate_file_state(key, &renamed, window, cx);
        let annotation_result = crate::annotations::relocate_annotations(&old, &full);
        self.relocate_tags(&[(key.to_owned(), renamed.clone())], false, cx);
        if let Some(workspace) = self.directory_workspace() {
            self.files = files::list_files(Path::new(&workspace.folder))?;
            storage::save_projects(&self.config)?;
        }
        self.remember_session()?;
        cx.notify();
        let kind = Self::file_kind(&full);
        if self.documents.tabs.contains(&renamed) {
            self.mount_document_pane(&PaneKey::tab(&renamed), kind, window, cx)?;
        }
        if self.documents.reading_panes.contains(&renamed) && Self::supports_split_pane(&renamed) {
            self.mount_document_pane(&PaneKey::reading(&renamed), kind, window, cx)?;
        }
        annotation_result
    }

    // Keep live buffers and Markdown panes: moving a file must not reload unsaved edits.
    pub(super) fn relocate_file_state(&mut self, key: &str, renamed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let full = self.document_path(renamed);
        let renamed = renamed.to_owned();
        if let Some(mut document) = self.documents.open.remove(key) {
            document.path = full.clone();
            document.subscription = self.subscribe_editor(renamed.clone(), &document.editor, cx);
            self.documents.open.insert(renamed.clone(), document);
        }
        for role in [PaneRole::Tab, PaneRole::Reading] {
            let old_key = PaneKey::new(key, role);
            let new_key = PaneKey::new(&renamed, role);
            if let Some(pane) = self.documents.markdown_panes.remove(&old_key)
                && renamed.to_ascii_lowercase().ends_with(".md")
            {
                pane.update(cx, |pane, cx| pane.set_path(full.clone(), window, cx));
                self.documents.markdown_panes.insert(new_key.clone(), pane);
            }
            if self.documents.pdf_panes.contains_key(&old_key) {
                self.release_pdf_pane(&old_key, window, cx);
                if renamed.to_ascii_lowercase().ends_with(".pdf") {
                    self.documents.pdf_panes.insert(new_key.clone(), self.new_pdf_pane(full.clone(), window, cx));
                }
            }
            if self.documents.html_panes.contains_key(&old_key) {
                self.release_html_pane(&old_key, window, cx);
                if Self::is_html_path(&renamed) {
                    let root = self.html_root_dir(&full);
                    self.add_html_pane(new_key.clone(), full.clone(), root, window, cx);
                }
            }
            if self.documents.drawio_panes.contains_key(&old_key) {
                self.release_drawio_pane(&old_key, window, cx);
                if Self::is_drawio_path(&renamed) {
                    self.add_drawio_pane(new_key.clone(), full.clone(), window, cx);
                }
            }
            if let Some(pane) = self.documents.image_panes.remove(&old_key) {
                if Self::is_image_path(&renamed) {
                    pane.update(cx, |pane, cx| pane.set_path(full.clone(), cx));
                    self.documents.image_panes.insert(new_key, pane);
                }
            }
        }
        for path in self.documents.tabs.iter_mut().chain(self.documents.reading_panes.iter_mut()) {
            if path == key { *path = renamed.clone(); }
        }
        if self.documents.active == key { self.documents.active = renamed.clone(); }
        if self.documents.focused_markdown.as_deref() == Some(key) { self.documents.focused_markdown = Some(renamed.clone()); }
        if self.focused_file.as_deref() == Some(key) { self.focused_file = Some(renamed.clone()); }
        for paths in [&mut self.selected_files, &mut self.documents.markdown_changed] {
            if paths.remove(key) { paths.insert(renamed.clone()); }
        }
        if self.default_workspace.main_file == key && self.directory_workspace().is_some_and(|p| p.id == self.default_workspace.id) {
            self.default_workspace.main_file = renamed.clone();
        }
        if let Some(project) = self.config.projects.iter_mut().find(|p| Some(&p.id) == self.config.active_id.as_ref()) {
            if project.main_file == key {
                project.main_file = if renamed.to_ascii_lowercase().ends_with(".tex") { renamed } else { String::new() };
            }
        }
    }

    pub(super) fn open_reading_pane(
        &mut self,
        file: &ProjectFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.reading_panes.contains(&file.path) {
            return;
        }
        let key = PaneKey::reading(&file.path);
        if let Err(error) = self.mount_document_pane(&key, file.kind, window, cx) {
            self.report(error, cx);
            return;
        }
        self.documents.reading_panes.push(file.path.clone());
        self.documents.focused_markdown = None;
        if let Some(pane) = self.documents.markdown_panes.get(&key) {
            pane.update(cx, |pane, cx| pane.focus(window, cx));
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }

    pub(super) fn close_reading_pane(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.documents.reading_panes.retain(|pane| pane != path);
        if !self.documents.tabs.iter().any(|tab| tab == path) {
            self.close_tab(path, window, cx);
            // A cancelled draft close keeps its document accessible.
            if self.documents.open.contains_key(path) {
                self.documents.reading_panes.push(path.to_owned());
                return;
            }
        }
        let key = PaneKey::reading(path);
        self.documents.markdown_panes.remove(&key);
        self.release_pdf_pane(&key, window, cx);
        self.release_html_pane(&key, window, cx);
        self.release_drawio_pane(&key, window, cx);
        self.documents.image_panes.remove(&key);
        if self.documents.focused_markdown.as_deref() == Some(path) {
            self.documents.focused_markdown = None;
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }

    pub(super) fn render_reading_pane(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let close = path.to_owned();
        let is_pdf = path.to_ascii_lowercase().ends_with(".pdf");
        let key = PaneKey::reading(path);
        let markdown = self.documents.markdown_panes.get(&key);
        let show_background = self.has_visible_background() && markdown.is_some();
        let source = self.document_path(path);
        let filename = Path::new(path)
            .file_name()
            .expect("reading pane has filename")
            .to_string_lossy()
            .into_owned();
        let mut pane = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .when(!show_background, |pane| pane.bg(rgb(0xffffff)));
        let mut titlebar = div()
            .h(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_1()
            .when(!show_background, |bar| bar.bg(rgb(0xfafaf9)))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(crate::ui::file_icon(path, false, false))
            .child(div().min_w_0().text_ellipsis().child(filename))
            .child(div().flex_1());
        if is_pdf {
            let source = source.clone();
            titlebar = titlebar.child(
                button("export-reading-pdf", crate::i18n::workspace::EXPORT_BTN.get()).on_click(
                    cx.listener(move |app, _, window, cx| app.export_pdf(source.clone(), window, cx)),
                ),
            );
        }
        titlebar = titlebar.child(
            crate::ui::control(
                SharedString::from(format!("close-reading-{path}")),
                IconName::Close,
            )
            .on_click(
                cx.listener(move |app, _, window, cx| app.close_reading_pane(&close, window, cx)),
            ),
        );
        pane = pane.child(titlebar);
        if let Some(pdf) = self.documents.pdf_panes.get(&key) {
            pane = pane.child(div().flex_1().min_h_0().child(pdf.clone()));
        } else if let Some(html) = self.documents.html_panes.get(&key) {
            pane = pane.child(div().flex_1().min_h_0().child(html.clone()));
        } else if let Some(drawio) = self.documents.drawio_panes.get(&key) {
            pane = pane.child(div().flex_1().min_h_0().child(drawio.clone()));
        } else if let Some(image) = self.documents.image_panes.get(&key) {
            pane = pane.child(div().flex_1().min_h_0().child(image.clone()));
        } else if let Some(markdown) = markdown {
            pane = pane.child(div().flex_1().min_h_0().child(markdown.clone()));
        }
        pane.into_any_element()
    }
}
