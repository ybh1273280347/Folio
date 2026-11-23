//! Live document buffers and their editor/reader projections.

mod canvas_links;
mod links;
mod view;

use super::*;
use crate::annotations::AnnotationItem;
use crate::canvas::{CanvasEvent, CanvasPane};
use crate::drawio::{DrawioPane, DrawioPaneEvent};
use crate::markdown::{MarkdownEvent, MarkdownPane};
use crate::model::DocumentBuffer;
use crate::pdf::{PdfPane, PdfPaneEvent};
use crate::web_view::{HtmlPane, HtmlPaneEvent};
use gpui_kit::component::input::{
    Editor, EditorState, Position, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PaneRole {
    Tab,
    Reading,
}

/// A pane's role is independent of the path persisted in workspace sessions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PaneKey {
    path: String,
    role: PaneRole,
}

impl PaneKey {
    fn new(path: &str, role: PaneRole) -> Self {
        Self { path: path.to_owned(), role }
    }

    fn tab(path: &str) -> Self {
        Self::new(path, PaneRole::Tab)
    }

    fn reading(path: &str) -> Self {
        Self::new(path, PaneRole::Reading)
    }
}

struct OpenDocument {
    path: PathBuf,
    buffer: DocumentBuffer,
    editor: Entity<EditorState>,
    line_highlight: RangeDecorationCollection,
    line_highlight_timer: Task<()>,
    last_edit: Instant,
    subscription: Subscription,
}

pub(super) struct PendingCanvasInsert {
    pub(super) preview: String,
    pub(super) anchor: crate::canvas::KnowledgeAnchor,
    pub(super) source_folder: Option<String>,
    pub(super) name_input: Entity<gpui_kit::component::input::InputState>,
    pub(super) _submit_sub: Subscription,
}

/// The open buffer is shared by all panes for the same document. Navigation
/// stores paths only, so workspace-session files remain independent of GPUI.
#[derive(Default)]
pub(super) struct Documents {
    open: BTreeMap<String, OpenDocument>,
    tabs: Vec<String>,
    active: String,
    reading_panes: Vec<String>,
    focused_markdown: Option<String>,
    markdown_panes: BTreeMap<PaneKey, Entity<MarkdownPane>>,
    canvas_panes: BTreeMap<PaneKey, Entity<CanvasPane>>,
    pdf_panes: BTreeMap<PaneKey, Entity<PdfPane>>,
    html_panes: BTreeMap<PaneKey, Entity<HtmlPane>>,
    drawio_panes: BTreeMap<PaneKey, Entity<DrawioPane>>,
    image_panes: BTreeMap<PaneKey, Entity<crate::image::ImagePane>>,
    markdown_changed: BTreeSet<String>,
    canvas_changed: BTreeSet<String>,
    active_canvas_backlinks: Vec<crate::canvas::CanvasBacklink>,
    pending_canvas_insert: Option<PendingCanvasInsert>,
    source_version: u64,
    document_watch: DocumentWatch,
}

impl Documents {
    pub(super) fn active(&self) -> &str {
        &self.active
    }

    pub(super) fn tabs(&self) -> &[String] {
        &self.tabs
    }

    pub(super) fn reading_panes(&self) -> &[String] {
        &self.reading_panes
    }

    pub(super) fn focused_markdown(&self) -> Option<&str> {
        self.focused_markdown.as_deref()
    }

    pub(super) fn clear_pending_canvas_insert(&mut self) {
        self.pending_canvas_insert = None;
    }

    pub(super) fn open_buffers(&self) -> impl Iterator<Item = (&str, &DocumentBuffer)> {
        self.open.iter().map(|(path, doc)| (path.as_str(), &doc.buffer))
    }

    pub(super) fn active_drawio(&self) -> Option<&Entity<DrawioPane>> {
        self.drawio_panes.get(&PaneKey::tab(&self.active))
    }

    pub(super) fn open_buffer(&self, path: &str) -> Option<&DocumentBuffer> {
        self.open.get(path).map(|doc| &doc.buffer)
    }

    pub(super) fn canvas_pane(&self, path: &str) -> Option<&Entity<CanvasPane>> {
        self.canvas_panes
            .get(&PaneKey::tab(path))
            .or_else(|| self.canvas_panes.get(&PaneKey::reading(path)))
    }

    pub(super) fn active_canvas(&self) -> Option<(String, &Entity<CanvasPane>)> {
        if let Some(pane) = self.canvas_panes.get(&PaneKey::tab(&self.active)) {
            return Some((self.active.clone(), pane));
        }
        self.reading_panes.iter().rev().find_map(|path| {
            self.canvas_panes
                .get(&PaneKey::reading(path))
                .map(|pane| (path.clone(), pane))
        })
    }

    pub(super) fn active_markdown_search_open(&self, cx: &App) -> bool {
        self.markdown_panes.get(&PaneKey::tab(&self.active))
            .is_some_and(|pane| pane.read(cx).is_search_open())
    }

    pub(super) fn markdown_export_pane(&self, path: &str) -> &Entity<MarkdownPane> {
        self.markdown_panes.get(&PaneKey::reading(path))
            .unwrap_or_else(|| &self.markdown_panes[&PaneKey::tab(path)])
    }

    pub(super) fn set_backdrop_visible(&self, visible: bool, cx: &mut App) {
        for pane in self.markdown_panes.values() {
            pane.update(cx, |pane, cx| pane.set_backdrop_visible(visible, cx));
        }
    }

    pub(super) fn set_zen_mode(&self, zen: bool, cx: &mut App) {
        for pane in self.markdown_panes.values() {
            pane.update(cx, |pane, cx| pane.set_zen_mode(zen, cx));
        }
    }

    pub(super) fn apply_editor_appearance(&self, previous: &AppSettings, settings: &AppSettings, window: &mut Window, cx: &mut App) {
        if previous.show_line_numbers != settings.show_line_numbers {
            for document in self.open.values() {
                document.editor.update(cx, |editor, cx| editor.set_line_number(settings.show_line_numbers, window, cx));
            }
        }
        if previous.show_line_numbers != settings.show_line_numbers || previous.line_width_limited != settings.line_width_limited
            || previous.editor_font != settings.editor_font || previous.editor_font_size != settings.editor_font_size
            || previous.markdown_font != settings.markdown_font || previous.markdown_font_size != settings.markdown_font_size
            || previous.interface_font != settings.interface_font
            || previous.inline_code_font != settings.inline_code_font || previous.block_code_font != settings.block_code_font
            || previous.inline_code_scale != settings.inline_code_scale || previous.block_code_scale != settings.block_code_scale
            || previous.text_color_presets != settings.text_color_presets
        {
            for pane in self.markdown_panes.values() {
                pane.update(cx, |pane, cx| pane.apply_settings(settings, window, cx));
            }
        }
    }

    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        for pane in self.markdown_panes.values() {
            pane.update(cx, |pane, cx| pane.refresh_locale(window, cx));
        }
        for pane in self.pdf_panes.values() {
            pane.update(cx, |pane, cx| pane.refresh_locale(window, cx));
        }
    }

    pub(super) fn poll_web_panes(&self, window: &mut Window, cx: &mut App) {
        for pane in self.html_panes.values() {
            pane.update(cx, |pane, cx| pane.poll(window, cx));
        }
        for pane in self.drawio_panes.values() {
            pane.update(cx, |pane, cx| pane.poll(window, cx));
        }
    }

    /// Native web surfaces must be hidden behind in-window overlays.
    pub(super) fn sync_pane_visibility(&self, zen: bool, trash_open: bool, modal_open: bool, window: &mut Window, cx: &mut App) {
        let visible = |key: &PaneKey| (!zen && key.role == PaneRole::Reading) || key.path == self.active;
        for (key, pane) in &self.pdf_panes {
            pane.update(cx, |pane, cx| pane.set_visible(!trash_open && visible(key), window, cx));
        }
        for (key, pane) in &self.html_panes {
            pane.update(cx, |pane, cx| pane.set_visible(!modal_open && visible(key), window, cx));
        }
        for (key, pane) in &self.drawio_panes {
            pane.update(cx, |pane, cx| pane.set_visible(!modal_open && visible(key), window, cx));
        }
    }

    pub(super) fn replace_watch(&mut self, watch: DocumentWatch) {
        self.document_watch = watch;
    }

    pub(super) fn restore_active_tab(&mut self, saved: &str) {
        if self.tabs.iter().any(|tab| tab == saved) {
            self.active = saved.to_owned();
        }
    }

    pub(super) fn select_last_tab_if_closed(&mut self) {
        if !self.tabs.contains(&self.active) {
            self.active = self.tabs.last().cloned().unwrap_or_default();
        }
    }

    pub(super) fn tracked_paths(&self) -> impl Iterator<Item = &String> {
        self.tabs.iter().chain(&self.reading_panes).chain(self.open.keys())
    }

    pub(super) fn needs_save_before_removal(&self, path: &str) -> bool {
        self.open.get(path).is_some_and(|document| {
            document.buffer.is_dirty() || document.buffer.conflict.is_some()
        })
    }

    pub(super) fn open_buffer_at(&self, path: &Path) -> Option<(&str, &DocumentBuffer)> {
        self.open.iter().find_map(|(key, document)| {
            (dunce::canonicalize(&document.path).ok().as_deref() == Some(path))
                .then_some((key.as_str(), &document.buffer))
        })
    }

    pub(super) fn source_version(&self) -> u64 {
        self.source_version
    }

    fn source_changed(&mut self, path: &str) {
        if path.to_ascii_lowercase().ends_with(".canvas") {
            self.canvas_changed.insert(path.to_owned());
        } else {
            self.markdown_changed.insert(path.to_owned());
        }
        self.source_version += 1;
    }
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

    fn invalidate_tree(&mut self) {
        self.paths.clear();
        self.tree = true;
    }
}

impl Workspace {
    /// Both editable Markdown projections and the plain editor submit the same draft.
    pub(super) fn edit_document(&mut self, path: &str, text: String) {
        let document = self.documents.open.get_mut(path).expect("editor owns an open document");
        if document.buffer.text != text {
            document.buffer.set_text(text);
            document.last_edit = Instant::now();
            self.documents.source_changed(path);
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

    pub(super) fn is_canvas_path(path: &str) -> bool {
        path.to_ascii_lowercase().ends_with(".canvas")
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
        self.projects.directory()
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

    fn release_html_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
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

    fn release_drawio_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.documents.drawio_panes.remove(key) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn release_all_drawio_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, pane) in std::mem::take(&mut self.documents.drawio_panes) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    fn release_pdf_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
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
        cx.subscribe_in(&pane, window, |app, _, event: &PdfPaneEvent, window, cx| match event {
            PdfPaneEvent::AskAi { path, text } => {
                let source = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "PDF".to_owned());
                app.open_ai_with_selection(source, Some(path.clone()), text.clone(), window, cx);
            }
            PdfPaneEvent::SendToCanvas {
                path,
                annotation_id,
                page_1based,
                quote,
                rects,
            } => {
                app.send_pdf_annotation_to_canvas(
                    path,
                    *annotation_id,
                    *page_1based,
                    quote.clone(),
                    rects,
                    window,
                    cx,
                );
            }
        })
        .detach();
        pane
    }

    pub(super) fn document_path(&self, key: &str) -> PathBuf {
        self.projects.directory()
            .map(|project| PathBuf::from(&project.folder).join(key))
            .unwrap_or_else(|| PathBuf::from(key))
    }

    fn release_document_pane(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        self.release_pdf_pane(key, window, cx);
        self.release_html_pane(key, window, cx);
        self.release_drawio_pane(key, window, cx);
        self.documents.image_panes.remove(key);
        self.documents.markdown_panes.remove(key);
        self.documents.canvas_panes.remove(key);
    }

    /// The file tree calls this only after moving the file to trash succeeds.
    pub(super) fn detach_removed_document(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.documents.open.remove(path);
        for role in [PaneRole::Tab, PaneRole::Reading] {
            self.release_document_pane(&PaneKey::new(path, role), window, cx);
        }
        self.documents.reading_panes.retain(|pane| pane != path);
        self.documents.tabs.retain(|tab| tab != path);
        if self.documents.focused_markdown.as_deref() == Some(path) {
            self.documents.focused_markdown = None;
        }
    }

    pub(super) fn invalidate_document_tree(&mut self) {
        self.documents.document_watch.invalidate_tree();
    }

    pub(super) fn refresh_all_files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        // Manual refresh reconciles all drafts, regardless of watcher event filtering.
        self.documents.document_watch.invalidate_tree();
        self.documents.document_watch.changed_at = None;
        self.refresh_files(window, cx)
    }

    pub(super) fn clear_open_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_all_pdf_panes(window, cx);
        self.release_all_html_panes(window, cx);
        self.release_all_drawio_panes(window, cx);
        self.documents.image_panes.clear();
        self.documents.open.clear();
        self.documents.tabs.clear();
        self.documents.active.clear();
        self.documents.reading_panes.clear();
        self.documents.markdown_panes.clear();
        self.documents.canvas_panes.clear();
        self.documents.focused_markdown = None;
        self.documents.markdown_changed.clear();
        self.documents.canvas_changed.clear();
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
            || lower.ends_with(".canvas")
            || lower.ends_with(".pdf")
            || Self::is_html_path(&lower)
            || Self::is_drawio_path(&lower)
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
        self.tree.dismiss_trash();
        self.documents.focused_markdown = None;
        let tab = PaneKey::tab(&file.path);
        self.mount_document_pane(&tab, file.kind, window, cx)?;
        if !self.documents.tabs.contains(&file.path) {
            self.documents.tabs.push(file.path.clone());
        }
        self.documents.active = file.path.clone();
        self.refresh_active_canvas_backlinks();
        if focus_editor {
            if let Some(pane) = self.documents.markdown_panes.get(&tab) {
                pane.update(cx, |pane, cx| pane.focus(window, cx));
            } else if let Some(pane) = self.documents.canvas_panes.get(&tab) {
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

    pub(in crate::workspace) fn open_kb_location(
        &mut self,
        rel_path: &str,
        line_1based: Option<usize>,
        page_1based: Option<usize>,
        heading: Option<&str>,
        snippet: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.directory() else {
            return;
        };
        let Ok(canonical_root) = dunce::canonicalize(&project.folder) else {
            return;
        };
        let clean = rel_path.strip_suffix("#annotations").unwrap_or(rel_path);
        let abs = canonical_root.join(clean.replace('/', "\\"));
        let Ok(resolved) = dunce::canonicalize(&abs) else {
            self.report(crate::i18n::workspace::err_file_not_found(&abs), cx);
            return;
        };
        if !resolved.is_file() || !files::is_relevant(&resolved) {
            self.report(crate::i18n::workspace::err_unsupported_open(&resolved), cx);
            return;
        }
        let Ok(relative) = resolved.strip_prefix(&canonical_root) else {
            return;
        };
        if self.right_panel.maximized {
            self.right_panel.maximized = false;
        }
        let key_path = relative.to_string_lossy().replace('\\', "/");
        let file = ProjectFile {
            kind: Self::file_kind(&resolved),
            path: key_path.clone(),
        };
        if let Err(error) = self.open_file(&file, window, cx) {
            self.report(error, cx);
            return;
        }
        let pane_key = PaneKey::tab(&key_path);
        if let Some(page) = page_1based
            && let Some(pdf) = self.documents.pdf_panes.get(&pane_key).cloned()
        {
            pdf.update(cx, |pane, cx| pane.jump_to_page(page, cx));
        } else if let Some(md) = self.documents.markdown_panes.get(&pane_key).cloned() {
            let clean_heading = heading
                .and_then(|h| h.rsplit(" > ").next())
                .map(crate::rag::pipeline::clean_heading_text);
            let s_owned = snippet.map(str::to_owned);
            md.update(cx, |pane, cx| {
                pane.jump_to_search_hit(
                    line_1based,
                    clean_heading.as_deref(),
                    s_owned.as_deref(),
                    window,
                    cx,
                );
            });
        } else if let Some(line) = line_1based {
            self.jump_to_document_line(&key_path, line, window, cx);
        }
        cx.notify();
    }

    fn jump_to_document_line(
        &mut self,
        path: &str,
        line_1based: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.open.get_mut(path) else {
            return;
        };
        let line0 = line_1based.saturating_sub(1);
        let mut highlight_range = None;
        document.editor.update(cx, |editor, cx| {
            let text = editor.value().to_string();
            highlight_range = crate::workspace::line_byte_range(&text, line0);
            editor.set_cursor_position(Position::new(line0 as u32, 0), window, cx);
            if let Some(line_height) = editor.line_height() {
                let row = editor.display_map().buffer_line_to_display_row(line0);
                let mut offset = editor.scroll_offset();
                offset.y = editor.input_bounds().size.height * 0.25 - line_height * row;
                editor.set_scroll_offset(offset, cx);
            }
            editor.focus(window, cx);
        });
        if let Some(range) = highlight_range {
            document.line_highlight.set(
                vec![RangeDecoration::new(range)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(gpui_kit::base::Theme::global(cx).tokens.colors.selection)],
                cx,
            );
            let key = path.to_owned();
            document.line_highlight_timer = cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1400))
                    .await;
                let _ = this.update(cx, |app, cx| {
                    if let Some(doc) = app.documents.open.get(&key) {
                        doc.line_highlight.clear(cx);
                    }
                });
            });
        }
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
            let disk = if self.projects.directory().is_some() {
                files::read_document(
                    Path::new(&self.projects.directory().expect("project was checked").folder),
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
                    .default_value(
                        if path.to_ascii_lowercase().ends_with(".md") || Self::is_canvas_path(path) {
                            String::new()
                        } else {
                            disk.text.clone()
                        },
                    )
            });
            let line_highlight = editor.update(cx, |editor, cx| {
                editor.create_range_decorations_collection(Vec::new(), cx)
            });
            let subscription = self.subscribe_editor(path.clone(), &editor, cx);
            self.documents.open.insert(
                path.clone(),
                OpenDocument {
                    path: full.clone(),
                    buffer: DocumentBuffer::from_disk(disk),
                    editor,
                    line_highlight,
                    line_highlight_timer: Task::ready(()),
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
        if Self::is_canvas_path(path) && !self.documents.canvas_panes.contains_key(key) {
            self.add_canvas_pane(path, key.role == PaneRole::Reading, window, cx);
        }
        Ok(())
    }

    fn add_canvas_pane(
        &mut self,
        path: &str,
        reading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.documents.open[path].buffer.text.clone();
        let full = self.documents.open[path].path.clone();
        let pane = cx.new(|cx| CanvasPane::new(&text, full, window, cx));
        cx.subscribe_in(&pane, window, move |app, pane, event: &CanvasEvent, window, cx| {
            let full = pane.read(cx).file_path().to_path_buf();
            let Some(file) = app
                .documents
                .open
                .iter()
                .find(|(_, doc)| doc.path == full)
                .map(|(k, _)| k.clone())
            else {
                return;
            };
            match event {
                CanvasEvent::Change(next_text) => {
                    app.edit_document(&file, next_text.clone());
                }
                CanvasEvent::Focus => {
                    app.documents.focused_markdown = None;
                }
                CanvasEvent::FilesChanged => {
                    if let Err(error) = app.refresh_files(window, cx) {
                        app.report(error, cx);
                    }
                }
                CanvasEvent::Notice(message) => {
                    app.report(message.clone(), cx);
                }
                CanvasEvent::JumpToAnchor {
                    anchor,
                    fallback_file,
                    fallback_subpath,
                } => {
                    app.jump_from_canvas_anchor(
                        &full,
                        anchor.as_ref(),
                        fallback_file.as_deref(),
                        fallback_subpath.as_deref(),
                        window,
                        cx,
                    );
                }
            }
            cx.notify();
        })
        .detach();
        self.documents.canvas_panes.insert(
            PaneKey::new(path, if reading { PaneRole::Reading } else { PaneRole::Tab }),
            pane,
        );
        self.sync_canvas_index_from_text(path, &text);
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
                MarkdownEvent::SendToCanvas {
                    quote,
                    occurrence,
                    heading_path,
                    line_hint,
                } => {
                    app.send_markdown_selection_to_canvas(
                        &file,
                        quote.clone(),
                        *occurrence,
                        heading_path.clone(),
                        *line_hint,
                        window,
                        cx,
                    );
                }
                MarkdownEvent::FilesChanged => {
                    if let Err(error) = app.refresh_files(window, cx) {
                        app.report(error, cx);
                    }
                }
                MarkdownEvent::UpdateTextColorPresets(presets) => {
                    let mut settings = app.settings.clone();
                    settings.text_color_presets =
                        crate::model::sanitize_text_color_presets(presets.clone());
                    app.apply_settings(settings, window, cx);
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
        let root = self.projects.directory().map(|project| PathBuf::from(&project.folder));
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
                let saved_text = saved.text.clone();
                document.buffer.saved_text = saved.text;
                document.buffer.dirty = false;
                document.buffer.revision = saved.revision;
                document.buffer.conflict = None;
                self.sync_canvas_index_from_text(path, &saved_text);
                self.schedule_rag_sync();
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
    /// Publish a prepared Markdown replacement only after the disk revision matches.
    /// Update both editable views immediately without changing tabs or keyboard focus.
    pub(super) fn save_markdown_snapshot(
        &mut self,
        path: &Path,
        open_key: Option<&str>,
        text: &str,
        expected_revision: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<bool> {
        let Ok(saved) = files::save_document_at(path, text, expected_revision)? else {
            return Ok(false);
        };
        if let Some(key) = open_key {
            self.documents.open.get_mut(key).expect("opened document").buffer.replace_from_disk(saved);
            self.documents.source_changed(key);
        }
        for pane in self.documents.markdown_panes.values() {
            if dunce::canonicalize(pane.read(cx).path()).ok().as_deref() == Some(path) {
                pane.update(cx, |pane, cx| pane.sync(text, window, cx));
            }
        }
        self.schedule_rag_sync();
        Ok(true)
    }

    /// Refresh every open projection after the annotation sidecar has been saved.
    pub(super) fn sync_document_annotations(
        &self,
        path: &Path,
        items: &[AnnotationItem],
        cx: &mut Context<Self>,
    ) {
        for pane in self.documents.markdown_panes.values() {
            if dunce::canonicalize(pane.read(cx).path()).ok().as_deref() == Some(path) {
                pane.update(cx, |pane, cx| pane.sync_annotations(items, cx));
            }
        }
        for pane in self.documents.pdf_panes.values() {
            if dunce::canonicalize(pane.read(cx).path()).ok().as_deref() == Some(path) {
                pane.update(cx, |pane, cx| pane.sync_annotations(items, cx));
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
    pub(super) fn focus_document_tab(&self, path: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(pane) = self.documents.markdown_panes.get(&PaneKey::tab(path)) {
            pane.update(cx, |pane, cx| pane.focus(window, cx));
        } else if let Some(pane) = self.documents.canvas_panes.get(&PaneKey::tab(path)) {
            pane.update(cx, |pane, cx| pane.focus(window, cx));
        } else if let Some(document) = self.documents.open.get(path) {
            document.editor.update(cx, |editor, cx| editor.focus(window, cx));
        } else {
            return false;
        }
        true
    }

    pub(super) fn select_tab_from_tree(&mut self, path: &str, focus_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.documents.active = path.to_owned();
        self.refresh_active_canvas_backlinks();
        if focus_editor {
            self.focus_document_tab(path, window, cx);
        } else {
            self.tree.focus(window, cx);
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }

    pub(super) fn activate_tab(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.documents.active == path && !self.tree.is_trash_open() {
            return;
        }
        self.tree.dismiss_trash();
        self.documents.focused_markdown = None;
        self.documents.active = path.to_owned();
        self.refresh_active_canvas_backlinks();
        self.focus_document_tab(path, window, cx);
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
                || self.dialogs.has_response(Dialog::CloseTab))
        {
            let retry_path = path.to_owned();
            match self.ask(
                Dialog::CloseTab,
                crate::i18n::workspace::dialog_close_unsaved_title(path),
                crate::i18n::workspace::DIALOG_UNSAVED_FILE_BODY.get().into(),
                DialogButtons::SaveDiscardCancel,
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
        self.release_document_pane(&PaneKey::tab(path), window, cx);
        if !self.documents.reading_panes.iter().any(|pane| pane == path) {
            self.documents.open.remove(path);
        }
        if self.documents.focused_markdown.as_deref() == Some(path) {
            self.documents.focused_markdown = None;
        }
        if self.documents.active == path {
            self.documents.active = self.documents.tabs.last().cloned().unwrap_or_default();
            if !self.tree.is_trash_open()
                && let Some(document) = self.documents.open.get(&self.documents.active)
            {
                document
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
            }
        }
        self.refresh_active_canvas_backlinks();
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }
    pub(super) fn reload_disk(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let disk = if let Some(project) = self.projects.directory() {
            files::read_document(Path::new(&project.folder), path)
        } else {
            files::read_document_at(&self.documents.open[path].path)
        };
        match disk {
            Ok(disk) => {
                let document = self.documents.open
                    .get_mut(path)
                    .expect("reload only open documents");
                if !path.to_ascii_lowercase().ends_with(".md") && !Self::is_canvas_path(path) {
                    document.editor.update(cx, |editor, cx| {
                        editor.set_value(disk.text.clone(), window, cx)
                    });
                }
                document.buffer.replace_from_disk(disk);
                self.documents.source_changed(path);
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
        let root = self.projects.directory().map(|project| PathBuf::from(&project.folder));
        let previous: BTreeSet<_> = self.tree.files().iter().map(|file| file.path.clone()).collect();
        let refresh_tree = self.documents.document_watch.paths.is_empty() || self.documents.document_watch.tree;
        if let Some(root) = &root {
            if refresh_tree {
                self.tree.reload_watched_listing(root)?;
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
        if refresh_tree {
            self.reconcile_rag_on_tree_change(&previous);
        }
        if refresh_tree || !changed.is_empty() {
            self.schedule_rag_sync();
        }
        self.queue_math_preload(
            self.tree.files()
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
        let mut updated_sources = Vec::new();
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
                        if !path.to_lowercase().ends_with(".md") && !Self::is_canvas_path(path) {
                            document.editor.update(cx, |editor, cx| {
                                editor.set_value(disk.text.clone(), window, cx)
                            });
                        }
                        document.buffer.replace_from_disk(disk);
                        updated_sources.push(path.clone());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    document.buffer.deleted = true
                }
                Err(error) => return Err(error),
            }
        }
        for path in updated_sources {
            self.documents.source_changed(&path);
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
        for path in std::mem::take(&mut self.documents.canvas_changed) {
            if let Some(document) = self.documents.open.get(&path) {
                for role in [PaneRole::Tab, PaneRole::Reading] {
                    if let Some(pane) = self.documents.canvas_panes.get(&PaneKey::new(&path, role)) {
                        pane.update(cx, |pane, cx| {
                            pane.sync_text(&document.buffer.text, window, cx)
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
                    let root = self.projects.directory().map(|project| PathBuf::from(&project.folder));
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
                                        || (tree && self.tree.folders().iter().any(|folder| root.join(folder) == *path)))
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
        if !self.tree.name_dialog_open() && !self.dialogs.is_open() {
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
        let renamed = if let Some(workspace) = self.projects.directory() {
            full.strip_prefix(&workspace.folder).expect("same workspace").to_string_lossy().replace('\\', "/")
        } else { full.to_string_lossy().into_owned() };
        self.relocate_file_state(key, &renamed, window, cx);
        let annotation_result = crate::annotations::relocate_annotations(&old, &full);
        let relocated = [(key.to_owned(), renamed.clone())];
        self.relocate_tags(&relocated, false, cx);
        self.relocate_rag_paths(&relocated, false, cx);
        self.relocate_canvas_links(&relocated, false, cx);
        if let Some(workspace) = self.projects.directory() {
            let root = PathBuf::from(&workspace.folder);
            self.tree.reload_files(&root)?;
            self.projects.save()?;
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

    pub(super) fn relocate_file_state(&mut self, key: &str, renamed: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.relocate_document(key, renamed, window, cx);
        self.tree.relocate_file(key, renamed);
        self.projects.relocate_main_file(key, renamed);
    }

    // Keep live buffers and Markdown panes: moving a file must not reload unsaved edits.
    fn relocate_document(&mut self, key: &str, renamed: &str, window: &mut Window, cx: &mut Context<Self>) {
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
            if let Some(pane) = self.documents.canvas_panes.remove(&old_key)
                && Self::is_canvas_path(&renamed)
            {
                pane.update(cx, |pane, cx| pane.set_path(full.clone(), cx));
                self.documents.canvas_panes.insert(new_key.clone(), pane);
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
        if self.documents.markdown_changed.remove(key) {
            self.documents.markdown_changed.insert(renamed.clone());
        }
        if self.documents.canvas_changed.remove(key) {
            self.documents.canvas_changed.insert(renamed.clone());
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
        } else if let Some(pane) = self.documents.canvas_panes.get(&key) {
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
        self.release_document_pane(&PaneKey::reading(path), window, cx);
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
        } else if let Some(canvas) = self.documents.canvas_panes.get(&key) {
            pane = pane.child(div().flex_1().min_h_0().child(canvas.clone()));
        }
        pane.into_any_element()
    }
}
