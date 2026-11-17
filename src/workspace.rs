//! Workspace orchestration: disk-backed drafts, project actions and asynchronous builds.
//! Disk revisions arbitrate external edits; conflicted drafts never autosave over them.
use crate::ai::{AiPane, AiPaneEvent, AttachedFile};
use crate::theme::color as rgb;
use crate::html::{HtmlPane, HtmlPaneEvent};
use crate::markdown::{MarkdownEvent, MarkdownPane};
use crate::math::MathRenderer;
use crate::pdf::{PdfPane, PdfPaneEvent};
use crate::preferences::{Preferences, SettingsChanged};
use crate::terminal::{TerminalPane, TerminalPaneEvent};
use crate::{
    compiler, files,
    model::{
        AppSettings, BuildStatus, DocumentBuffer, Engine, FileKind, InternalLinkOpenTarget,
        Project, ProjectConfig, ProjectFile, WorkspaceLocation, WorkspaceSession,
    },
    storage,
};
use gpui_kit::{
    assets::IconName,
    base::{Scrollbar, ScrollbarMode, StyledExt},
    component::{
        button::{Button, ButtonVariants},
        input::{Editor, EditorState, Input, InputEvent, InputState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        progress::Progress,
        resizable::{h_resizable, resizable_panel, v_resizable},
    },
    prelude::FluentBuilder,
    *,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sha2::Digest;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

actions!(
    folio,
    [
        Save,
        SaveAll,
        OpenProject,
        OpenFile,
        Compile,
        ToggleSidebar,
        FindFile,
        CloseTab,
        CopyFilePath,
        DeleteFileTreeItem,
        UndoFileDelete,
        Dismiss
    ]
);
const BG: u32 = 0xf7f6f5;
const BORDER: u32 = 0xe8e7e3;
const MUTED: u32 = 0x787774;

struct TreeSelectionDrag {
    start: Point<Pixels>,
    current: Point<Pixels>,
    original: BTreeSet<String>,
    moved: bool,
}

impl TreeSelectionDrag {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.start.x.min(self.current.x), self.start.y.min(self.current.y)),
            point(self.start.x.max(self.current.x), self.start.y.max(self.current.y)),
        )
    }
}

struct OpenDocument {
    path: PathBuf,
    buffer: DocumentBuffer,
    editor: Entity<EditorState>,
    last_edit: Instant,
    _subscription: Subscription,
}
enum BuildEvent {
    Log(String),
    Finished(io::Result<compiler::BuildResult>),
}
#[derive(Clone)]
enum NameAction {
    Create,
    CreateFolder,
    RenameFile(String),
    RenameFolder(String),
    RenameProject(String),
}

pub fn bind_shortcuts(settings: &AppSettings, cx: &mut App) {
    let mut bindings = vec![
        KeyBinding::new("ctrl-c", gpui_kit::component::input::Copy, Some("FolioPdf")),
        KeyBinding::new("ctrl-c", CopyFilePath, Some("FolioFileTree")),
        KeyBinding::new("delete", DeleteFileTreeItem, Some("FolioFileTree")),
        KeyBinding::new("ctrl-z", UndoFileDelete, Some("FolioFileTree")),
        KeyBinding::new("escape", Dismiss, Some("!GpuiXterm")),
    ];
    fn push_key<A: Action>(
        bindings: &mut Vec<KeyBinding>,
        key: String,
        action: A,
        ctx: Option<&'static str>,
    ) {
        let trimmed = key.trim().to_owned();
        if !trimmed.is_empty() {
            let static_key: &'static str = Box::leak(trimmed.into_boxed_str());
            // GPUI evaluates the negation across the complete focus context stack.
            // Workspace shortcuts must not steal control sequences from a TUI.
            bindings.push(KeyBinding::new(static_key, action, ctx.or(Some("!GpuiXterm"))));
        }
    }
    push_key(&mut bindings, settings.key_for("save"), Save, None);
    push_key(&mut bindings, settings.key_for("save_all"), SaveAll, None);
    push_key(
        &mut bindings,
        settings.key_for("open_project"),
        OpenProject,
        None,
    );
    push_key(&mut bindings, settings.key_for("open_file"), OpenFile, None);
    push_key(&mut bindings, settings.key_for("compile"), Compile, None);
    push_key(
        &mut bindings,
        settings.key_for("toggle_sidebar"),
        ToggleSidebar,
        None,
    );
    push_key(&mut bindings, settings.key_for("find_file"), FindFile, None);
    push_key(&mut bindings, settings.key_for("close_tab"), CloseTab, None);
    push_key(
        &mut bindings,
        settings.key_for("toggle_source"),
        crate::markdown::ToggleSource,
        None,
    );
    push_key(
        &mut bindings,
        settings.key_for("md_bold"),
        markdown_editor::ToggleBold,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_italic"),
        markdown_editor::ToggleItalic,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_underline"),
        markdown_editor::ToggleUnderline,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_strikethrough"),
        markdown_editor::ToggleStrikethrough,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_code"),
        markdown_editor::ToggleCode,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_highlight"),
        markdown_editor::ToggleHighlight,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_link"),
        markdown_editor::EditLink,
        Some("BlockEditor"),
    );
    push_key(
        &mut bindings,
        settings.key_for("md_find"),
        crate::markdown::FindInDocument,
        Some("FolioMarkdownVisual"),
    );
    cx.bind_keys(bindings);
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightDrawerTab {
    Terminal,
    Ai,
}

#[derive(Clone, Copy, PartialEq)]
enum Dialog {
    Drafts,
    CloseTab,
    DeleteSelected,
    RemoveProject,
    TrashPurge,
    TrashClear,
    TrashConflict,
}
#[derive(Clone)]
enum DialogButtons {
    SaveDiscardCancel,
    Confirm(String),
    RestoreConflict,
}
#[derive(Clone)]
struct DialogPrompt {
    kind: Dialog,
    title: String,
    description: String,
    buttons: DialogButtons,
}

pub struct Workspace {
    default_workspace: Project,
    config: ProjectConfig,
    workspace_sessions: BTreeMap<String, WorkspaceSession>,
    active_session: Option<String>,
    settings: AppSettings,
    preferences: Entity<Preferences>,
    math: Entity<MathRenderer>,
    show_preferences: bool,
    searching: bool,
    search_input: Entity<InputState>,
    reading_panes: Vec<String>,
    markdown_panes: BTreeMap<String, Entity<MarkdownPane>>,
    // Retain the capture pane until publication, even when its tab closes.
    markdown_export: Option<Entity<MarkdownPane>>,
    focused_markdown: Option<String>,
    markdown_changed: BTreeSet<String>,
    collapsed_folders: BTreeSet<String>,
    focused_folder: Option<String>,
    selected_files: BTreeSet<String>,
    tree_row_bounds: Arc<Mutex<BTreeMap<String, (bool, Bounds<Pixels>)>>>,
    tree_drag: Option<TreeSelectionDrag>,
    trash_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging_tree_item: Arc<AtomicBool>,
    trash_entries: Vec<crate::trash::TrashEntry>,
    show_trash: bool,
    next_trash_cleanup: Instant,
    file_tree_focus: FocusHandle,
    sidebar_scroll: ScrollHandle,
    show_build_preview: bool,
    files: Vec<ProjectFile>,
    folders: Vec<String>,
    documents: BTreeMap<String, OpenDocument>,
    tabs: Vec<String>,
    active: String,
    watcher: Option<RecommendedWatcher>,
    watch_events: Option<mpsc::Receiver<notify::Result<notify::Event>>>,
    watch_changed: Option<Instant>,
    watch_paths: BTreeSet<PathBuf>,
    watch_tree: bool,
    watched_directories: BTreeSet<PathBuf>,
    build_events: Option<mpsc::Receiver<BuildEvent>>,
    cancellation: Option<Arc<AtomicBool>>,
    build_status: BuildStatus,
    build_target: String,
    batch_queue: std::collections::VecDeque<String>,
    batch_next: Option<Instant>,
    batch_target: Option<String>,
    batch_logs: String,
    batch_failed: bool,
    batch_running: bool,
    build_started: Option<Instant>,
    build_duration: Duration,
    logs: String,
    show_logs: bool,
    terminal: Entity<TerminalPane>,
    ai: Entity<AiPane>,
    show_terminal: bool,
    terminal_maximized: bool,
    right_drawer_tab: RightDrawerTab,
    preview: Option<PathBuf>,
    build_pdf: Option<Entity<PdfPane>>,
    pdf_panes: BTreeMap<String, Entity<PdfPane>>,
    html_panes: BTreeMap<String, Entity<HtmlPane>>,
    notice: Option<String>,
    notice_until: Option<Instant>,
    name_action: Option<NameAction>,
    name_input: Entity<InputState>,
    focus: FocusHandle,
    source_version: u64,
    build_version: u64,
    close_requested: bool,
    dialog_response: Option<(Dialog, rfd::MessageDialogResult)>,
    dialog_prompt: Option<DialogPrompt>,
    dialog_retry: Option<Box<dyn FnOnce(&mut Self, &mut Window, &mut Context<Self>)>>,
    dialog_open: bool,
    editor_zoom_hud: crate::ui::ZoomHud,
    _tick: Task<()>,
}

impl Workspace {
    fn file_kind(path: &Path) -> FileKind {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("pdf" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "html" | "htm") => {
                FileKind::Asset
            }
            _ => FileKind::Text,
        }
    }

    fn is_html_path(path: &str) -> bool {
        let lower = path.to_lowercase();
        lower.ends_with(".html") || lower.ends_with(".htm")
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
        key: String,
        file_path: PathBuf,
        root_dir: PathBuf,
        is_reading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = cx.new(|cx| HtmlPane::new(file_path, root_dir, is_reading, cx));
        let close_key = key.strip_prefix("reading:").unwrap_or(&key).to_owned();
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
        self.html_panes.insert(key, pane);
    }

    fn release_html_pane(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.html_panes.remove(key) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    fn release_all_html_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, pane) in std::mem::take(&mut self.html_panes) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    fn release_pdf_pane(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.pdf_panes.remove(key) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    fn release_all_pdf_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, pane) in std::mem::take(&mut self.pdf_panes) {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    fn release_build_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.build_pdf.take() {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = storage::load_projects().expect("无法读取项目列表");
        let mut workspace_sessions = storage::load_workspace_sessions().expect("无法读取工作区标签页");
        // The old default session contained external paths, never default-space files.
        if let Some(session) = workspace_sessions.remove("default") {
            workspace_sessions.insert("loose".into(), session);
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
        let settings = storage::load_settings().expect("无法读取界面设置");
        let math = cx.new(MathRenderer::new);
        math.update(cx, |renderer, cx| renderer.set_appearance(&settings, cx));
        let preferences = cx.new(|cx| Preferences::new(settings.clone(), window, cx));
        let terminal = cx.new(|cx| TerminalPane::new(settings.terminal.clone(), settings.theme.palette, cx));
        cx.subscribe_in(&terminal, window, |app, _, event: &TerminalPaneEvent, window, cx| {
            match event {
                TerminalPaneEvent::Hide => {
                    app.show_terminal = false;
                    app.terminal_maximized = false;
                    app.terminal.update(cx, |pane, _| pane.hide());
                    app.focus.focus(window, cx);
                }
                TerminalPaneEvent::ToggleMaximize => {
                    app.terminal_maximized = !app.terminal_maximized;
                }
            }
            cx.notify();
        })
        .detach();
        let initial_dir = dirs::home_dir().expect("user home");
        let ai = cx.new(|cx| AiPane::new(initial_dir, settings.clone(), math.clone(), window, cx));
        cx.subscribe_in(&ai, window, |app, _, event: &AiPaneEvent, window, cx| match event {
            AiPaneEvent::OpenFile(path) => {
                app.open_path_from_ai(path, window, cx);
            }
            AiPaneEvent::ToggleMaximize => {
                app.terminal_maximized = !app.terminal_maximized;
                cx.notify();
            }
            AiPaneEvent::Hide => {
                app.show_terminal = false;
                app.terminal_maximized = false;
                app.focus.focus(window, cx);
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&preferences, window, |app, _, event: &SettingsChanged, window, cx| {
            app.settings = event.0.clone();
            app.terminal.update(cx, |pane, cx| pane.sync(&app.settings.terminal, app.settings.theme.palette, cx));
            app.ai.update(cx, |pane, cx| pane.sync_settings(&app.settings, cx));
            for document in app.documents.values() {
                document.editor.update(cx, |editor, cx| {
                    editor.set_line_number(app.settings.show_line_numbers, window, cx);
                });
            }
            crate::theme::apply(app.settings.theme.palette, cx);
            crate::theme::apply_font(&app.settings.interface_font, cx);
            bind_shortcuts(&app.settings, cx);
            app.math.update(cx, |renderer, cx| {
                renderer.set_appearance(&app.settings, cx)
            });
            app.markdown_changed.extend(app.documents.keys().cloned());
            cx.notify();
        })
        .detach();
        let name_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(
            &name_input,
            window,
            |app, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) && app.name_action.is_some() {
                    if let Err(error) = app.apply_name(window, cx) {
                        app.report(error, cx);
                    }
                }
            },
        )
        .detach();
        let search_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&search_input, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let entity = cx.entity();
        window.on_window_should_close(cx, move |window, cx| {
            entity.update(cx, |app, cx| app.request_close(window, cx))
        });
        let tick = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this
                    .update_in(cx, |app, window, cx| app.poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut app = Self {
            default_workspace,
            config,
            workspace_sessions,
            active_session: None,
            settings,
            preferences,
            math,
            show_preferences: false,
            searching: false,
            search_input,
            reading_panes: Vec::new(),
            markdown_panes: BTreeMap::new(),
            markdown_export: None,
            focused_markdown: None,
            markdown_changed: BTreeSet::new(),
            collapsed_folders: BTreeSet::new(),
            focused_folder: None,
            selected_files: BTreeSet::new(),
            tree_row_bounds: Arc::new(Mutex::new(BTreeMap::new())),
            tree_drag: None,
            trash_bounds: Arc::new(Mutex::new(None)),
            dragging_tree_item: Arc::new(AtomicBool::new(false)),
            trash_entries: Vec::new(),
            show_trash: false,
            next_trash_cleanup: Instant::now(),
            file_tree_focus: cx.focus_handle(),
            sidebar_scroll: ScrollHandle::new(),
            show_build_preview: false,
            files: Vec::new(),
            folders: Vec::new(),
            documents: BTreeMap::new(),
            tabs: Vec::new(),
            active: String::new(),
            watcher: None,
            watch_events: None,
            watch_changed: None,
            watch_paths: BTreeSet::new(),
            watch_tree: false,
            watched_directories: BTreeSet::new(),
            build_events: None,
            cancellation: None,
            build_status: BuildStatus::Idle,
            build_target: String::new(),
            batch_queue: Default::default(),
            batch_next: None,
            batch_target: None,
            batch_logs: String::new(),
            batch_failed: false,
            batch_running: false,
            build_started: None,
            build_duration: Duration::ZERO,
            logs: String::new(),
            show_logs: false,
            terminal,
            ai,
            show_terminal: false,
            terminal_maximized: false,
            right_drawer_tab: RightDrawerTab::Terminal,
            preview: None,
            build_pdf: None,
            pdf_panes: BTreeMap::new(),
            html_panes: BTreeMap::new(),
            notice: None,
            notice_until: None,
            name_action: None,
            name_input,
            focus: cx.focus_handle(),
            source_version: 0,
            build_version: 0,
            close_requested: false,
            dialog_response: None,
            dialog_prompt: None,
            dialog_retry: None,
            dialog_open: false,
            editor_zoom_hud: crate::ui::ZoomHud::default(),
            _tick: tick,
        };
        let result = match initial_location {
            WorkspaceLocation::Default => app.select_project("__folio_default", window, cx),
            WorkspaceLocation::Project(id) => app.select_project(&id, window, cx),
            WorkspaceLocation::LooseFiles => app.select_loose_files(window, cx),
        };
        if let Err(error) = result {
            app.report(error, cx);
        }
        let arguments: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !arguments.is_empty() {
            app.open_paths(arguments, window, cx);
        }
        app.focus.focus(window, cx);
        app
    }

    fn directory_workspace(&self) -> Option<&Project> {
        if self.config.active_workspace == Some(WorkspaceLocation::Default) {
            return Some(&self.default_workspace);
        }
        self.config
            .projects
            .iter()
            .find(|project| Some(&project.id) == self.config.active_id.as_ref())
    }

    fn terminal_directory(&self) -> PathBuf {
        self.directory_workspace().map(|project| PathBuf::from(&project.folder))
            .or_else(|| (!self.active.is_empty()).then(|| self.document_path(&self.active)).and_then(|path| path.parent().map(Path::to_path_buf)))
            .unwrap_or_else(|| dirs::home_dir().expect("user home"))
    }

    fn toggle_right_drawer(&mut self, tab: RightDrawerTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_terminal && self.right_drawer_tab == tab {
            self.show_terminal = false;
            self.terminal_maximized = false;
            self.terminal.update(cx, |pane, _| pane.hide());
            self.focus.focus(window, cx);
        } else {
            self.show_terminal = true;
            self.terminal_maximized = false;
            self.right_drawer_tab = tab;
            let directory = self.terminal_directory();
            match tab {
                RightDrawerTab::Terminal => {
                    self.terminal.update(cx, |pane, cx| pane.open(directory, window, cx));
                }
                RightDrawerTab::Ai => {
                    self.terminal.update(cx, |pane, _| pane.hide());
                    self.ai.update(cx, |pane, cx| {
                        pane.set_directory(directory, cx);
                        pane.open(window, cx);
                    });
                }
            }
        }
        cx.notify();
    }

    fn focused_attached_file(&self) -> Option<AttachedFile> {
        let key = self.focused_markdown.as_deref().unwrap_or(&self.active);
        let clean_key = key.strip_prefix("reading:").unwrap_or(key);
        if clean_key.is_empty() {
            return None;
        }
        let path = self.document_path(clean_key);
        let display = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| clean_key.to_owned());
        Some(AttachedFile { display, path })
    }

    fn open_ai_with_selection(
        &mut self,
        source: String,
        file_path: Option<PathBuf>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_terminal = true;
        self.right_drawer_tab = RightDrawerTab::Ai;
        self.terminal.update(cx, |pane, _| pane.hide());
        let directory = self.terminal_directory();
        let attached = file_path.map(|path| AttachedFile {
            display: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| source.clone()),
            path,
        });
        self.ai.update(cx, |pane, cx| {
            pane.set_directory(directory, cx);
            pane.attach_selection(source, attached, text, window, cx);
        });
        cx.notify();
    }

    fn open_path_from_ai(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(resolved) = dunce::canonicalize(path) else {
            self.report(format!("找不到文件：{}", path.display()), cx);
            return;
        };
        if let Some(project) = self.directory_workspace()
            && let Ok(canonical_root) = dunce::canonicalize(&project.folder)
            && let Ok(relative) = resolved.strip_prefix(&canonical_root)
            && files::is_relevant(&resolved)
        {
            let key = relative.to_string_lossy().replace('\\', "/");
            let file = ProjectFile {
                kind: Self::file_kind(&resolved),
                path: key,
            };
            if let Err(error) = self.open_file(&file, window, cx) {
                self.report(error, cx);
            }
            return;
        }
        if files::is_relevant(&resolved) {
            match self
                .prepare_standalone_file(&resolved)
                .and_then(|file| self.open_file(&file, window, cx))
            {
                Ok(()) => {}
                Err(error) => self.report(error, cx),
            }
        }
    }

    fn new_pdf_pane(
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

    // Save only navigation state; documents themselves remain authoritative on disk.
    fn remember_session(&mut self) -> io::Result<()> {
        let Some(key) = self.active_session.clone() else {
            return Ok(());
        };
        self.workspace_sessions.insert(
            key,
            WorkspaceSession {
                tabs: self.tabs.clone(),
                active: self.active.clone(),
                reading_panes: self.reading_panes.clone(),
            },
        );
        storage::save_workspace_sessions(&self.workspace_sessions)
    }

    fn document_path(&self, key: &str) -> PathBuf {
        let key = key.strip_prefix("reading:").unwrap_or(key);
        self.directory_workspace()
            .map(|project| PathBuf::from(&project.folder).join(key))
            .unwrap_or_else(|| PathBuf::from(key))
    }

    fn clear_open_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_build_pdf(window, cx);
        self.release_all_pdf_panes(window, cx);
        self.release_all_html_panes(window, cx);
        self.documents.clear();
        self.tabs.clear();
        self.active.clear();
        self.reading_panes.clear();
        self.markdown_panes.clear();
        self.markdown_export = None;
        self.focused_markdown = None;
        self.markdown_changed.clear();
        self.show_build_preview = false;
        self.preview = None;
    }

    fn select_loose_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.show_trash = false;
        self.remember_session()?;
        self.watcher = None;
        self.watch_events = None;
        self.watch_paths.clear();
        self.watched_directories.clear();
        self.watch_changed = None;
        self.watch_tree = false;
        self.config.active_id = None;
        self.config.active_workspace = Some(WorkspaceLocation::LooseFiles);
        self.active_session = Some("loose".into());
        self.ensure_default_watcher()?;
        self.files.clear();
        self.folders.clear();
        self.collapsed_folders.clear();
        self.focused_folder = None;
        self.selected_files.clear();
        self.logs.clear();
        self.build_status = BuildStatus::Idle;
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
        let active = if self.tabs.contains(&session.active) {
            session.active
        } else {
            self.tabs.last().cloned().unwrap_or_default()
        };
        for path in &session.reading_panes {
            self.open_reading_key(path, window, cx)?;
        }
        self.active = active;
        storage::save_projects(&self.config)?;
        self.remember_session()?;
        cx.notify();
        Ok(())
    }
    fn report(&mut self, error: impl std::fmt::Display, cx: &mut Context<Self>) {
        self.notice = Some(error.to_string());
        self.notice_until = Some(Instant::now() + Duration::from_millis(2000));
        cx.notify();
    }
    fn export_pdf(&mut self, source: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title("导出 PDF")
            .set_file_name(
                source
                    .file_name()
                    .expect("PDF has filename")
                    .to_string_lossy(),
            )
            .add_filter("PDF", &["pdf"]);
        self.dialog_open = true;
        let app_entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let result = if let Some(destination) = dialog.save_file().await {
                    std::fs::copy(&source, destination.path()).map(|_| ())
                } else {
                    Ok(())
                };
                let _ = app_entity.update(cx, |app, cx| {
                    app.dialog_open = false;
                    if let Err(error) = result {
                        app.report(error, cx);
                    }
                });
            })
            .detach();
    }
    fn select_project(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.show_trash = false;
        self.remember_session()?;
        self.watcher = None;
        self.watch_events = None;
        self.active_session = Some(id.to_owned());
        if id == "__folio_default" {
            std::fs::create_dir_all(&self.default_workspace.folder)?;
            self.config.active_id = None;
            self.config.active_workspace = Some(WorkspaceLocation::Default);
        } else {
            self.config.active_id = Some(id.to_owned());
            self.config.active_workspace = Some(WorkspaceLocation::Project(id.to_owned()));
        }
        let project = self.directory_workspace().expect("selected project exists").clone();
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .map_err(io::Error::other)?;
        watcher
            .watch(Path::new(&project.folder), RecursiveMode::Recursive)
            .map_err(io::Error::other)?;
        self.watcher = Some(watcher);
        self.watch_events = Some(receiver);
        self.watch_changed = None;
        self.watch_paths.clear();
        self.watched_directories.clear();
        self.watch_tree = false;
        self.files = files::list_files(Path::new(&project.folder))?;
        self.folders = files::list_folders(Path::new(&project.folder)).unwrap_or_default();
        self.logs.clear();
        self.build_status = BuildStatus::Idle;
        self.notice = None;
        self.preview = None;
        self.clear_open_documents(window, cx);
        self.collapsed_folders.clear();
        self.focused_folder = None;
        self.selected_files.clear();
        let previous_pdf = storage::data_directory()?
            .join("builds")
            .join(&project.id)
            .join("preview.pdf");
        if previous_pdf.exists() {
            self.build_pdf = Some(self.new_pdf_pane(previous_pdf.clone(), window, cx));
            self.preview = Some(previous_pdf);
        }
        let session = self.workspace_sessions.get(id).cloned().unwrap_or_default();
        for path in &session.tabs {
            if let Some(file) = self.files.iter().find(|file| &file.path == path).cloned() {
                self.open_file(&file, window, cx)?;
            }
        }
        if self.tabs.is_empty() {
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
        if self.tabs.contains(&session.active) {
            self.active = session.active;
        }
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
    fn switch_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let retry_id = id.to_owned();
        if self.cancellation.is_some()
            || self.batch_running
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
    fn switch_to_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config.active_workspace == Some(WorkspaceLocation::Default) {
            return;
        }
        if self.cancellation.is_some()
            || self.batch_running
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
    fn add_folder(&mut self, folder: &Path, window: &mut Window, cx: &mut Context<Self>) {
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
    fn add_project(&mut self, _: &OpenProject, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancellation.is_some()
            || self.batch_running
            || !self.confirm_drafts(window, cx, |app, window, cx| {
                app.add_project(&OpenProject, window, cx)
            })
        {
            return;
        }
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title("选择本地项目文件夹");
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

    fn open_file_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title("打开文档")
            .add_filter(
                "Folio 支持的文件",
                &["pdf", "md", "tex", "ltx", "bib", "sty", "cls", "html", "htm"],
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

    fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
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
            self.report("请一次打开一个项目文件夹，或选择多个文档。", cx);
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
                    format!("此文件类型不能在 Folio 中打开：{}", path.display()),
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
    }

    fn open_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.open_paths(vec![path.to_path_buf()], window, cx);
        Ok(())
    }

    fn ensure_default_watcher(&mut self) -> io::Result<()> {
        if self.watcher.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .map_err(io::Error::other)?;
        self.watcher = Some(watcher);
        self.watch_events = Some(receiver);
        Ok(())
    }

    fn prepare_standalone_file(&mut self, path: &Path) -> io::Result<ProjectFile> {
        let full = dunce::canonicalize(path)?;
        if !full.is_file() || !files::is_relevant(&full) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "请选择受支持的本地文件。",
            ));
        }
        self.ensure_default_watcher()?;
        let parent = full
            .parent()
            .expect("canonical file has a parent")
            .to_path_buf();
        if self.watched_directories.insert(parent.clone()) {
            self.watcher
                .as_mut()
                .expect("default space has a watcher")
                .watch(&parent, RecursiveMode::NonRecursive)
                .map_err(io::Error::other)?;
        }
        let key = full.to_string_lossy().into_owned();
        Ok(ProjectFile {
            kind: Self::file_kind(&full),
            path: key,
        })
    }

    fn open_standalone_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let file = self.prepare_standalone_file(path)?;
        self.open_file(&file, window, cx)
    }

    fn supports_split_pane(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        lower.ends_with(".md") || lower.ends_with(".pdf") || Self::is_html_path(&lower)
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
            let reading_key = format!("reading:{}", file.path);
            if let Some(pane) = self.markdown_panes.get(&reading_key).cloned() {
                pane.update(cx, |pane, cx| pane.focus(window, cx));
            }
            self.jump_markdown_anchor(&reading_key, anchor, window, cx);
            return Ok(());
        }
        self.open_file(file, window, cx)?;
        self.jump_markdown_anchor(&file.path, anchor, window, cx);
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
            self.report(format!("找不到链接目标：{}", candidate.display()), cx);
            return;
        };
        if resolved.is_dir() {
            match url::Url::from_directory_path(&resolved) {
                Ok(dir_url) => cx.open_url(dir_url.as_str()),
                Err(()) => self.report(format!("无法打开目录：{}", resolved.display()), cx),
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
                Ok(()) => {}
                Err(error) => self.report(error, cx),
            }
            return;
        }
        match url::Url::from_file_path(&resolved) {
            Ok(file_url) => cx.open_url(file_url.as_str()),
            Err(()) => self.report(format!("无法打开文件：{}", resolved.display()), cx),
        }
    }

    fn jump_markdown_anchor(
        &mut self,
        key: &str,
        anchor: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = anchor.filter(|a| !a.is_empty()) else {
            return;
        };
        let target_path = key.strip_prefix("reading:").unwrap_or(key);
        if !target_path.to_ascii_lowercase().ends_with(".md") {
            return;
        }
        if let Some(pane) = self.markdown_panes.get(key).cloned() {
            pane.update(cx, |pane, cx| pane.navigate_to_anchor(anchor, window, cx));
        }
    }

    fn open_reading_key(
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
                app.focused_markdown = None;
            }
            if matches!(event, InputEvent::Change) {
                let document = app
                    .documents
                    .get_mut(&path)
                    .expect("subscription owned by document");
                document.buffer.set_text(state.read(cx).value().to_string());
                document.last_edit = Instant::now();
                app.markdown_changed.insert(path.clone());
                app.source_version += 1;
                cx.notify();
            }
        })
    }
    fn open_file(
        &mut self,
        file: &ProjectFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.open_file_with_focus(file, true, window, cx)
    }

    fn open_file_with_focus(
        &mut self,
        file: &ProjectFile,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.focused_markdown = None;
        let full = self.document_path(&file.path);
        if file.path.to_lowercase().ends_with(".pdf") && !self.pdf_panes.contains_key(&file.path) {
            self.pdf_panes.insert(
                file.path.clone(),
                self.new_pdf_pane(full.clone(), window, cx),
            );
        }
        if Self::is_html_path(&file.path) && !self.html_panes.contains_key(&file.path) {
            let root_dir = self.html_root_dir(&full);
            self.add_html_pane(file.path.clone(), full.clone(), root_dir, false, window, cx);
        }
        if file.kind == FileKind::Text && !self.documents.contains_key(&file.path) {
            let disk = if self.directory_workspace().is_some() {
                files::read_document(
                    Path::new(&self.directory_workspace().expect("project was checked").folder),
                    &file.path,
                )?
            } else {
                files::read_document_at(&full)?
            };
            let extension = Path::new(&file.path)
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
                    .default_value(if file.path.to_lowercase().ends_with(".md") {
                        String::new()
                    } else {
                        disk.text.clone()
                    })
            });
            let subscription = self.subscribe_editor(file.path.clone(), &editor, cx);
            self.documents.insert(
                file.path.clone(),
                OpenDocument {
                    path: full.clone(),
                    buffer: DocumentBuffer::from_disk(disk),
                    editor,
                    last_edit: Instant::now(),
                    _subscription: subscription,
                },
            );
        }
        if !self.tabs.contains(&file.path) {
            self.tabs.push(file.path.clone());
        }
        if file.path.to_lowercase().ends_with(".md")
            && !self.markdown_panes.contains_key(&file.path)
        {
            self.add_markdown_pane(&file.path, false, window, cx);
        }
        self.active = file.path.clone();
        if focus_editor {
            if let Some(pane) = self.markdown_panes.get(&file.path) {
                pane.update(cx, |pane, cx| pane.focus(window, cx));
            } else if let Some(document) = self.documents.get(&file.path) {
                document
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
            }
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
        Ok(())
    }

    fn add_markdown_pane(
        &mut self,
        path: &str,
        reading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.documents[path].buffer.text.clone();
        let full = self.documents[path].path.clone();
        let settings = self.settings.clone();
        let math = self.math.clone();
        let pane = cx.new(|cx| MarkdownPane::new(text, full, settings, math, reading, window, cx));
        cx.subscribe_in(&pane, window, move |app, pane, event: &MarkdownEvent, window, cx| {
            // Resolve identity from the live pane; a rename keeps this subscription alive.
            let full = pane.read(cx).path().to_path_buf();
            let file = app.documents.iter().find(|(_, doc)| doc.path == full)
                .expect("Markdown pane owns open document").0.clone();
            match event {
                MarkdownEvent::RenameTitle(title) => {
                    let name = format!("{title}.md");
                    if title.is_empty() {
                        app.report("标题不能为空。", cx);
                    } else if let Err(error) = app.rename_markdown(&file, &name, window, cx) {
                        app.report(error, cx);
                    } else {
                        pane.update(cx, |pane, cx| pane.focus(window, cx));
                    }
                }
                MarkdownEvent::Change(text) => {
                    let document = app
                        .documents
                        .get_mut(&file)
                        .expect("Markdown pane owns open document");
                    if document.buffer.text == *text {
                        return;
                    }
                    document.buffer.set_text(text.clone());
                    document.last_edit = Instant::now();
                    app.markdown_changed.insert(file.clone());
                    app.source_version += 1;
                }
                MarkdownEvent::Focus => app.focused_markdown = reading.then(|| file.clone()),
                MarkdownEvent::Zoom(size) => {
                    app.settings.markdown_font_size = *size;
                    if let Err(error) = storage::save_settings(&app.settings) {
                        app.report(error, cx);
                    }
                    app.markdown_changed.extend(app.documents.keys().cloned());
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
            }
            cx.notify();
        })
        .detach();
        self.markdown_panes.insert(
            if reading {
                format!("reading:{path}")
            } else {
                path.to_owned()
            },
            pane,
        );
    }
    fn save_file(
        &mut self,
        path: &str,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) -> io::Result<bool> {
        let root = self.directory_workspace().map(|project| PathBuf::from(&project.folder));
        let document = self
            .documents
            .get_mut(path)
            .expect("save only opened text files");
        if document.buffer.deleted {
            return Err(io::Error::other(
                "文件已被删除，请在原位置恢复文件后保存草稿。",
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
                self.active = path.to_owned();
                cx.notify();
                Ok(false)
            }
        }
    }
    fn save_active(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let path = self
            .focused_markdown
            .clone()
            .unwrap_or_else(|| self.active.clone());
        if self.documents.contains_key(&path) {
            if let Err(error) = self.save_file(&path, false, cx) {
                self.report(error, cx);
            }
        }
    }
    fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        let paths: Vec<_> = self
            .documents
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
    fn ask(
        &mut self,
        kind: Dialog,
        title: String,
        description: String,
        buttons: DialogButtons,
        _window: &mut Window,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Option<rfd::MessageDialogResult> {
        if self
            .dialog_response
            .as_ref()
            .is_some_and(|(pending, _)| *pending == kind)
        {
            return Some(
                self.dialog_response
                    .take()
                    .expect("matched dialog response")
                    .1,
            );
        }
        self.dialog_open = true;
        self.dialog_prompt = Some(DialogPrompt {
            kind,
            title,
            description,
            buttons,
        });
        self.dialog_retry = Some(Box::new(retry));
        cx.notify();
        None
    }

    fn answer_dialog(
        &mut self,
        result: rfd::MessageDialogResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = self.dialog_prompt.take().expect("visible prompt exists");
        self.dialog_open = false;
        self.dialog_response = Some((prompt.kind, result));
        let retry = self
            .dialog_retry
            .take()
            .expect("visible prompt has retry action");
        retry(self, window, cx);
        cx.notify();
    }
    fn confirm_drafts(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        if !self
            .documents
            .values()
            .any(|document| document.buffer.is_dirty())
            && !self
                .dialog_response
                .as_ref()
                .is_some_and(|(kind, _)| *kind == Dialog::Drafts)
        {
            return true;
        }
        match self.ask(
            Dialog::Drafts,
            "处理未保存的修改".into(),
            "保存全部修改、放弃修改，或返回继续编辑。".into(),
            DialogButtons::SaveDiscardCancel,
            window,
            cx,
            retry,
        ) {
            Some(rfd::MessageDialogResult::Yes) => self.save_all(cx),
            Some(rfd::MessageDialogResult::No) => true,
            _ => false,
        }
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.confirm_drafts(window, cx, |app, window, cx| {
            if app.request_close(window, cx) {
                window.remove_window();
            }
        }) {
            return false;
        }
        if self.cancellation.is_some() {
            // Let the build worker terminate latexmk's child process tree before exit.
            self.cancel_build();
            self.close_requested = true;
            return false;
        }
        self.release_all_html_panes(window, cx);
        true
    }

    fn close_tab(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.reading_panes.iter().any(|pane| pane == path)
            && (self
                .documents
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
                format!("关闭 {path} 前保存修改？"),
                "保存文件、放弃修改，或返回继续编辑。".into(),
                DialogButtons::SaveDiscardCancel,
                window,
                cx,
                move |app, window, cx| {
                    if app.reading_panes.contains(&retry_path) && !app.tabs.contains(&retry_path) {
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
        self.tabs.retain(|tab| tab != path);
        self.release_pdf_pane(path, window, cx);
        self.release_html_pane(path, window, cx);
        self.markdown_panes.remove(path);
        if !self.reading_panes.iter().any(|pane| pane == path) {
            self.documents.remove(path);
        }
        if self.focused_markdown.as_deref() == Some(path) {
            self.focused_markdown = None;
        }
        if self.active == path {
            self.active = self.tabs.last().cloned().unwrap_or_default();
            if let Some(document) = self.documents.get(&self.active) {
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
    fn reload_disk(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let disk = if let Some(project) = self.directory_workspace() {
            files::read_document(Path::new(&project.folder), path)
        } else {
            files::read_document_at(&self.documents[path].path)
        };
        match disk {
            Ok(disk) => {
                let document = self
                    .documents
                    .get_mut(path)
                    .expect("reload only open documents");
                document.editor.update(cx, |editor, cx| {
                    editor.set_value(disk.text.clone(), window, cx)
                });
                document.buffer.replace_from_disk(disk);
                self.markdown_changed.insert(path.to_owned());
                cx.notify();
            }
            Err(error) => self.report(error, cx),
        }
    }
    fn queue_math_preload(&self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
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
    fn refresh_files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let root = self.directory_workspace().map(|project| PathBuf::from(&project.folder));
        let previous: BTreeSet<_> = self.files.iter().map(|file| file.path.clone()).collect();
        if let Some(root) = &root {
            if self.watch_paths.is_empty() || self.watch_tree {
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
        let changed = std::mem::take(&mut self.watch_paths);
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
        self.watch_tree = false;
        for (path, document) in &mut self.documents {
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
                        self.markdown_changed.insert(path.clone());
                        self.source_version += 1;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    document.buffer.deleted = true
                }
                Err(error) => return Err(error),
            }
        }
        let pdf_keys: Vec<_> = self
            .pdf_panes
            .keys()
            .filter(|key| {
                let file = key.strip_prefix("reading:").unwrap_or(key);
                let path = root
                    .as_ref()
                    .map(|root| root.join(file))
                    .unwrap_or_else(|| PathBuf::from(file));
                changed.contains(&path)
            })
            .cloned()
            .collect();
        for key in pdf_keys {
            let file = key.strip_prefix("reading:").unwrap_or(&key);
            let full = root
                .as_ref()
                .map(|root| root.join(file))
                .unwrap_or_else(|| PathBuf::from(file));
            self.pdf_panes
                .insert(key, self.new_pdf_pane(full, window, cx));
        }
        if !changed.is_empty() {
            for pane in self.html_panes.values() {
                let file_path = pane.read(cx).file_path().to_path_buf();
                if changed.contains(&file_path) {
                    pane.update(cx, |pane, cx| pane.reload(window, cx));
                }
            }
        }
        cx.notify();
        Ok(())
    }
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Instant::now() >= self.next_trash_cleanup {
            self.next_trash_cleanup = Instant::now() + Duration::from_secs(3600);
            if let Err(error) = crate::trash::cleanup().and_then(|_| crate::trash::list()).map(|entries| self.trash_entries = entries) {
                self.report(error, cx);
            }
            cx.notify();
        }
        if self
            .notice_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.notice = None;
            self.notice_until = None;
            cx.notify();
        }
        for path in std::mem::take(&mut self.markdown_changed) {
            if let Some(document) = self.documents.get(&path) {
                for key in [&path, &format!("reading:{path}")] {
                    if let Some(pane) = self.markdown_panes.get(key) {
                        pane.update(cx, |pane, cx| {
                            pane.sync(&document.buffer.text, &self.settings, window, cx)
                        });
                    }
                }
            }
        }
        let events: Vec<_> = self
            .watch_events
            .iter()
            .flat_map(|receiver| receiver.try_iter())
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
                                (directory_event || path.is_dir() || files::is_relevant(path))
                                    && path.strip_prefix(root).is_ok_and(|relative| {
                                        !relative.components().any(|part| {
                                            files::EXCLUDED_DIRECTORIES
                                                .iter()
                                                .any(|name| part.as_os_str() == *name)
                                        })
                                    })
                            } else {
                                files::is_relevant(path)
                                    && (self
                                        .documents
                                        .values()
                                        .any(|document| document.path == *path)
                                        || self.pdf_panes.keys().any(|key| {
                                            let file = key.strip_prefix("reading:").unwrap_or(key);
                                            PathBuf::from(file) == *path
                                        })
                                        || self.html_panes.values().any(|pane| {
                                            pane.read(cx).file_path() == *path
                                        }))
                            }
                        })
                        .collect();
                    if !paths.is_empty() {
                        self.watch_paths.extend(paths);
                        self.watch_tree |= tree;
                        self.watch_changed = Some(Instant::now());
                    }
                }
                Ok(_) => {}
                Err(error) => self.report(error, cx),
            }
        }
        if self
            .watch_changed
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(200))
        {
            self.watch_changed = None;
            if let Err(error) = self.refresh_files(window, cx) {
                self.report(error, cx);
            }
        }
        // Autosave only idle, conflict-free drafts. Manual saves and builds share this revision path.
        if self.name_action.is_none() && !self.dialog_open {
            let paths: Vec<_> = self
                .documents
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
                    self.documents
                        .get_mut(&path)
                        .expect("autosave document exists")
                        .last_edit = Instant::now();
                    self.report(error, cx);
                }
            }
        }
        // Reuse the workspace tick for elapsed time, including quiet compiler phases.
        // Idle workspaces do not redraw for the build indicator.
        if let Some(started) = self.build_started {
            self.build_duration = started.elapsed();
            cx.notify();
        }
        let events: Vec<_> = self
            .build_events
            .iter()
            .flat_map(|receiver| receiver.try_iter())
            .collect();
        for event in events {
            match event {
                BuildEvent::Log(text) => {
                    self.logs.push_str(&text);
                    cx.notify();
                }
                BuildEvent::Finished(result) => {
                    self.markdown_export = None;
                    self.build_duration = self
                        .build_started
                        .take()
                        .expect("finished build has a start time")
                        .elapsed();
                    self.cancellation = None;
                    self.build_events = None;
                    if self.close_requested {
                        self.release_all_html_panes(window, cx);
                        window.remove_window();
                        cx.quit();
                        return;
                    }
                    match result {
                        Ok(result) => {
                            self.build_status = result.status;
                            self.build_duration = self.build_duration.max(result.elapsed);
                            self.logs = result.log;
                            if let Some(path) = result.preview {
                                self.release_build_pdf(window, cx);
                                self.build_pdf =
                                    Some(self.new_pdf_pane(path.clone(), window, cx));
                                self.preview = Some(path);
                            }
                            self.show_logs = result.status == BuildStatus::Failed;
                        }
                        Err(error) => {
                            self.build_status = BuildStatus::Failed;
                            self.logs.push_str(&format!("\n{error}\n"));
                            self.report(error, cx);
                            self.show_logs = true;
                        }
                    }
                    if self.batch_running {
                        self.batch_logs.push_str(&format!(
                            "\n===== {} =====\n{}\n",
                            self.build_target, self.logs
                        ));
                        self.batch_failed |= self.build_status == BuildStatus::Failed;
                        if self.build_status != BuildStatus::Cancelled
                            && !self.batch_queue.is_empty()
                        {
                            self.prepare_batch_next(window, cx);
                        } else {
                            self.batch_running = false;
                            self.logs = std::mem::take(&mut self.batch_logs);
                            if self.batch_failed {
                                self.build_status = BuildStatus::Failed;
                                self.show_logs = true;
                            }
                        }
                    }
                    cx.notify();
                }
            }
        }
        if self
            .batch_next
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(250))
        {
            self.batch_next = None;
            self.compile(&Compile, window, cx);
        }
        let html_panes: Vec<_> = self.html_panes.values().cloned().collect();
        for pane in html_panes {
            pane.update(cx, |pane, cx| pane.poll(window, cx));
        }
    }
    fn compile_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancellation.is_some() || self.batch_running {
            return;
        }
        let targets: Vec<_> = self
            .selected_files
            .iter()
            .filter(|path| {
                let lower = path.to_lowercase();
                lower.ends_with(".tex") || lower.ends_with(".md")
            })
            .cloned()
            .collect();
        if targets.is_empty() {
            self.report("请选择要编译的 .tex 或 .md 文件。", cx);
            return;
        }
        let mut outputs = BTreeSet::new();
        if targets.iter().any(|target| {
            !outputs.insert(
                Path::new(target)
                    .with_extension("pdf")
                    .to_string_lossy()
                    .to_lowercase(),
            )
        }) {
            self.report("所选文件会生成同名 PDF，请分开编译。", cx);
            return;
        }
        if !self.save_all(cx) {
            return;
        }
        self.batch_queue = targets.into();
        self.batch_logs.clear();
        self.batch_failed = false;
        self.batch_running = true;
        self.prepare_batch_next(window, cx);
    }

    fn prepare_batch_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.batch_queue.pop_front().unwrap();
        let file = self.files.iter().find(|file| file.path == target).cloned();
        let result = file
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "待编译文件不存在"))
            .and_then(|file| self.open_file(&file, window, cx));
        if let Err(error) = result {
            self.batch_running = false;
            self.batch_queue.clear();
            self.report(error, cx);
            return;
        }
        // Markdown export captures the rendered pane, so let GPUI paint it first.
        self.batch_target = Some(target);
        self.batch_next = Some(Instant::now());
        cx.notify();
    }

    fn cancel_build(&mut self) {
        self.batch_queue.clear();
        self.batch_next = None;
        if let Some(cancelled) = &self.cancellation {
            cancelled.store(true, Ordering::Relaxed);
        }
    }
    fn compile_target(&self) -> Option<&str> {
        let focused = self.focused_markdown.as_deref().unwrap_or(&self.active);
        let lower = focused.to_lowercase();
        if lower.ends_with(".md") || (self.config.active_id.is_none() && lower.ends_with(".tex")) {
            Some(focused)
        } else {
            self.directory_workspace()
                .map(|project| project.main_file.as_str())
                .filter(|file| !file.is_empty())
        }
    }

    fn compile(&mut self, _: &Compile, _: &mut Window, cx: &mut Context<Self>) {
        if self.batch_running && self.batch_next.is_some() {
            self.batch_next = None;
            self.batch_target = None;
            self.batch_queue.clear();
            self.batch_running = false;
            self.build_status = BuildStatus::Cancelled;
            cx.notify();
            return;
        }
        if self.cancellation.is_some() {
            self.cancel_build();
            return;
        }
        // Match the original app: a focused Markdown split overrides the active
        // tab; other files use the project's configured TeX entry point.
        let Some(target) = self
            .batch_target
            .take()
            .or_else(|| self.compile_target().map(str::to_owned))
        else {
            self.report("请选择一个 .tex 文件作为编译主文件。", cx);
            return;
        };
        let mut project = self.directory_workspace().cloned().unwrap_or_else(|| {
            let source = PathBuf::from(&target);
            let root = source.parent().expect("standalone document has a parent");
            // The synthetic compile context only supplies latexmk's working directory;
            // it is never stored in the user's project list.
            let hash = sha2::Sha256::digest(source.as_os_str().to_string_lossy().as_bytes());
            let id = hash
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            Project {
                id: format!("standalone-{id}"),
                name: source
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                folder: root.to_string_lossy().into_owned(),
                main_file: source
                    .file_name()
                    .expect("standalone document has a name")
                    .to_string_lossy()
                    .into_owned(),
                engine: Engine::default(),
            }
        });
        if !self.save_all(cx) {
            self.batch_running = false;
            self.batch_queue.clear();
            return;
        }
        let mut build_root = match storage::data_directory() {
            Ok(path) => path.join("builds"),
            Err(error) => {
                self.batch_running = false;
                self.batch_queue.clear();
                self.report(error, cx);
                return;
            }
        };
        if self.batch_running {
            use sha2::{Digest, Sha256};
            // Different directories can contain TeX sources with the same stem;
            // their latexmk work files must not share an output directory.
            let hash: String = Sha256::digest(target.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            build_root = build_root.join(format!("batch-{hash}"));
        }
        let markdown = target
            .to_lowercase()
            .ends_with(".md")
            .then(|| target.clone());
        if markdown.is_none() && self.directory_workspace().is_some() {
            project.main_file = target.clone();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.build_version = self.source_version;
        self.build_target = target;
        self.build_status = BuildStatus::Idle;
        self.show_build_preview = true;
        self.cancellation = Some(cancelled.clone());
        self.build_started = Some(Instant::now());
        self.build_duration = Duration::ZERO;
        self.logs.clear();
        self.notice = None;
        let (sender, receiver) = mpsc::channel();
        self.build_events = Some(receiver);
        if let Some(file) = markdown {
            let key = if self.markdown_panes.contains_key(&format!("reading:{file}")) {
                format!("reading:{file}")
            } else {
                file.clone()
            };
            let pane = self.markdown_panes[&key].clone();
            self.markdown_export = Some(pane.clone());
            let request = crate::markdown_pdf::ExportRequest {
                destination: PathBuf::from(&project.folder)
                    .join(&file)
                    .with_extension("pdf"),
                preview: build_root.join(&project.id).join("preview.pdf"),
                cancelled,
                complete: Box::new(move |result| {
                    let _ = sender.send(BuildEvent::Finished(result));
                }),
                started: Instant::now(),
            };
            pane.update(cx, |pane, cx| pane.export_pdf(request, cx));
        } else {
            thread::spawn(move || {
                let log_sender = sender.clone();
                let on_log = move |line: &str| {
                    let _ = log_sender.send(BuildEvent::Log(line.to_owned()));
                };
                let result = compiler::compile_tex(&project, &build_root, cancelled, on_log);
                let _ = sender.send(BuildEvent::Finished(result));
            });
        }
        cx.notify();
    }
    fn start_name(
        &mut self,
        action: NameAction,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.name_action = Some(action);
        self.name_input.update(cx, |input, cx| {
            input.set_value(value.to_owned(), window, cx);
        });
        // Let the menu and its pointer event finish before the dialog takes focus.
        let input = self.name_input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }
    fn rename_markdown(&mut self, key: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let old = self.document_path(key);
        if old.file_name().unwrap() == name { return Ok(()); }
        let parent = old.parent().unwrap();
        files::rename_file(parent, &old.file_name().unwrap().to_string_lossy(), name)?;
        let full = parent.join(name);
        let renamed = if let Some(workspace) = self.directory_workspace() {
            full.strip_prefix(&workspace.folder).expect("same workspace").to_string_lossy().replace('\\', "/")
        } else { full.to_string_lossy().into_owned() };
        self.relocate_file_state(key, &renamed, window, cx);
        if let Some(workspace) = self.directory_workspace() {
            self.files = files::list_files(Path::new(&workspace.folder))?;
        }
        self.remember_session()?;
        cx.notify();
        // Annotation storage is keyed by filename, independently of editor state.
        let mut annotations = storage::load_annotations(&old);
        for item in &mut annotations { item.file = name.to_owned(); }
        if !annotations.is_empty() {
            storage::save_annotations(&old, &[])?;
            storage::save_annotations(&full, &annotations)?;
        }
        Ok(())
    }

    // Keep live buffers and Markdown panes: moving a file must not reload unsaved edits.
    fn relocate_file_state(&mut self, key: &str, renamed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let full = self.document_path(renamed);
        let renamed = renamed.to_owned();
        if let Some(mut document) = self.documents.remove(key) {
            document.path = full.clone();
            document._subscription = self.subscribe_editor(renamed.clone(), &document.editor, cx);
            self.documents.insert(renamed.clone(), document);
        }
        for reading in [false, true] {
            let old_key = if reading { format!("reading:{key}") } else { key.to_owned() };
            let new_key = if reading { format!("reading:{renamed}") } else { renamed.clone() };
            if let Some(pane) = self.markdown_panes.remove(&old_key) {
                pane.update(cx, |pane, cx| pane.set_path(full.clone(), window, cx));
                self.markdown_panes.insert(new_key, pane);
            }
        }
        for path in self.tabs.iter_mut().chain(self.reading_panes.iter_mut()) {
            if path == key { *path = renamed.clone(); }
        }
        if self.active == key { self.active = renamed.clone(); }
        if self.focused_markdown.as_deref() == Some(key) { self.focused_markdown = Some(renamed.clone()); }
        for paths in [&mut self.selected_files, &mut self.markdown_changed] {
            if paths.remove(key) { paths.insert(renamed.clone()); }
        }
        for reading in [false, true] {
            let old_key = if reading { format!("reading:{key}") } else { key.to_owned() };
            let new_key = if reading { format!("reading:{renamed}") } else { renamed.clone() };
            if self.pdf_panes.contains_key(&old_key) {
                self.release_pdf_pane(&old_key, window, cx);
                self.pdf_panes.insert(new_key.clone(), self.new_pdf_pane(full.clone(), window, cx));
            }
            if self.html_panes.contains_key(&old_key) {
                self.release_html_pane(&old_key, window, cx);
                let root = PathBuf::from(&self.directory_workspace().expect("moved HTML has project").folder);
                self.add_html_pane(new_key, full.clone(), root, reading, window, cx);
            }
        }
        if self.default_workspace.main_file == key && self.directory_workspace().is_some_and(|p| p.id == self.default_workspace.id) {
            self.default_workspace.main_file = renamed.clone();
        }
        if let Some(project) = self.config.projects.iter_mut().find(|p| Some(&p.id) == self.config.active_id.as_ref()) {
            if project.main_file == key { project.main_file = renamed; }
        }
    }

    fn move_tree_files(&mut self, dragged: &crate::terminal::DraggedPaths, folder: &str, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let project = self.directory_workspace().ok_or_else(|| io::Error::other("请选择来源项目。"))?;
        if dragged.1.as_deref() != Some(project.id.as_str()) {
            return Err(io::Error::other("仅支持移动同项目文件，独立文件只能复制路径。"));
        }
        if self.cancellation.is_some() || self.batch_running {
            return Err(io::Error::other("请等待编译结束后移动文件。"));
        }
        let root = PathBuf::from(&project.folder);
        // The empty relative directory denotes the project root, never an absolute file key.
        let destination = files::project_path(&root, if folder.is_empty() { "." } else { folder })?;
        let mut moves = Vec::new();
        let mut targets = BTreeSet::new();
        // Preflight the whole selection before touching disk; existing targets are never replaced.
        for source in &dragged.0 {
            let relative = source.strip_prefix(&root).map_err(|_| io::Error::other("来源文件不属于当前项目。"))?.to_string_lossy().replace('\\', "/");
            let source = files::project_path(&root, &relative)?;
            let target = destination.join(source.file_name().expect("dragged file has name"));
            if source == target { continue; }
            if target.exists() || !targets.insert(target.to_string_lossy().to_lowercase()) {
                return Err(io::Error::other(format!("目标存在同名文件，未移动：{}", target.display())));
            }
            let name = source.file_name().unwrap().to_string_lossy();
            let renamed = if folder.is_empty() { name.into_owned() } else { format!("{folder}/{name}") };
            moves.push((source, target, relative, renamed));
        }
        let mut result = Ok(());
        for (source, target, relative, renamed) in moves {
            // Match rename's no-overwrite hard-link operation. On failure, retain earlier successes.
            result = std::fs::hard_link(&source, &target).and_then(|_| std::fs::remove_file(&source));
            if result.is_err() { break; }
            self.relocate_file_state(&relative, &renamed, window, cx);
            self.expand_ancestors(&renamed);
            // Annotations live in the source directory's .folio store, not inside the file.
            let annotations = storage::load_annotations(&source);
            if !annotations.is_empty() {
                result = storage::save_annotations(&target, &annotations)
                    .and_then(|_| storage::save_annotations(&source, &[]));
                if result.is_err() { break; }
            }
        }
        self.files = files::list_files(&root)?;
        self.folders = files::list_folders(&root)?;
        storage::save_projects(&self.config)?;
        self.remember_session()?;
        cx.notify();
        result
    }

    fn focused_folder_prefix(&self) -> String {
        self.focused_folder
            .as_deref()
            .filter(|folder| !folder.is_empty())
            .map_or_else(String::new, |folder| format!("{folder}/"))
    }

    fn expand_ancestors(&mut self, relative: &str) {
        let mut prefix = String::new();
        let parts: Vec<_> = relative.split('/').collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            self.collapsed_folders.remove(&prefix);
        }
    }

    fn new_markdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let folder = self.focused_folder.clone();
        self.new_markdown_in(folder.as_deref(), window, cx)
    }

    fn new_default_markdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        if self.config.active_workspace != Some(WorkspaceLocation::Default) {
            if self.cancellation.is_some() || self.batch_running { return Ok(()); }
            if !self.confirm_drafts(window, cx, |app, window, cx| {
                if let Err(error) = app.new_default_markdown(window, cx) { app.report(error, cx); }
            }) { return Ok(()); }
            self.select_project("__folio_default", window, cx)?;
        }
        self.show_trash = false;
        self.new_markdown_in(None, window, cx)
    }

    fn new_markdown_in(
        &mut self,
        folder: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.directory_workspace().is_none() {
            if self.cancellation.is_some() || self.batch_running {
                return Ok(());
            }
            let target_folder = folder.map(ToOwned::to_owned);
            if !self.confirm_drafts(window, cx, move |app, window, cx| {
                if let Err(error) = app.new_markdown_in(target_folder.as_deref(), window, cx) {
                    app.report(error, cx);
                }
            }) {
                return Ok(());
            }
            self.select_project("__folio_default", window, cx)?;
        }
        let root = PathBuf::from(&self.directory_workspace().unwrap().folder);
        let file = files::create_markdown_in(&root, folder)?;
        self.files = files::list_files(&root)?;
        self.expand_ancestors(&file.path);
        self.open_file(&file, window, cx)
    }

    fn apply_name(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let name = self.name_input.read(cx).value().trim().to_owned();
        if let Some(NameAction::RenameFile(path)) = self.name_action.clone()
            && path.to_lowercase().ends_with(".md") && name.to_lowercase().ends_with(".md") {
            self.rename_markdown(&path, &name, window, cx)?;
            self.name_action = None;
            return Ok(());
        }
        match self.name_action.clone().expect("modal has action") {
            NameAction::Create => {
                let root = PathBuf::from(&self.directory_workspace().expect("file creation has a directory").folder);
                files::create_file(&root, &name)?;
                self.files = files::list_files(&root)?;
                self.folders = files::list_folders(&root).unwrap_or_default();
                let normalized = name.trim().replace('\\', "/");
                let file = self
                    .files
                    .iter()
                    .find(|file| file.path == normalized)
                    .expect("new supported file appears in scan")
                    .clone();
                self.expand_ancestors(&file.path);
                self.open_file(&file, window, cx)?;
            }
            NameAction::CreateFolder => {
                let root = PathBuf::from(&self.directory_workspace().expect("folder creation has a directory").folder);
                files::create_folder(&root, &name)?;
                self.files = files::list_files(&root)?;
                self.folders = files::list_folders(&root).unwrap_or_default();
                let created = name
                    .trim()
                    .trim_matches(['/', '\\'])
                    .replace('\\', "/");
                self.expand_ancestors(&created);
            }
            NameAction::RenameFile(path) => {
                let root = PathBuf::from(&self.directory_workspace().expect("file tree has a directory").folder);
                let renamed = files::rename_file(&root, &path, &name)?;
                if let Some(mut document) = self.documents.remove(&path) {
                    document.path = root.join(&renamed);
                    // Reconnect the subscription because it identifies the owning document by path.
                    document._subscription =
                        self.subscribe_editor(renamed.clone(), &document.editor, cx);
                    self.documents.insert(renamed.clone(), document);
                }
                for tab in &mut self.tabs {
                    if *tab == path {
                        *tab = renamed.clone();
                    }
                }
                if self.active == path {
                    self.active = renamed.clone();
                }
                self.markdown_panes.remove(&path);
                self.markdown_panes.remove(&format!("reading:{path}"));
                self.release_pdf_pane(&path, window, cx);
                self.release_pdf_pane(&format!("reading:{path}"), window, cx);
                self.release_html_pane(&path, window, cx);
                self.release_html_pane(&format!("reading:{path}"), window, cx);
                if self.focused_markdown.as_deref() == Some(&path) {
                    self.focused_markdown = Some(renamed.clone());
                }
                for pane in &mut self.reading_panes {
                    if *pane == path {
                        *pane = renamed.clone();
                    }
                }
                if renamed.to_lowercase().ends_with(".md") {
                    if self.tabs.contains(&renamed) {
                        self.add_markdown_pane(&renamed, false, window, cx);
                    }
                    if self.reading_panes.contains(&renamed) {
                        self.add_markdown_pane(&renamed, true, window, cx);
                    }
                }
                if renamed.to_lowercase().ends_with(".pdf") {
                    if self.tabs.contains(&renamed) {
                        self.pdf_panes.insert(
                            renamed.clone(),
                            self.new_pdf_pane(root.join(&renamed), window, cx),
                        );
                    }
                    if self.reading_panes.contains(&renamed) {
                        self.pdf_panes.insert(
                            format!("reading:{renamed}"),
                            self.new_pdf_pane(root.join(&renamed), window, cx),
                        );
                    }
                }
                if Self::is_html_path(&renamed) {
                    if self.tabs.contains(&renamed) {
                        self.add_html_pane(
                            renamed.clone(),
                            root.join(&renamed),
                            root.clone(),
                            false,
                            window,
                            cx,
                        );
                    }
                    if self.reading_panes.contains(&renamed) {
                        self.add_html_pane(
                            format!("reading:{renamed}"),
                            root.join(&renamed),
                            root.clone(),
                            true,
                            window,
                            cx,
                        );
                    }
                }
                let project = self
                    .config
                    .projects
                    .iter_mut()
                    .find(|project| Some(&project.id) == self.config.active_id.as_ref())
                    ;
                if let Some(project) = project && project.main_file == path {
                    project.main_file = if renamed.to_lowercase().ends_with(".tex") {
                        renamed
                    } else {
                        String::new()
                    };
                }
                self.files = files::list_files(&root)?;
                self.folders = files::list_folders(&root).unwrap_or_default();
                storage::save_projects(&self.config)?;
            }
            NameAction::RenameFolder(folder_path) => {
                let root = PathBuf::from(
                    &self
                        .directory_workspace()
                        .expect("folder tree has a directory")
                        .folder,
                );
                let renamed = files::rename_folder(&root, &folder_path, &name)?;
                if renamed != folder_path {
                    let old_prefix = format!("{folder_path}/");
                    let remap = |value: &str| -> Option<String> {
                        if value == folder_path {
                            Some(renamed.clone())
                        } else {
                            value
                                .strip_prefix(&old_prefix)
                                .map(|rest| format!("{renamed}/{rest}"))
                        }
                    };
                    let doc_keys: Vec<String> = self
                        .documents
                        .keys()
                        .filter(|key| key.starts_with(&old_prefix))
                        .cloned()
                        .collect();
                    for old_key in doc_keys {
                        if let Some(mut document) = self.documents.remove(&old_key) {
                            let new_key = remap(&old_key).expect("matched prefix");
                            document.path = root.join(&new_key);
                            document._subscription =
                                self.subscribe_editor(new_key.clone(), &document.editor, cx);
                            self.documents.insert(new_key, document);
                        }
                    }
                    for tab in &mut self.tabs {
                        if let Some(new_path) = remap(tab) {
                            *tab = new_path;
                        }
                    }
                    if let Some(new_active) = remap(&self.active) {
                        self.active = new_active;
                    }
                    if let Some(focused) = self.focused_markdown.as_deref().and_then(remap) {
                        self.focused_markdown = Some(focused);
                    }
                    for pane in &mut self.reading_panes {
                        if let Some(new_path) = remap(pane) {
                            *pane = new_path;
                        }
                    }
                    let md_keys: Vec<String> = self
                        .markdown_panes
                        .keys()
                        .filter(|key| {
                            let rel = key.strip_prefix("reading:").unwrap_or(key);
                            rel.starts_with(&old_prefix)
                        })
                        .cloned()
                        .collect();
                    for old_key in md_keys {
                        if let Some(pane) = self.markdown_panes.remove(&old_key) {
                            let reading = old_key.starts_with("reading:");
                            let old_rel = old_key.strip_prefix("reading:").unwrap_or(&old_key);
                            let new_rel = remap(old_rel).expect("matched prefix");
                            let new_key = if reading {
                                format!("reading:{new_rel}")
                            } else {
                                new_rel.clone()
                            };
                            pane.update(cx, |pane, cx| {
                                pane.set_path(root.join(&new_rel), window, cx)
                            });
                            self.markdown_panes.insert(new_key, pane);
                        }
                    }
                    let pdf_keys: Vec<String> = self
                        .pdf_panes
                        .keys()
                        .filter(|key| {
                            let rel = key.strip_prefix("reading:").unwrap_or(key);
                            rel.starts_with(&old_prefix)
                        })
                        .cloned()
                        .collect();
                    for old_key in pdf_keys {
                        self.release_pdf_pane(&old_key, window, cx);
                        let reading = old_key.starts_with("reading:");
                        let old_rel = old_key.strip_prefix("reading:").unwrap_or(&old_key);
                        let new_rel = remap(old_rel).expect("matched prefix");
                        let new_key = if reading {
                            format!("reading:{new_rel}")
                        } else {
                            new_rel.clone()
                        };
                        self.pdf_panes.insert(
                            new_key,
                            self.new_pdf_pane(root.join(&new_rel), window, cx),
                        );
                    }
                    let html_keys: Vec<String> = self
                        .html_panes
                        .keys()
                        .filter(|key| {
                            let rel = key.strip_prefix("reading:").unwrap_or(key);
                            rel.starts_with(&old_prefix)
                        })
                        .cloned()
                        .collect();
                    for old_key in html_keys {
                        self.release_html_pane(&old_key, window, cx);
                        let reading = old_key.starts_with("reading:");
                        let old_rel = old_key.strip_prefix("reading:").unwrap_or(&old_key);
                        let new_rel = remap(old_rel).expect("matched prefix");
                        let new_key = if reading {
                            format!("reading:{new_rel}")
                        } else {
                            new_rel.clone()
                        };
                        self.add_html_pane(
                            new_key,
                            root.join(&new_rel),
                            root.clone(),
                            reading,
                            window,
                            cx,
                        );
                    }
                    for set in [
                        &mut self.selected_files,
                        &mut self.markdown_changed,
                        &mut self.collapsed_folders,
                    ] {
                        let moved: Vec<String> = set
                            .iter()
                            .filter(|item| *item == &folder_path || item.starts_with(&old_prefix))
                            .cloned()
                            .collect();
                        for old_item in moved {
                            set.remove(&old_item);
                            if let Some(new_item) = remap(&old_item) {
                                set.insert(new_item);
                            }
                        }
                    }
                    if let Some(focused) = self.focused_folder.as_deref().and_then(remap) {
                        self.focused_folder = Some(focused);
                    }
                    let project = self
                        .config
                        .projects
                        .iter_mut()
                        .find(|project| Some(&project.id) == self.config.active_id.as_ref());
                    if let Some(project) = project {
                        if let Some(new_main) = remap(&project.main_file) {
                            project.main_file = new_main;
                        }
                    }
                    self.watch_tree = true;
                    self.files = files::list_files(&root)?;
                    self.folders = files::list_folders(&root).unwrap_or_default();
                    storage::save_projects(&self.config)?;
                }
            }
            NameAction::RenameProject(id) => {
                if name.is_empty() {
                    return Err(io::Error::other("项目名称不能为空。"));
                }
                self.config
                    .projects
                    .iter_mut()
                    .find(|project| project.id == id)
                    .expect("menu project exists")
                    .name = name;
                storage::save_projects(&self.config)?;
            }
        }
        self.remember_session()?;
        self.name_action = None;
        cx.notify();
        Ok(())
    }
    fn delete_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_trash(vec![path.to_owned()], window, cx);
    }

    fn delete_folder(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancellation.is_some() {
            return;
        }
        let prefix = format!("{folder}/");
        let paths = self
            .files
            .iter()
            .filter(|file| file.path.starts_with(&prefix))
            .map(|file| file.path.clone())
            .collect();
        self.confirm_trash(paths, window, cx);
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_files.is_empty() || self.cancellation.is_some() {
            return;
        }
        self.confirm_trash(self.selected_files.iter().cloned().collect(), window, cx);
    }

    fn confirm_trash(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() || self.cancellation.is_some() { return; }
        let retry = paths.clone();
        if self.ask(
            Dialog::DeleteSelected,
            format!("将 {} 个文件移入废纸篓？", paths.len()),
            "文件保留 7 天，可在废纸篓恢复。文件夹内仅处理受支持的项目文件。".into(),
            DialogButtons::Confirm("移入废纸篓".into()),
            window,
            cx,
            move |app, window, cx| app.confirm_trash(retry, window, cx),
        ) != Some(rfd::MessageDialogResult::Ok)
        {
            return;
        }
        self.delete_paths(paths, window, cx);
    }

    fn delete_paths(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        for path in &paths {
            if self.documents.get(path).is_some_and(|document| {
                document.buffer.is_dirty() || document.buffer.conflict.is_some()
            }) {
                match self.save_file(path, false, cx) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        self.report(error, cx);
                        return;
                    }
                }
            }
        }
        let project = self.directory_workspace().expect("files have a project").clone();
        let root = Path::new(&project.folder);
        let mut deleted = Vec::new();
        let mut failed = false;
        let batch = uuid::Uuid::new_v4().to_string();
        for path in paths {
            let result = crate::trash::put(&project.id, root, &path, &batch, project.main_file == path);
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    failed = true;
                    self.report(error, cx);
                    break;
                }
            };
            deleted.push(path.clone());
            self.trash_entries.push(entry);
            self.documents.remove(&path);
            self.release_pdf_pane(&path, window, cx);
            self.release_pdf_pane(&format!("reading:{path}"), window, cx);
            self.release_html_pane(&path, window, cx);
            self.release_html_pane(&format!("reading:{path}"), window, cx);
            self.markdown_panes.remove(&path);
            self.markdown_panes.remove(&format!("reading:{path}"));
            self.reading_panes.retain(|pane| pane != &path);
            self.tabs.retain(|tab| tab != &path);
            self.selected_files.remove(&path);
            if self.focused_markdown.as_deref() == Some(&path) {
                self.focused_markdown = None;
            }
        }
        if deleted.is_empty() {
            return;
        }
        if !self.tabs.contains(&self.active) {
            self.active = self.tabs.last().cloned().unwrap_or_default();
        }
        let main = project.main_file.clone();
        if deleted.iter().any(|path| path == &main) {
            if project.id == self.default_workspace.id {
                self.default_workspace.main_file.clear();
            } else if let Some(project) = self.config.projects.iter_mut().find(|p| p.id == project.id) {
                project.main_file.clear();
            }
            if let Err(error) = storage::save_projects(&self.config) {
                self.report(error, cx);
            }
        }
        if let Err(error) = self.refresh_files(window, cx) {
            self.report(error, cx);
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        self.file_tree_focus.focus(window, cx);
        if !failed {
            self.report("已移入废纸篓，按 Ctrl+Z 撤销。", cx);
        }
        cx.notify();
    }

    fn copy_file_path(
        &mut self,
        _: &CopyFilePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_files.is_empty() {
            let paths: Vec<String> = self
                .selected_files
                .iter()
                .map(|path| self.document_path(path).to_string_lossy().into_owned())
                .collect();
            let count = paths.len();
            cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
            if count == 1 {
                self.report("已复制完整路径。", cx);
            } else {
                self.report(format!("已复制 {count} 个文件的完整路径。"), cx);
            }
            return;
        }
        if let Some(folder) = &self.focused_folder
            && let Some(project) = self.directory_workspace()
        {
            let full = PathBuf::from(&project.folder).join(folder);
            cx.write_to_clipboard(ClipboardItem::new_string(
                full.to_string_lossy().into_owned(),
            ));
            self.report("已复制文件夹完整路径。", cx);
            return;
        }
        if !self.active.is_empty() {
            let full = self.document_path(&self.active);
            cx.write_to_clipboard(ClipboardItem::new_string(
                full.to_string_lossy().into_owned(),
            ));
            self.report("已复制完整路径。", cx);
        }
    }

    fn delete_file_tree_item(
        &mut self,
        _: &DeleteFileTreeItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.directory_workspace().is_none() || self.cancellation.is_some() {
            return;
        }
        if !self.selected_files.is_empty() {
            if self.selected_files.len() == 1 {
                if let Some(path) = self.selected_files.iter().next().cloned() {
                    self.delete_file(&path, window, cx);
                }
            } else {
                self.delete_selected(window, cx);
            }
            return;
        }
        if let Some(folder) = self.focused_folder.clone() {
            self.delete_folder(&folder, window, cx);
            return;
        }
        if !self.active.is_empty() && self.files.iter().any(|file| file.path == self.active) {
            let path = self.active.clone();
            self.delete_file(&path, window, cx);
        }
    }

    fn undo_file_delete(
        &mut self,
        _: &UndoFileDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.directory_workspace() else { return; };
        let Some(batch) = self.trash_entries.iter().rev().find(|entry| entry.project_id == project.id).map(|entry| entry.batch.clone()) else { return; };
        let ids = self.trash_entries.iter().filter(|entry| entry.batch == batch).map(|entry| entry.id.clone()).collect();
        self.restore_trash(ids, window, cx);
    }

    fn restore_trash(&mut self, mut ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(id) = ids.first().cloned() {
            let Some(entry) = self.trash_entries.iter().find(|entry| entry.id == id).cloned() else {
                ids.remove(0);
                continue;
            };
            if !entry.project_root.is_dir() {
                self.report("原项目目录不存在，文件仍保留在废纸篓。", cx);
                return;
            }
            let mut target = entry.project_root.join(&entry.relative);
            if target.exists() {
                let retry = ids.clone();
                let Some(choice) = self.ask(Dialog::TrashConflict, "原位置已有同名文件".into(), target.display().to_string(), DialogButtons::RestoreConflict, window, cx, move |app, window, cx| app.restore_trash(retry, window, cx)) else { return; };
                match choice {
                    rfd::MessageDialogResult::Yes => {
                        if self.directory_workspace().is_some_and(|project| project.id == entry.project_id && Path::new(&project.folder) == entry.project_root) {
                            self.delete_paths(vec![entry.relative.clone()], window, cx);
                            if target.exists() { return; }
                        } else {
                            let batch = uuid::Uuid::new_v4().to_string();
                            match crate::trash::put(&entry.project_id, &entry.project_root, &entry.relative, &batch, false) {
                                Ok(replaced) => self.trash_entries.push(replaced),
                                Err(error) => { self.report(error, cx); return; }
                            }
                        }
                    }
                    rfd::MessageDialogResult::No => {
                        let original = target.clone();
                        let stem = original.file_stem().expect("file name").to_string_lossy();
                        let extension = original.extension().map(|value| format!(".{}", value.to_string_lossy())).unwrap_or_default();
                        let mut number = 1;
                        while target.exists() {
                            target = original.with_file_name(format!("{stem} ({number}){extension}"));
                            number += 1;
                        }
                    }
                    _ => { ids.remove(0); continue; }
                }
            }
            let result = std::fs::create_dir_all(target.parent().expect("file parent"))
                .and_then(|_| crate::trash::restore(&entry, &target));
            if let Err(error) = result { self.report(error, cx); return; }
            self.trash_entries.retain(|item| item.id != id);
            if entry.was_main && target == entry.project_root.join(&entry.relative) {
                if entry.project_id == self.default_workspace.id && self.default_workspace.main_file.is_empty() {
                    self.default_workspace.main_file = entry.relative.clone();
                } else if let Some(project) = self.config.projects.iter_mut().find(|project| project.id == entry.project_id && project.main_file.is_empty()) {
                    project.main_file = entry.relative.clone();
                    if let Err(error) = storage::save_projects(&self.config) { self.report(error, cx); }
                }
            }
            ids.remove(0);
        }
        if let Err(error) = self.refresh_files(window, cx) { self.report(error, cx); }
        cx.notify();
    }

    fn purge_trash(&mut self, ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if ids.is_empty() { return; }
        let all = ids.len() > 1 || ids.len() == self.trash_entries.len();
        let retry = ids.clone();
        if self.ask(if all { Dialog::TrashClear } else { Dialog::TrashPurge }, format!("彻底删除 {} 个文件？", ids.len()), "删除后无法从 Folio 废纸篓恢复。".into(), DialogButtons::Confirm("彻底删除".into()), window, cx, move |app, window, cx| app.purge_trash(retry, window, cx)) != Some(rfd::MessageDialogResult::Ok) { return; }
        for id in ids {
            if let Some(entry) = self.trash_entries.iter().find(|entry| entry.id == id).cloned() {
                if let Err(error) = crate::trash::purge(&entry) { self.report(error, cx); return; }
                self.trash_entries.retain(|entry| entry.id != id);
            }
        }
        cx.notify();
    }

    fn set_engine(&mut self, engine: Engine, cx: &mut Context<Self>) {
        if self.cancellation.is_some() {
            return;
        }
        if let Some(project) = self
            .config
            .projects
            .iter_mut()
            .find(|project| Some(&project.id) == self.config.active_id.as_ref())
        {
            project.engine = engine;
            if let Err(error) = storage::save_projects(&self.config) {
                self.report(error, cx);
            }
            cx.notify();
        }
    }

    fn open_reading_pane(
        &mut self,
        file: &ProjectFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading_panes.contains(&file.path) {
            return;
        }
        let active = self.active.clone();
        let already_tab = self.tabs.contains(&file.path);
        if let Err(error) = self.open_file(file, window, cx) {
            self.report(error, cx);
            return;
        }
        if !already_tab {
            self.tabs.retain(|tab| tab != &file.path);
            self.markdown_panes.remove(&file.path);
            self.release_pdf_pane(&file.path, window, cx);
            self.release_html_pane(&file.path, window, cx);
        }
        self.active = active;
        if file.path.to_lowercase().ends_with(".md") {
            self.add_markdown_pane(&file.path, true, window, cx);
            self.markdown_panes[&format!("reading:{}", file.path)]
                .update(cx, |pane, cx| pane.focus(window, cx));
        }
        if file.path.to_lowercase().ends_with(".pdf") {
            let full = self.document_path(&file.path);
            self.pdf_panes.insert(
                format!("reading:{}", file.path),
                self.new_pdf_pane(full, window, cx),
            );
        }
        if Self::is_html_path(&file.path) {
            let full = self.document_path(&file.path);
            let root_dir = self.html_root_dir(&full);
            self.add_html_pane(
                format!("reading:{}", file.path),
                full,
                root_dir,
                true,
                window,
                cx,
            );
        }
        if !self.reading_panes.contains(&file.path) {
            self.reading_panes.push(file.path.clone());
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }

    fn close_reading_pane(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.reading_panes.retain(|pane| pane != path);
        if !self.tabs.iter().any(|tab| tab == path) {
            self.close_tab(path, window, cx);
            // A cancelled draft close keeps its document accessible.
            if self.documents.contains_key(path) {
                self.reading_panes.push(path.to_owned());
                return;
            }
        }
        self.markdown_panes.remove(&format!("reading:{path}"));
        self.release_pdf_pane(&format!("reading:{path}"), window, cx);
        self.release_html_pane(&format!("reading:{path}"), window, cx);
        if self.focused_markdown.as_deref() == Some(path) {
            self.focused_markdown = None;
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        cx.notify();
    }

    fn find_file(&mut self, _: &FindFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.directory_workspace().is_none() {
            return;
        }
        self.settings.sidebar_open = true;
        self.searching = !self.searching;
        if self.searching {
            self.search_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn render_reading_pane(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let close = path.to_owned();
        let is_pdf = path.to_ascii_lowercase().ends_with(".pdf");
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
            .bg(rgb(0xffffff));
        let mut titlebar = div()
            .h(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_1()
            .bg(rgb(0xfafaf9))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(crate::ui::file_icon(path, false, false))
            .child(div().min_w_0().text_ellipsis().child(filename))
            .child(div().flex_1());
        if is_pdf {
            let source = source.clone();
            titlebar = titlebar.child(button("export-reading-pdf", "导出").on_click(
                cx.listener(move |app, _, window, cx| app.export_pdf(source.clone(), window, cx)),
            ));
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
        if let Some(pdf) = self.pdf_panes.get(&format!("reading:{path}")) {
            pane = pane.child(div().flex_1().min_h_0().child(pdf.clone()));
        } else if let Some(html) = self.html_panes.get(&format!("reading:{path}")) {
            pane = pane.child(div().flex_1().min_h_0().child(html.clone()));
        } else if let Some(markdown) = self.markdown_panes.get(&format!("reading:{path}")) {
            pane = pane.child(div().flex_1().min_h_0().child(markdown.clone()));
        }
        pane.into_any_element()
    }

    fn remove_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let retry_id = id.to_owned();
        let removing = self
            .dialog_response
            .as_ref()
            .is_some_and(|(kind, _)| *kind == Dialog::RemoveProject);
        if self.cancellation.is_some()
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
            "从工作台移除项目？".into(),
            "文件夹和文件保留在磁盘上。".into(),
            DialogButtons::Confirm("移除项目".into()),
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

    fn render_trash(&self, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.trash_entries.iter().rev().map(|entry| {
            let restore = entry.id.clone();
            let purge = entry.id.clone();
            let date = |timestamp: u64| chrono::DateTime::from_timestamp(timestamp as i64, 0)
                .map(|date| date.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            div().flex().items_center().gap_3().px_4().py_3().border_b_1().border_color(rgb(BORDER))
                .child(crate::ui::file_icon(&entry.relative, false, false))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(div().text_ellipsis().child(Path::new(&entry.relative).file_name().expect("file name").to_string_lossy().into_owned()))
                    .child(div().text_xs().text_color(rgb(MUTED)).text_ellipsis().child(entry.project_root.join(&entry.relative).display().to_string()))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!("删除：{}  ·  清理：{}", date(entry.deleted_at), date(entry.deleted_at + 7 * 24 * 3600)))))
                .child(crate::ui::button_secondary(SharedString::from(format!("trash-restore-{}", entry.id)), "恢复")
                    .on_click(cx.listener(move |app, _, window, cx| app.restore_trash(vec![restore.clone()], window, cx))))
                .child(crate::ui::button_danger_soft(SharedString::from(format!("trash-purge-{}", entry.id)), "彻底删除")
                    .on_click(cx.listener(move |app, _, window, cx| app.purge_trash(vec![purge.clone()], window, cx))))
        });
        div().size_full().flex().flex_col().bg(rgb(0xffffff))
            .child(div().flex().items_center().justify_between().px_5().py_4().border_b_1().border_color(rgb(BORDER))
                .child(div().font_semibold().child(format!("废纸篓 · {} 项", self.trash_entries.len())))
                .child(crate::ui::button_danger_soft("trash-clear", "清空废纸篓")
                    .on_click(cx.listener(|app, _, window, cx| {
                        let ids = app.trash_entries.iter().map(|entry| entry.id.clone()).collect();
                        app.purge_trash(ids, window, cx);
                    }))))
            .child(div().id("trash-list").flex_1().min_h_0().overflow_y_scroll()
                .when(self.trash_entries.is_empty(), |area| area.p_5().text_color(rgb(MUTED)).child("废纸篓为空"))
                .children(entries))
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        if !cx.has_active_drag() { self.dragging_tree_item.store(false, Ordering::Relaxed); }
        let dragging_tree_item = cx.has_active_drag() && self.dragging_tree_item.load(Ordering::Relaxed);
        let user_project = self.config.active_id.is_some();
        let projects = self.config.projects.iter().map(|project| {
            let id = project.id.clone();
            let rename_id = id.clone();
            let row_rename = id.clone();
            let row_remove = id.clone();
            let row_name = project.name.clone();
            let menu_app = cx.entity();
            let name = project.name.clone();
            let root = PathBuf::from(&project.folder);
            let app = cx.entity();
            div()
                .id(SharedString::from(format!("project-{}", project.id)))
                .flex()
                .items_center()
                .gap_2()
                .mx_2()
                .px_2()
                .h(px(32.))
                .flex_shrink_0()
                .rounded(px(4.))
                .cursor_pointer()
                .when(!self.show_trash && Some(&project.id) == self.config.active_id.as_ref(), |row| {
                    row.bg(rgb(0xe8e7e4))
                })
                .hover(|style| style.bg(rgb(0xefefed)))
                .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                .child(crate::ui::icon(IconName::Folder))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(project.name.clone()),
                )
                .child(
                    Button::new(SharedString::from(format!("project-menu-{id}")))
                        .ghost()
                        .compact()
                        .w(px(27.))
                        .h(px(27.))
                        .icon(crate::ui::icon(IconName::Ellipsis))
                        .tooltip("更多操作")
                        .dropdown_menu(move |menu, _, _| {
                            let rename_app = menu_app.clone();
                            let remove_app = menu_app.clone();
                            let rename = row_rename.clone();
                            let name = row_name.clone();
                            let remove = row_remove.clone();
                            menu.item(PopupMenuItem::new("重命名项目…").on_click(
                                move |_, window, cx| {
                                    rename_app.update(cx, |app, cx| {
                                        app.start_name(
                                            NameAction::RenameProject(rename.clone()),
                                            &name,
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            ))
                            .item(
                                PopupMenuItem::new("移除项目…").on_click(move |_, window, cx| {
                                    remove_app.update(cx, |app, cx| {
                                        app.remove_project(&remove, window, cx)
                                    });
                                }),
                            )
                        }),
                )
                .on_click(
                    cx.listener(move |app, _, window, cx| { app.show_trash = false; app.switch_project(&id, window, cx); cx.notify(); }),
                )
                .context_menu(move |menu, _, _| {
                    let root = root.clone();
                    let name = name.clone();
                    let id = rename_id.clone();
                    let app = app.clone();
                    let remove_app = app.clone();
                    let remove_id = id.clone();
                    menu.item(
                        PopupMenuItem::new("在资源管理器中显示")
                            .on_click(move |_, _, cx| reveal_path(&root, cx)),
                    )
                    .item(
                        PopupMenuItem::new("重命名项目…").on_click(move |_, window, cx| {
                            app.update(cx, |app, cx| {
                                app.start_name(
                                    NameAction::RenameProject(id.clone()),
                                    &name,
                                    window,
                                    cx,
                                )
                            })
                        }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("移除项目…").on_click(move |_, window, cx| {
                            remove_app
                                .update(cx, |app, cx| app.remove_project(&remove_id, window, cx))
                        }),
                    )
                })
        });
        let query = if self.searching {
            self.search_input.read(cx).value().to_lowercase()
        } else {
            String::new()
        };
        let mut entries = BTreeMap::new();
        if self.directory_workspace().is_none() {
            for path in self
                .tabs
                .iter()
                .filter(|path| path.to_lowercase().contains(&query))
            {
                entries.insert(path.clone(), false);
            }
        } else {
            for folder in self
                .folders
                .iter()
                .filter(|folder| folder.to_lowercase().contains(&query))
            {
                let parts: Vec<_> = folder.split('/').collect();
                for depth in 1..=parts.len() {
                    entries.insert(parts[..depth].join("/"), true);
                }
            }
            for file in self
                .files
                .iter()
                .filter(|file| file.path.to_lowercase().contains(&query))
            {
                let parts: Vec<_> = file.path.split('/').collect();
                for depth in 1..parts.len() {
                    entries.insert(parts[..depth].join("/"), true);
                }
                entries.insert(file.path.clone(), false);
            }
        }
        let mut entries: Vec<_> = entries.iter().collect();
        // Group sibling files as PDF, Markdown, TeX sources/support, then other
        // extensions. Component-wise keys keep folders first and children together.
        entries.sort_by_cached_key(|(path, folder)| {
            let mut parts = path.split('/').peekable();
            let mut key = Vec::new();
            while let Some(part) = parts.next() {
                let name = part.to_lowercase();
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                let group = if parts.peek().is_some() || **folder {
                    0
                } else if name == ".latexmkrc" {
                    3
                } else {
                    match extension {
                        "pdf" => 1,
                        "md" => 2,
                        "tex" | "ltx" | "bib" | "sty" | "cls" | "bst" | "bbx" | "cbx"
                        | "cfg" | "def" | "clo" | "ist" | "glsdefs" | "tikz" | "pgf"
                        | "asy" => 3,
                        _ => 4,
                    }
                };
                key.push((
                    group,
                    if group == 4 { extension.to_owned() } else { String::new() },
                    name,
                    part.to_owned(),
                ));
            }
            key
        });
        self.tree_row_bounds.lock().expect("tree bounds").clear();
        let rows = entries
            .into_iter()
            .filter(|(path, _)| {
                !query.is_empty()
                    || !self
                        .collapsed_folders
                        .iter()
                        .any(|folder| path.starts_with(&format!("{folder}/")))
            })
            .map(|(path, folder)| {
                let row_path = path.clone();
                let is_file = !*folder && self.directory_workspace().is_some();
                let row = (|| {
                let depth = if self.directory_workspace().is_some() {
                    path.matches('/').count()
                } else {
                    0
                };
                if self.directory_workspace().is_none() {
                    let key = path.clone();
                    let active = self.active == key;
                    let close = key.clone();
                    let rename_key = key.clone();
                    let rename_name = Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
                    let rename_app = cx.entity();
                    let full = PathBuf::from(path);
                    let reveal = full.clone();
                    let copy = full.clone();
                    let preview_font: SharedString = if self.settings.interface_font.is_empty() { "Segoe UI".into() } else { self.settings.interface_font.clone().into() };
                    let preview_font_size = self.settings.interface_font_size;
                    let trash_bounds = self.trash_bounds.clone();
                    let dragging = self.dragging_tree_item.clone();
                    let folder_bounds = self.tree_row_bounds.clone();
                    let close_id = SharedString::from(format!("close-default-file-{key}"));
                    let name = Path::new(path)
                        .file_name()
                        .expect("standalone file has a name")
                        .to_string_lossy()
                        .into_owned();
                    return div()
                        .id(SharedString::from(format!("default-file-{key}")))
                        .on_drag(crate::terminal::DraggedPaths(vec![full], None), move |paths, offset, _, cx| {
                            dragging.store(true, Ordering::Relaxed);
                            cx.new(|_| crate::terminal::PathsPreview { paths: paths.0.clone(), offset, font: preview_font.clone(), font_size: preview_font_size, trash_bounds: trash_bounds.clone(), is_folder: false, can_trash: false, folder_bounds: folder_bounds.clone() })
                        })
                        .flex()
                        .items_center()
                        .h(px(32.))
                        .px_3()
                        .gap_2()
                        .cursor_pointer()
                        .when(active, |row| row.bg(rgb(0xe8e7e4)))
                        .hover(|style| style.bg(rgb(0xefefed)))
                        .child(crate::ui::file_icon(&key, false, false))
                        .child(div().flex_1().min_w_0().text_ellipsis().child(name))
                        .child(
                            crate::ui::control(close_id, IconName::Close).on_click(cx.listener(
                                move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_tab(&close, window, cx);
                                },
                            )),
                        )
                        .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            app.show_trash = false;
                            app.active = key.clone();
                            if event.click_count() >= 2 {
                                if let Some(pane) = app.markdown_panes.get(&key) {
                                    pane.update(cx, |pane, cx| pane.focus(window, cx));
                                } else if let Some(document) = app.documents.get(&key) {
                                    document
                                        .editor
                                        .update(cx, |editor, cx| editor.focus(window, cx));
                                }
                            } else {
                                app.file_tree_focus.focus(window, cx);
                            }
                            if let Err(error) = app.remember_session() {
                                app.report(error, cx);
                            }
                            cx.notify();
                        }))
                        .context_menu(move |menu, _, _| {
                            let app = rename_app.clone();
                            let path = rename_key.clone();
                            let name = rename_name.clone();
                            let reveal = reveal.clone();
                            let copy = copy.clone();
                            menu.item(PopupMenuItem::new("在资源管理器中显示").on_click(move |_, _, cx| reveal_path(&reveal, cx)))
                            .item(PopupMenuItem::new("复制完整路径").on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string_lossy().into_owned()));
                            }))
                            .when(path.to_lowercase().ends_with(".md"), |menu| {
                                menu.item(PopupMenuItem::new("重命名…").on_click(move |_, window, cx| {
                                    app.update(cx, |app, cx| app.start_name(NameAction::RenameFile(path.clone()), &name, window, cx));
                                }))
                            })
                        })
                        .into_any_element();
                }
                if *folder {
                    let toggled = path.clone();
                    let is_focused = self.focused_folder.as_deref() == Some(path);
                    let full = PathBuf::from(&self.directory_workspace().expect("folder has project").folder)
                        .join(path);
                    let deleted = path.clone();
                    let row_deleted = path.clone();
                    let app = cx.entity();
                    let folder_drag = crate::terminal::DraggedFolder(path.clone(), self.directory_workspace().expect("folder has project").id.clone());
                    let folder_path = full.clone();
                    let folder_font: SharedString = if self.settings.interface_font.is_empty() { "Segoe UI".into() } else { self.settings.interface_font.clone().into() };
                    let folder_font_size = self.settings.interface_font_size;
                    let trash_bounds = self.trash_bounds.clone();
                    let dragging = self.dragging_tree_item.clone();
                    let drop_folder = path.clone();
                    let folder_bounds = self.tree_row_bounds.clone();
                    let drop_project = self.directory_workspace().expect("folder has project").id.clone();
                    return div()
                        .id(SharedString::from(format!("folder-{path}")))
                        .on_drag(folder_drag, move |_, offset, _, cx| {
                            dragging.store(true, Ordering::Relaxed);
                            cx.new(|_| crate::terminal::PathsPreview { paths: vec![folder_path.clone()], offset, font: folder_font.clone(), font_size: folder_font_size, trash_bounds: trash_bounds.clone(), is_folder: true, can_trash: true, folder_bounds: folder_bounds.clone() })
                        })
                        .drag_over::<crate::terminal::DraggedPaths>(move |style, dragged, _, _| {
                            if dragged.1.as_deref() == Some(drop_project.as_str()) { style.bg(rgb(0xdde9f6)) } else { style }
                        })
                        .on_drop(cx.listener(move |app, dragged: &crate::terminal::DraggedPaths, window, cx| {
                            cx.stop_propagation();
                            if let Err(error) = app.move_tree_files(dragged, &drop_folder, window, cx) { app.report(error, cx); }
                        }))
                        .flex()
                        .h(px(29.))
                        .items_center()
                        .gap_2()
                        .pl(px(12. + depth as f32 * 15.))
                        .cursor_pointer()
                        .when(is_focused, |row| row.bg(rgb(0xedf2fa)))
                        .hover(|style| style.bg(rgb(0xefefed)))
                        .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                        .child(crate::ui::icon(if self.collapsed_folders.contains(path) {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        }))
                        .child(crate::ui::icon(if self.collapsed_folders.contains(path) {
                            IconName::Folder
                        } else {
                            IconName::FolderOpen
                        }))
                        .child(
                            div().flex_1().min_w_0().text_ellipsis().child(
                                Path::new(path)
                                    .file_name()
                                    .expect("folder has name")
                                    .to_string_lossy()
                                    .into_owned(),
                            ),
                        )
                        .child(
                            crate::ui::control(
                                SharedString::from(format!("delete-folder-{path}")),
                                IconName::Trash,
                            )
                            .on_click(cx.listener(
                                move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.delete_folder(&row_deleted, window, cx);
                                },
                            )),
                        )
                        .on_click(cx.listener(move |app, _, window, cx| {
                            cx.stop_propagation();
                            app.show_trash = false;
                            app.focused_folder = Some(toggled.clone());
                            if !app.collapsed_folders.remove(&toggled) {
                                app.collapsed_folders.insert(toggled.clone());
                            }
                            app.file_tree_focus.focus(window, cx);
                            cx.notify();
                        }))
                        .context_menu(move |menu, _, _| {
                            let full = full.clone();
                            let deleted = deleted.clone();
                            let new_md_folder = deleted.clone();
                            let renamed_path = deleted.clone();
                            let renamed_name = Path::new(&deleted)
                                .file_name()
                                .expect("folder has name")
                                .to_string_lossy()
                                .into_owned();
                            let folder_prefix = format!("{deleted}/");
                            let folder_prefix_dir = folder_prefix.clone();
                            let app = app.clone();
                            let rename_app = app.clone();
                            let new_md_app = app.clone();
                            let create_file_app = app.clone();
                            let create_folder_app = app.clone();
                            menu.item(PopupMenuItem::new("新建 Markdown").on_click(
                                move |_, window, cx| {
                                    new_md_app.update(cx, |app, cx| {
                                        app.focused_folder = Some(new_md_folder.clone());
                                        if let Err(error) = app.new_markdown_in(
                                            Some(&new_md_folder),
                                            window,
                                            cx,
                                        ) {
                                            app.report(error, cx);
                                        }
                                    })
                                },
                            ))
                            .item(PopupMenuItem::new("新建文件…").on_click(
                                move |_, window, cx| {
                                    create_file_app.update(cx, |app, cx| {
                                        app.start_name(
                                            NameAction::Create,
                                            &folder_prefix,
                                            window,
                                            cx,
                                        )
                                    })
                                },
                            ))
                            .item(PopupMenuItem::new("新建文件夹…").on_click(
                                move |_, window, cx| {
                                    create_folder_app.update(cx, |app, cx| {
                                        app.start_name(
                                            NameAction::CreateFolder,
                                            &folder_prefix_dir,
                                            window,
                                            cx,
                                        )
                                    })
                                },
                            ))
                            .separator()
                            .item(
                                PopupMenuItem::new("在资源管理器中显示")
                                    .on_click(move |_, _, cx| reveal_path(&full, cx)),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("重命名…").on_click(move |_, window, cx| {
                                    rename_app.update(cx, |app, cx| {
                                        app.start_name(
                                            NameAction::RenameFolder(renamed_path.clone()),
                                            &renamed_name,
                                            window,
                                            cx,
                                        )
                                    })
                                }),
                            )
                            .item(
                                PopupMenuItem::new("删除文件夹…").on_click(move |_, window, cx| {
                                    app.update(cx, |app, cx| {
                                        app.delete_folder(&deleted, window, cx)
                                    })
                                }),
                            )
                        })
                        .into_any_element();
                }
                let file = self
                    .files
                    .iter()
                    .find(|file| &file.path == path)
                    .expect("tree entry is a listed file");
                let open = file.clone();
                let rename = file.path.clone();
                let delete = file.path.clone();
                let full = PathBuf::from(&self.directory_workspace().expect("file has project").folder)
                    .join(&file.path);
                let app = cx.entity();
                let split = file.clone();
                let row_split = file.clone();
                let row_rename = file.path.clone();
                let row_delete = file.path.clone();
                let selected_path = file.path.clone();
                let clicked_path = file.path.clone();
                let menu_app = cx.entity();
                let row_name = Path::new(path)
                    .file_name()
                    .expect("file has name")
                    .to_string_lossy()
                    .into_owned();
                let pdf_file = file.path.to_lowercase().ends_with(".pdf");
                let name = Path::new(&file.path)
                    .file_name()
                    .expect("file has name")
                    .to_string_lossy()
                    .to_string();
                let is_selected = self.selected_files.contains(path);
                let dragged_paths = if is_selected {
                    self.selected_files.iter().map(|path| self.document_path(path)).collect()
                } else {
                    vec![full.clone()]
                };
                let preview_font: SharedString = if self.settings.interface_font.is_empty() {
                    "Segoe UI".into()
                } else {
                    self.settings.interface_font.clone().into()
                };
                let preview_font_size = self.settings.interface_font_size;
                let drag_project = self.directory_workspace().expect("file has project").id.clone();
                let trash_bounds = self.trash_bounds.clone();
                let dragging = self.dragging_tree_item.clone();
                let folder_bounds = self.tree_row_bounds.clone();
                div()
                    .id(SharedString::from(format!("file-{}", file.path)))
                    .on_drag(crate::terminal::DraggedPaths(dragged_paths, Some(drag_project)), move |paths, offset, _, cx| {
                        dragging.store(true, Ordering::Relaxed);
                        cx.new(|_| crate::terminal::PathsPreview { paths: paths.0.clone(), offset, font: preview_font.clone(), font_size: preview_font_size, trash_bounds: trash_bounds.clone(), is_folder: false, can_trash: true, folder_bounds: folder_bounds.clone() })
                    })
                    .flex()
                    .items_center()
                    .h(px(29.))
                    .pl(px(14. + depth as f32 * 15.))
                    .pr_2()
                    .gap_2()
                    .cursor_pointer()
                    .when(is_selected, |row| row.bg(rgb(0xdde9f6)))
                    .when(!is_selected && self.active == file.path, |row| row.bg(rgb(0xe8e7e4)))
                    .hover(|style| style.bg(rgb(0xefefed)))
                    .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                    .child(
                        div()
                            .id(SharedString::from(format!("select-{path}")))
                            .w(px(15.))
                            .h(px(15.))
                            .rounded(px(3.5))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(is_selected, |s| {
                                s.bg(rgb(0x165dff))
                                    .border_1()
                                    .border_color(rgb(0x165dff))
                                    .text_color(crate::theme::on_accent())
                                    .active(|a| a.bg(rgb(0x0e42d2)).border_color(rgb(0x0e42d2)))
                                    .child(crate::ui::icon(IconName::Check).w(px(10.)).h(px(10.)))
                            })
                            .when(!is_selected, |s| {
                                s.bg(rgb(0xffffff))
                                    .border_1()
                                    .border_color(rgb(0xc4c7cc))
                                    .hover(|h| h.border_color(rgb(0x165dff)).bg(rgb(0xf2f7ff)))
                                    .active(|a| a.border_color(rgb(0x0e42d2)).bg(rgb(0xdde9f6)))
                            })
                            .on_click(cx.listener(move |app, _, window, cx| {
                                cx.stop_propagation();
                                if !app.selected_files.remove(&selected_path) {
                                    app.selected_files.insert(selected_path.clone());
                                }
                                app.file_tree_focus.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(crate::ui::file_icon(
                        &file.path,
                        file.kind == FileKind::Asset,
                        self.directory_workspace()
                            .is_some_and(|project| project.main_file == file.path),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .when(
                                self.directory_workspace()
                                    .is_some_and(|project| project.main_file == file.path),
                                |name| {
                                    name.text_color(rgb(crate::ui::MAIN_TEX_COLOR))
                                        .font_semibold()
                                },
                            )
                            .child(name.clone()),
                    )
                    .when(
                        pdf_file || Self::is_html_path(&file.path) || file.path.to_lowercase().ends_with(".md"),
                        |row| {
                            row.child(
                                crate::ui::control(
                                    SharedString::from(format!("split-{path}")),
                                    IconName::Columns2,
                                )
                                .on_click(cx.listener(
                                    move |app, _, window, cx| {
                                        cx.stop_propagation();
                                        app.open_reading_pane(&row_split, window, cx);
                                    },
                                )),
                            )
                        },
                    )
                    .child(
                        Button::new(SharedString::from(format!("file-menu-{path}")))
                            .ghost()
                            .compact()
                            .w(px(27.))
                            .h(px(27.))
                            .icon(crate::ui::icon(IconName::Ellipsis))
                            .tooltip("更多操作")
                            .dropdown_menu(move |menu, _, _| {
                                let rename_app = menu_app.clone();
                                let delete_app = menu_app.clone();
                                let rename = row_rename.clone();
                                let name = row_name.clone();
                                let delete = row_delete.clone();
                                menu.item(PopupMenuItem::new("重命名…").on_click(
                                    move |_, window, cx| {
                                        rename_app.update(cx, |app, cx| {
                                            app.start_name(
                                                NameAction::RenameFile(rename.clone()),
                                                &name,
                                                window,
                                                cx,
                                            )
                                        });
                                    },
                                ))
                                .item(
                                    PopupMenuItem::new("删除…").on_click(move |_, window, cx| {
                                        delete_app.update(cx, |app, cx| {
                                            app.delete_file(&delete, window, cx)
                                        });
                                    }),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        app.show_trash = false;
                        if event.modifiers().control {
                            if !app.selected_files.remove(&clicked_path) {
                                app.selected_files.insert(clicked_path.clone());
                            }
                            app.focused_folder = None;
                            app.file_tree_focus.focus(window, cx);
                            cx.notify();
                            return;
                        }
                        app.focused_folder = None;
                        if let Err(error) =
                            app.open_file_with_focus(&open, event.click_count() >= 2, window, cx)
                        {
                            app.report(error, cx);
                        }
                        if event.click_count() < 2 {
                            app.file_tree_focus.focus(window, cx);
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let reveal = full.clone();
                        let copied = full.to_string_lossy().to_string();
                        let rename = rename.clone();
                        let name = name.clone();
                        let delete = delete.clone();
                        let rename_app = app.clone();
                        let delete_app = app.clone();
                        let main_app = app.clone();
                        let main = rename.clone();
                        let split = split.clone();
                        let split_app = app.clone();
                        menu.item(
                            PopupMenuItem::new("在资源管理器中显示")
                                .on_click(move |_, _, cx| reveal_path(&reveal, cx)),
                        )
                        .item(
                            PopupMenuItem::new("复制完整路径").on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                            }),
                        )
                        .when(
                            split.path.to_lowercase().ends_with(".pdf")
                                || Self::is_html_path(&split.path)
                                || split.path.to_lowercase().ends_with(".md"),
                            |menu| {
                                menu.item(PopupMenuItem::new("在分屏中打开").on_click(
                                    move |_, window, cx| {
                                        split_app.update(cx, |app, cx| {
                                            app.open_reading_pane(&split, window, cx)
                                        })
                                    },
                                ))
                            },
                        )
                        .when(user_project && main.to_lowercase().ends_with(".tex"), |menu| {
                            menu.item(PopupMenuItem::new("设为编译主文件").on_click(
                                move |_, _, cx| {
                                    main_app.update(cx, |app, cx| {
                                        if app.cancellation.is_some() {
                                            return;
                                        }
                                        app.config
                                            .projects
                                            .iter_mut()
                                            .find(|project| {
                                                Some(&project.id) == app.config.active_id.as_ref()
                                            })
                                            .expect("file has active project")
                                            .main_file = main.clone();
                                        if let Err(error) = storage::save_projects(&app.config) {
                                            app.report(error, cx);
                                        }
                                        cx.notify();
                                    })
                                },
                            ))
                        })
                        .separator()
                        .item(
                            PopupMenuItem::new("重命名…").on_click(move |_, window, cx| {
                                rename_app.update(cx, |app, cx| {
                                    app.start_name(
                                        NameAction::RenameFile(rename.clone()),
                                        &name,
                                        window,
                                        cx,
                                    )
                                })
                            }),
                        )
                        .item(PopupMenuItem::new("删除…").on_click(move |_, window, cx| {
                            delete_app.update(cx, |app, cx| app.delete_file(&delete, window, cx))
                        }))
                    })
                    .into_any_element()
                })();
                let bounds = self.tree_row_bounds.clone();
                div().id(SharedString::from(format!("tree-row-{row_path}"))).relative()
                    // The child handles file dragging; blank-space presses belong to marquee selection.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(row).child(
                    canvas(move |area, _, _| {
                        bounds.lock().expect("tree bounds").insert(row_path.clone(), (is_file, area));
                    }, |_, _, _, _| {}).absolute().top_0().left_0().size_full(),
                )
            });
        let default_space = div()
            .id("default-space")
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .px_2()
            .h(px(32.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(!self.show_trash && self.config.active_workspace == Some(WorkspaceLocation::Default), |row| row.bg(rgb(0xe8e7e4)))
            .hover(|style| style.bg(rgb(0xefefed)))
            .child(crate::ui::icon(IconName::NotebookPen))
            .child(div().flex_1().child("默认空间"))
            .child(crate::ui::control("default-new-markdown", IconName::Plus)
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("在默认空间新建 Markdown").build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|app, _, window, cx| {
                    cx.stop_propagation();
                    if let Err(error) = app.new_default_markdown(window, cx) { app.report(error, cx); }
                })))
            .on_click(cx.listener(|app, _, window, cx| { app.show_trash = false; app.switch_to_default(window, cx); cx.notify(); }));
        let loose_count = if self.directory_workspace().is_none() { self.tabs.len() } else { self.workspace_sessions.get("loose").map_or(0, |session| session.tabs.len()) };
        div()
            .w_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0xfbfbfa))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(div().pt_2().child(default_space))
            .child(div().id("loose-files").flex().items_center().gap_2()
                .mx_2().px_2().h(px(32.)).rounded(px(4.)).cursor_pointer()
                .when(!self.show_trash && self.directory_workspace().is_none(), |row| row.bg(rgb(0xe8e7e4)))
                .hover(|style| style.bg(rgb(0xefefed)))
                .child(crate::ui::icon(IconName::Files))
                .child(div().flex_1().child("独立文件"))
                .when(loose_count > 0, |row| row.pr_1().child(div().text_xs().text_color(rgb(MUTED)).child(loose_count.to_string())))
                .on_click(cx.listener(|app, _, window, cx| {
                app.show_trash = false;
                if app.directory_workspace().is_none() { return; }
                if app.cancellation.is_some() || app.batch_running || !app.confirm_drafts(window, cx, |app, window, cx| {
                    if let Err(error) = app.select_loose_files(window, cx) { app.report(error, cx); }
                }) { return; }
                if let Err(error) = app.select_loose_files(window, cx) { app.report(error, cx); }
            })))
            .child(div().relative().flex_1().min_h_0().overflow_hidden().child(
                div().id("sidebar-content-scroll").flex().flex_col().size_full().overflow_y_scroll()
                .track_scroll(&self.sidebar_scroll)
                .child(div().pt_5().pb_2().px_4().flex_shrink_0().text_xs().text_color(rgb(MUTED)).child("项目"))
                .children(projects)
                .child(div().id("add-project").flex().flex_shrink_0().items_center().gap_2().mx_2().px_2().h(px(32.))
                    .rounded(px(4.)).cursor_pointer().text_color(rgb(MUTED))
                    .hover(|style| style.bg(rgb(0xefefed)))
                    .child(crate::ui::icon(IconName::Plus)).child("添加本地项目")
                    .on_click(cx.listener(|app, _, window, cx| app.add_project(&OpenProject, window, cx))))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .pt_2()
                    .flex_shrink_0()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("文件")
                    .child(button("new-markdown", "+ Markdown").on_click(cx.listener(|app, _, window, cx| {
                        if let Err(error) = app.new_markdown(window, cx) { app.report(error, cx); }
                    })))
                    .when(self.directory_workspace().is_none(), |header| header.child(button("open-file", "打开文件").on_click(
                        cx.listener(|app, _, window, cx| app.open_file_dialog(window, cx)),
                    )))
                    .when(self.directory_workspace().is_some(), |header| {
                        header.child(
                            div()
                                .flex()
                                .gap_1()
                                .child(crate::ui::control("refresh-files", IconName::RotateCcw)
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("刷新文件列表")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        // Manual refresh reconciles the whole directory and all open
                                        // drafts, regardless of pending watcher event filtering.
                                        app.watch_paths.clear();
                                        app.watch_tree = true;
                                        app.watch_changed = None;
                                        match app.refresh_files(window, cx) {
                                            Ok(()) => {
                                                app.selected_files.retain(|path| {
                                                    app.files.iter().any(|file| &file.path == path)
                                                });
                                                app.report("已刷新文件列表。", cx);
                                            }
                                            Err(error) => app.report(error, cx),
                                        }
                                    })))
                                .child(button("new-file", "+").on_click(cx.listener(
                                    |app, _, window, cx| {
                                        let prefix = app.focused_folder_prefix();
                                        app.start_name(NameAction::Create, &prefix, window, cx)
                                    },
                                )))
                                .child(button("new-folder", "📁+").on_click(cx.listener(
                                    |app, _, window, cx| {
                                        let prefix = app.focused_folder_prefix();
                                        app.start_name(NameAction::CreateFolder, &prefix, window, cx)
                                    },
                                )))
                                .child(button("find-file", "⌕").on_click(cx.listener(
                                    |app, _, window, cx| app.find_file(&FindFile, window, cx),
                                ))),
                        )
                    }),
            )
            .when(self.searching, |sidebar| {
                sidebar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .pb_2()
                        .child(Input::new(&self.search_input).flex_1().min_w_0())
                        .child(
                            crate::ui::control("close-file-search", IconName::Close).on_click(
                                cx.listener(|app, _, _, cx| {
                                    app.searching = false;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
            })
            .when(!self.selected_files.is_empty(), |sidebar| {
                let sel_count = self.selected_files.len();
                sidebar.child(
                    div()
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .bg(rgb(0xf7f6f3))
                        .border_1()
                        .border_color(rgb(0xe8e7e3))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1p5()
                        .child(
                            div().flex().items_center().gap_1().child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x787774))
                                    .child(format!("已选 {sel_count} 项")),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    crate::ui::button_secondary("batch-compile", "编译")
                                        .min_h(px(22.))
                                        .py_0p5()
                                        .text_xs()
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.compile_selected(window, cx)
                                        })),
                                )
                                .child(
                                    crate::ui::button_danger_soft("batch-delete", "")
                                        .min_h(px(22.))
                                        .py_0p5()
                                        .gap_1()
                                        .text_xs()
                                        .child(
                                            crate::ui::icon(IconName::Trash).w(px(11.)).h(px(11.)),
                                        )
                                        .child("删除")
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.delete_selected(window, cx)
                                        })),
                                )
                                .child(
                                    crate::ui::control("clear-selection", IconName::Close)
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.selected_files.clear();
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .child(
                // Preserve the tree's content height for the outer scroll handle. A zero
                // flex basis hides overflowing rows from its measured scroll extent.
                div().relative().flex().flex_col().flex_grow(1.).flex_shrink_0().child(div()
                    .id("file-tree-scroll")
                    .flex().flex_col().w_full().flex_grow(1.).flex_shrink_0()
                    .key_context("FolioFileTree")
                    .track_focus(&self.file_tree_focus)
                    .on_action(cx.listener(Self::copy_file_path))
                    .on_action(cx.listener(Self::delete_file_tree_item))
                    .on_action(cx.listener(Self::undo_file_delete))
                    .on_mouse_down(MouseButton::Left, cx.listener(|app, event: &MouseDownEvent, _, cx| {
                        if app.directory_workspace().is_none() || app.tree_row_bounds.lock().expect("tree bounds").values().any(|(_, bounds)| bounds.contains(&event.position)) {
                            return;
                        }
                        app.tree_drag = Some(TreeSelectionDrag {
                            start: event.position,
                            current: event.position,
                            original: if event.modifiers.control { app.selected_files.clone() } else { BTreeSet::new() },
                            moved: false,
                        });
                        cx.notify();
                    }))
                    .on_mouse_move(cx.listener(|app, event: &MouseMoveEvent, _, cx| {
                        if cx.has_active_drag() {
                            if app.tree_drag.take().is_some() { cx.notify(); }
                            return;
                        }
                        let Some(drag) = app.tree_drag.as_mut() else { return; };
                        if !event.dragging() { return; }
                        drag.current = event.position;
                        if (drag.current - drag.start).magnitude() > 3.0 { drag.moved = true; }
                        if drag.moved {
                            let selection = drag.bounds();
                            app.selected_files = drag.original.clone();
                            // Bounds are measured in window coordinates, including the scroll offset.
                            for (path, (is_file, bounds)) in app.tree_row_bounds.lock().expect("tree bounds").iter() {
                                if *is_file && selection.intersects(bounds) { app.selected_files.insert(path.clone()); }
                            }
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|app, _, _, cx| {
                        if let Some(drag) = app.tree_drag.take() {
                            if !drag.moved { app.selected_files = drag.original; }
                            cx.notify();
                        }
                    }))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|app, _, _, cx| {
                        if app.tree_drag.take().is_some() { cx.notify(); }
                    }))
                    .on_click(cx.listener(|app, _, window, cx| {
                        if app.focused_folder.take().is_some() {
                            cx.notify();
                        }
                        app.file_tree_focus.focus(window, cx);
                    }))
                    .children(rows)
                    .when_some(self.directory_workspace(), |tree, project| {
                        let project_id = project.id.clone();
                        let bounds = self.tree_row_bounds.clone();
                        // Empty space belongs to the root; folder rows retain their own drop targets.
                        tree.child(div().id("project-root-drop").relative()
                            .flex_grow(1.).h(px(48.)).flex_shrink_0()
                            .drag_over::<crate::terminal::DraggedPaths>(move |style, dragged, _, _| {
                                if dragged.1.as_deref() == Some(project_id.as_str()) { style.bg(rgb(0xdde9f6)) } else { style }
                            })
                            .child(canvas(move |area, _, _| {
                                bounds.lock().expect("tree bounds").insert(String::new(), (false, area));
                            }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
                            .on_click(cx.listener(|app, _, window, cx| {
                                cx.stop_propagation();
                                app.focused_folder = None;
                                app.file_tree_focus.focus(window, cx);
                                cx.notify();
                            }))
                            .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedPaths, window, cx| {
                                cx.stop_propagation();
                                if let Err(error) = app.move_tree_files(dragged, "", window, cx) { app.report(error, cx); }
                            })))
                    })
                    .when(self.directory_workspace().is_none(), |tree| tree.child(div().h(px(48.)).flex_shrink_0())))
                    .when_some(self.tree_drag.as_ref().filter(|drag| drag.moved).map(TreeSelectionDrag::bounds), |area, selection| {
                        area.child(canvas(|_, _, _| (), move |_, _, window, _| {
                            window.paint_quad(fill(selection, rgb(0x165dff).opacity(0.18)));
                        }).absolute().top_0().left_0().size_full())
                    }),
            ))
                .child(div().absolute().top(px(20.)).bottom(px(20.)).left_0().right_0().child(
                    Scrollbar::vertical(&self.sidebar_scroll)
                        .mode(ScrollbarMode::Always)
                        .styles(|styles| styles
                            .thumb(|style| style.bg(rgb(0xd9d9d7)).width(px(6.)).inset(px(1.)).radius(px(4.)).min_length(px(24.)))
                            .thumb_hover(|style| style.bg(rgb(0xbcbcb9)).width(px(6.)).inset(px(1.)).radius(px(4.)))
                            .thumb_active(|style| style.bg(rgb(0xa8a8a5)).width(px(6.)).inset(px(1.)).radius(px(4.)))),
                )))
            .child(div().mx_4().h(px(1.)).flex_shrink_0().bg(rgb(0xeae9e6)))
            .child({
                let bounds = self.trash_bounds.clone();
                div().id("trash-entry").relative().flex().flex_shrink_0().items_center().gap_2()
                    .mx_2().my_2().px_2().h(px(32.)).rounded(px(4.)).cursor_pointer()
                    .when(self.show_trash, |row| row.bg(rgb(0xe8e7e4)))
                    .when(dragging_tree_item, |row| row.bg(rgb(0xf0f4fa)))
                    .drag_over::<crate::terminal::DraggedPaths>(|style, _, _, _| style.bg(rgb(0xdde9f6)))
                    .drag_over::<crate::terminal::DraggedFolder>(|style, _, _, _| style.bg(rgb(0xdde9f6)))
                    .child(crate::ui::icon(IconName::Trash))
                    .child("废纸篓")
                    .child(canvas(move |area, _, _| { *bounds.lock().expect("trash bounds") = Some(area); }, |_, _, _, _| {}).absolute().size_full())
                    .on_click(cx.listener(|app, _, _, cx| { app.show_trash = true; cx.notify(); }))
                    .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedPaths, window, cx| {
                        if dragged.1.is_none() { app.report("独立文件不支持删除。", cx); return; }
                        let Some(project) = app.directory_workspace() else { return; };
                        if Some(project.id.as_str()) != dragged.1.as_deref() { app.report("来源项目已切换，请重新拖动。", cx); return; }
                        let root = PathBuf::from(&project.folder);
                        let paths: Vec<String> = dragged.0.iter().filter_map(|path| path.strip_prefix(&root).ok().map(|path| path.to_string_lossy().replace('\\', "/"))).collect();
                        if paths.len() == dragged.0.len() { app.confirm_trash(paths, window, cx); }
                    }))
                    .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedFolder, window, cx| {
                        if app.directory_workspace().is_none_or(|project| project.id != dragged.1) {
                            app.report("来源项目已切换，请重新拖动。", cx);
                            return;
                        }
                        // A folder is a projection of supported files; its other contents remain in place.
                        app.delete_folder(&dragged.0, window, cx);
                    }))
            })
            .child(div().id("preferences").flex().flex_shrink_0().items_center().gap_2()
                .mx_2().mb_2().px_2().h(px(32.)).rounded(px(4.)).cursor_pointer()
                .text_color(rgb(MUTED)).hover(|style| style.bg(rgb(0xefefed)))
                .child(crate::ui::icon(IconName::Settings)).child("设置")
                .on_click(cx.listener(|app, _, window, cx| {
                    app.preferences.update(cx, |panel, cx| panel.sync(&app.settings, window, cx));
                    app.show_preferences = true;
                    cx.notify();
                })))
            .into_any_element()
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut area = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xffffff))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.focused_markdown = None),
            );
        if let Some(document) = self.documents.get(&self.active) {
            if document.buffer.deleted {
                area = area.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0xfff4e5))
                        .child("文件已从磁盘删除，草稿已保留。"),
                );
            } else if document.buffer.conflict.is_some() {
                let reload = self.active.clone();
                let overwrite = self.active.clone();
                area =
                    area.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_2()
                            .bg(rgb(0xfff4e5))
                            .child("磁盘文件已被外部修改。")
                            .child(button("load-disk", "使用磁盘版本").on_click(cx.listener(
                                move |app, _, window, cx| app.reload_disk(&reload, window, cx),
                            )))
                            .child(button("overwrite-disk", "保留草稿并覆盖").on_click(
                                cx.listener(move |app, _, _, cx| {
                                    if let Err(error) = app.save_file(&overwrite, true, cx) {
                                        app.report(error, cx);
                                    }
                                }),
                            )),
                    );
            }
            if let Some(markdown) = self.markdown_panes.get(&self.active) {
                area = area.child(div().flex_1().min_h_0().child(markdown.clone()));
            } else {
                let editor = document.editor.clone();
                let ask_editor = document.editor.clone();
                let ask_path = document.path.clone();
                let ask_source = document
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "源码".to_owned());
                area = area.child(div().h(px(32.)).px_3().flex().items_center().justify_end().gap_1()
                    .child(
                        Button::new("ask-ai-source")
                            .ghost()
                            .compact()
                            .icon(crate::ui::icon(IconName::Sparkles))
                            .label("问 AI")
                            .tooltip("将当前选中内容（或当前文件）发送到 AI 助手")
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let selected = ask_editor.read(cx).selected_text().to_string();
                                app.open_ai_with_selection(
                                    ask_source.clone(),
                                    Some(ask_path.clone()),
                                    selected,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(crate::ui::copy_document_button(
                        SharedString::from(format!("copy-document:{}", document.path.display())),
                        move |cx| editor.read(cx).value().to_string(),
                    )));
                area = area.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .p_2()
                        .flex()
                        .justify_center()
                        .child(crate::ui::ctrl_scroll_capture(cx.listener(|app, event: &ScrollEvent, _, cx| {
                            if event.modifiers.control {
                                let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                                    1
                                } else {
                                    -1
                                };
                                app.settings.editor_font_size =
                                    (app.settings.editor_font_size as i16 + delta).clamp(11, 26)
                                        as u8;
                                if let Err(error) = storage::save_settings(&app.settings) {
                                    app.report(error, cx);
                                }
                                app.editor_zoom_hud.show(
                                    app.settings.editor_font_size,
                                    cx,
                                    |app| &mut app.editor_zoom_hud,
                                );
                                cx.stop_propagation();
                                cx.notify();
                            }
                        })))
                        .child(
                            div()
                                .size_full()
                                .when(self.settings.line_width_limited, |editor| {
                                    editor.max_w(px(850.))
                                })
                                .child(
                                    Editor::new(&document.editor)
                                        .appearance(false)
                                        .bordered(false)
                                        .size_full()
                                        .text_size(px(self.settings.editor_font_size as f32))
                                        .font_family(if self.settings.editor_font.is_empty() {
                                            SharedString::from("Consolas")
                                        } else {
                                            self.settings.editor_font.clone().into()
                                        }),
                                ),
                        )
                        .when_some(
                            self.editor_zoom_hud.render("editor-zoom-hud"),
                            |area, hud| area.child(hud),
                        ),
                );
            }
        } else if !self.active.is_empty() {
            let full = self.document_path(&self.active);
            if let Some(pane) = self.html_panes.get(&self.active) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else if let Some(pane) = self.pdf_panes.get(&self.active) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else {
                area = area.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_6()
                        .child(img(full).size_full().object_fit(ObjectFit::Contain)),
                );
            }
        } else {
            area = area.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(rgb(MUTED))
                    .child(div().text_xl().child("Folio"))
                    .child(if self.directory_workspace().is_none() {
                        "打开文件，或点击 + Markdown 开始写作"
                    } else {
                        "选择文件，或点击 + Markdown 开始写作"
                    })
                    .when(self.directory_workspace().is_none(), |area| area.child(button("welcome-open-file", "打开文件").on_click(
                        cx.listener(|app, _, window, cx| app.open_file_dialog(window, cx)),
                    )))
                    .child(if self.directory_workspace().is_none() {
                        "Ctrl+Shift+O 打开文件  ·  Ctrl+O 打开项目  ·  Ctrl+S 保存"
                    } else {
                        "Ctrl+O 打开项目  ·  Ctrl+S 保存"
                    }),
            );
        }
        area.into_any_element()
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(pane) = &self.build_pdf {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(36.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .bg(rgb(0xfafaf9))
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(crate::ui::icon(IconName::FileText))
                        .child("编译预览")
                        .child(div().flex_1())
                        .when_some(self.preview.clone(), |bar, path| {
                            bar.child(button("export-build-pdf", "导出").on_click(cx.listener(
                                move |app, _, window, cx| app.export_pdf(path.clone(), window, cx),
                            )))
                        })
                        .child(
                            crate::ui::control("close-build-preview", IconName::Close).on_click(
                                cx.listener(|app, _, _, cx| {
                                    app.show_build_preview = false;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
                .child(div().flex_1().min_h_0().child(pane.clone()))
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf1f1ef))
            .child(
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(0xffffff))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::ui::icon(IconName::FileText))
                            .child("预览"),
                    )
                    .when_some(self.preview.clone(), |toolbar, path| {
                        toolbar.child(
                            button("reveal-output", "在资源管理器中显示")
                                .on_click(move |_, _, cx| reveal_path(&path, cx)),
                        )
                    })
                    .child(
                        crate::ui::control("close-build-preview", IconName::Close).on_click(
                            cx.listener(|app, _, _, cx| {
                                app.show_build_preview = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .text_color(rgb(MUTED))
                    .child(if self.cancellation.is_some() {
                        "正在编译…"
                    } else if self.preview.is_some() {
                        "编译成功，PDF 已生成"
                    } else {
                        "编译后在此查看文档"
                    })
                    .when_some(self.preview.clone(), |area, path| {
                        area.child(
                            crate::ui::button_secondary("open-output", "打开生成的 PDF").on_click(
                                move |_, _, cx| {
                                    cx.open_url(&format!(
                                        "file:///{}",
                                        path.to_string_lossy().replace('\\', "/")
                                    ))
                                },
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn button(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    crate::ui::button(id, text)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // New sessions follow the active workspace; live shells retain their cwd.
        let directory = self.terminal_directory();
        self.terminal.update(cx, |pane, _| pane.set_directory(directory.clone()));
        let active_file = self.focused_attached_file();
        self.ai.update(cx, |pane, cx| {
            pane.set_directory(directory, cx);
            pane.set_active_file(active_file, cx);
        });
        let exports: Vec<_> = self
            .markdown_export
            .iter()
            .filter_map(|pane| pane.read(cx).export_element())
            .collect();
        for (key, pane) in &self.pdf_panes {
            let visible = !self.show_trash && (key.starts_with("reading:") || *key == self.active);
            pane.update(cx, |pane, cx| pane.set_visible(visible, window, cx));
        }
        let modal_open = self.show_trash || self.show_preferences
            || self.name_action.is_some()
            || self.dialog_prompt.is_some()
            || self.dialog_open;
        for (key, pane) in &self.html_panes {
            let visible = !modal_open && (key.starts_with("reading:") || *key == self.active);
            pane.update(cx, |pane, cx| pane.set_visible(visible, window, cx));
        }
        if let Some(pane) = &self.build_pdf {
            pane.update(cx, |pane, cx| {
                pane.set_visible(self.show_build_preview && !self.show_trash, window, cx)
            });
        }
        let engine = self.directory_workspace().map(|project| project.engine);
        let engine_app = cx.entity();
        let main_app = cx.entity();
        let main_label = self
            .directory_workspace()
            .map(|project| project.main_file.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| "选择主文件".into());
        let main_files: Vec<_> = self
            .files
            .iter()
            .filter(|file| file.path.to_lowercase().ends_with(".tex"))
            .map(|file| file.path.clone())
            .collect();
        let save_status = self
            .documents
            .get(self.focused_markdown.as_deref().unwrap_or(&self.active))
            .map(|document| {
                if document.buffer.deleted {
                    "自动保存已暂停：文件已删除"
                } else if document.buffer.conflict.is_some() {
                    "自动保存已暂停：文件冲突"
                } else if document.buffer.is_dirty() {
                    "等待自动保存…"
                } else {
                    "已保存 · 自动保存"
                }
            });
        let resize_app = cx.entity();
        let area_width = f32::from(window.viewport_size().width)
            - if self.show_terminal && !self.terminal_maximized { self.settings.terminal.panel_width } else { 0. }
            - if self.settings.sidebar_open {
                self.settings.sidebar_width + 16.
            } else {
                16.
            };
        let mut work_area = h_resizable("work-area")
            .on_resize(move |state, _, cx| {
                let sizes = state.read(cx).sizes();
                if sizes.len() >= 2 {
                    let ratio = f32::from(sizes[0]) / (f32::from(sizes[0]) + f32::from(sizes[1]));
                    resize_app.update(cx, |app, cx| {
                        app.settings.editor_ratio = ratio;
                        if let Err(error) = storage::save_settings(&app.settings) {
                            app.report(error, cx);
                        }
                    });
                }
            })
            .child(
                resizable_panel()
                    .size(px(area_width * self.settings.editor_ratio))
                    .child(self.render_editor(cx)),
            );
        if self.show_build_preview {
            work_area = work_area.child(
                resizable_panel()
                    .size(px(area_width * (1. - self.settings.editor_ratio)))
                    .child(div().relative().size_full().child(self.render_preview(cx))),
            );
        }
        for path in &self.reading_panes {
            work_area =
                work_area.child(resizable_panel().child(self.render_reading_pane(path, cx)));
        }
        let label = if self.cancellation.is_some() {
            "■  停止"
        } else {
            "▶  编译"
        };
        let status = if self.build_started.is_some() {
            format!(
                "{} · {:.1}s",
                if self
                    .cancellation
                    .as_ref()
                    .expect("running build owns cancellation")
                    .load(Ordering::Relaxed)
                {
                    "正在停止"
                } else {
                    "正在编译"
                },
                self.build_duration.as_secs_f32()
            )
        } else {
            match self.build_status {
                BuildStatus::Idle => String::new(),
                BuildStatus::Success => format!(
                    "✓ {:.1}s{}",
                    self.build_duration.as_secs_f32(),
                    if self.source_version != self.build_version {
                        " · 需重新编译"
                    } else {
                        ""
                    }
                ),
                BuildStatus::Failed => format!("失败 · {:.1}s", self.build_duration.as_secs_f32()),
                BuildStatus::Cancelled => {
                    format!("已取消 · {:.1}s", self.build_duration.as_secs_f32())
                }
            }
        };
        let mut vertical_work_area = v_resizable(if self.show_logs {
            "work-with-logs"
        } else {
            "work-no-logs"
        })
        .on_resize({
            let app = cx.entity();
            move |state, _, cx| {
                let log_height = f32::from(
                    *state
                        .read(cx)
                        .sizes()
                        .last()
                        .expect("work area has a panel"),
                );
                app.update(cx, |app, cx| {
                    if app.show_logs {
                        app.settings.log_height = log_height;
                        if let Err(error) = storage::save_settings(&app.settings) {
                            app.report(error, cx);
                        }
                    }
                });
            }
        })
        .child(resizable_panel().child(work_area));
        if self.show_logs {
            vertical_work_area = vertical_work_area.child(
                resizable_panel()
                    .size(px(self.settings.log_height))
                    .size_range(px(100.)..px(700.))
                    .fixed()
                    .child(crate::build_log::panel(self.logs.clone(), cx.listener(|app, _, _, cx| {
                        app.show_logs = false;
                        cx.notify();
                    }))),
            );
        }
        let editor_with_terminal = h_resizable(match (self.show_terminal, self.terminal_maximized) {
            (true, true) => "terminal-maximized",
            (true, false) => "editor-with-terminal",
            _ => "editor-without-terminal",
        })
            .on_resize({
                let app = cx.entity();
                move |state, _, cx| {
                    let width = state.read(cx).sizes().last().map(|size| f32::from(*size));
                    app.update(cx, |app, cx| {
                        if app.show_terminal && !app.terminal_maximized && let Some(width) = width {
                            app.settings.terminal.panel_width = width;
                            if let Err(error) = storage::save_settings(&app.settings) { app.report(error, cx); }
                        }
                    });
                }
            })
            .when(!self.show_terminal || !self.terminal_maximized, |body| {
                body.child(resizable_panel().child(div().flex().flex_col().size_full().min_w_0().min_h_0()
                    .p_2().gap_2().child(vertical_work_area)))
            })
            .when(self.show_terminal, |body| {
                let drawer_child = if self.right_drawer_tab == RightDrawerTab::Ai {
                    self.ai.clone().into_any_element()
                } else {
                    self.terminal.clone().into_any_element()
                };
                body.child(
                    resizable_panel()
                        .when(!self.terminal_maximized, |panel| {
                            panel
                                .size(px(self.settings.terminal.panel_width))
                                .size_range(px(280.)..px(1600.))
                                .fixed()
                        })
                        .child(drawer_child),
                )
            });
        let mut app = div()
            .children(exports)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(0x37352f))
            .text_size(px(self.settings.interface_font_size as f32))
            .font_family(if self.settings.interface_font.is_empty() {
                SharedString::from("Segoe UI")
            } else {
                self.settings.interface_font.clone().into()
            })
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::save_active))
            .on_action(cx.listener(Self::add_project))
            .on_action(
                cx.listener(|app, _: &OpenFile, window, cx| app.open_file_dialog(window, cx)),
            )
            .on_action(cx.listener(Self::compile))
            .on_action(cx.listener(Self::find_file))
            .on_action(cx.listener(|app, _: &Dismiss, _, cx| {
                app.show_preferences = false;
                app.name_action = None;
                app.searching = false;
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &SaveAll, _, cx| {
                app.save_all(cx);
            }))
            .on_action(cx.listener(|app, _: &CloseTab, window, cx| {
                let path = app.active.clone();
                app.close_tab(&path, window, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleSidebar, _, cx| {
                app.settings.sidebar_open = !app.settings.sidebar_open;
                if let Err(error) = storage::save_settings(&app.settings) {
                    app.report(error, cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .h(px(38.))
                    .flex_shrink_0()
                    .items_center()
                    .pl_4()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .w(px(170.))
                            .child(
                                div()
                                    .font_family("Georgia")
                                    .italic()
                                    .text_size(px(24.))
                                    .child("ƒ"),
                            )
                            .child(div().font_semibold().child("Folio")),
                    )
                    .child(
                        crate::ui::control(
                            "toggle-sidebar",
                            if self.settings.sidebar_open {
                                IconName::PanelLeftClose
                            } else {
                                IconName::PanelLeftOpen
                            },
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.settings.sidebar_open = !app.settings.sidebar_open;
                            if let Err(error) = storage::save_settings(&app.settings) {
                                app.report(error, cx);
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .id("window-drag")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_color(rgb(MUTED))
                            .text_xs()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .when(self.compile_target().is_some(), |header| {
                        header.child(
                            button("compile", label)
                                .text_color(rgb(0x4b7b59))
                                .tooltip({
                                    let target =
                                        self.compile_target().unwrap_or("主文件").to_owned();
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(format!(
                                            "编译 {target} · Ctrl+Enter"
                                        ))
                                        .build(window, cx)
                                    }
                                })
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.compile(&Compile, window, cx)
                                })),
                        )
                    })
                    .when(self.config.active_id.is_some(), |header| {
                        header.child(
                            Button::new("compile-settings")
                                .icon(crate::ui::icon(IconName::SlidersHorizontal))
                                .tooltip("编译设置")
                                .dropdown_menu(move |menu, _, _| {
                                    let menu =
                                        [Engine::Xelatex, Engine::Pdflatex, Engine::Lualatex]
                                            .into_iter()
                                            .fold(
                                                menu.item(PopupMenuItem::label("编译引擎")),
                                                |menu, choice| {
                                                    let app = engine_app.clone();
                                                    menu.item(
                                                        PopupMenuItem::new(format!("{choice:?}"))
                                                            .checked(engine == Some(choice))
                                                            .on_click(move |_, _, cx| {
                                                                app.update(cx, |app, cx| {
                                                                    app.set_engine(choice, cx)
                                                                })
                                                            }),
                                                    )
                                                },
                                            );
                                    main_files.iter().fold(
                                        menu.separator().item(PopupMenuItem::label("编译主文件")),
                                        |menu, file| {
                                            let app = main_app.clone();
                                            let file = file.clone();
                                            menu.item(
                                                PopupMenuItem::new(file.clone())
                                                    .checked(file == main_label)
                                                    .on_click(move |_, _, cx| {
                                                        app.update(cx, |app, cx| {
                                                            if app.cancellation.is_some() {
                                                                return;
                                                            }
                                                            app.config
                                                                .projects
                                                                .iter_mut()
                                                                .find(|project| {
                                                                    Some(&project.id)
                                                                        == app
                                                                            .config
                                                                            .active_id
                                                                            .as_ref()
                                                                })
                                                                .expect(
                                                                    "main file belongs to project",
                                                                )
                                                                .main_file = file.clone();
                                                            if let Err(error) =
                                                                storage::save_projects(&app.config)
                                                            {
                                                                app.report(error, cx);
                                                            }
                                                            cx.notify();
                                                        })
                                                    }),
                                            )
                                        },
                                    )
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .gap_1()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .when(self.build_started.is_some(), |status| {
                                status
                                    .w(px(140.))
                                    .text_color(rgb(crate::ui::MAIN_TEX_COLOR))
                            })
                            .child(status)
                            .when(self.build_started.is_some(), |status| {
                                // latexmk has no reliable percentage; show activity without inventing one.
                                status.child(
                                    Progress::new("build-progress")
                                        .loading(true)
                                        .accessibility_label("正在编译")
                                        .color(rgb(crate::ui::MAIN_TEX_COLOR))
                                        .h(px(3.)),
                                )
                            }),
                    )
                    .child(
                        crate::ui::control("logs", IconName::ListTree)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("编译日志")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.show_logs = !app.show_logs;
                            cx.notify();
                        })),
                    )
                    .child(
                        crate::ui::control("toggle-ai", IconName::Sparkles)
                            .when(self.show_terminal && self.right_drawer_tab == RightDrawerTab::Ai, |c| {
                                c.bg(rgb(0xe8e7e4)).text_color(rgb(0x1f2328))
                            })
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("AI 助手")
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.toggle_right_drawer(RightDrawerTab::Ai, window, cx);
                            })),
                    )
                    .child(
                        crate::ui::control("toggle-terminal", IconName::Terminal)
                            .when(self.show_terminal && self.right_drawer_tab == RightDrawerTab::Terminal, |c| {
                                c.bg(rgb(0xe8e7e4)).text_color(rgb(0x1f2328))
                            })
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("终端")
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.toggle_right_drawer(RightDrawerTab::Terminal, window, cx);
                            })),
                    )
                    .when(!self.show_build_preview, |header| {
                        header.child(button("show-build-preview", "◫").on_click(cx.listener(
                            |app, _, _, cx| {
                                app.show_build_preview = true;
                                cx.notify();
                            },
                        )))
                    })
                    .child(
                        div()
                            .flex()
                            .h_full()
                            .flex_shrink_0()
                            .child(
                                crate::ui::control("minimize", IconName::Minus)
                                    .w(px(42.))
                                    .h_full()
                                    .rounded_none()
                                    .window_control_area(WindowControlArea::Min),
                            )
                            .child(
                                crate::ui::control("maximize", IconName::Maximize2)
                                    .w(px(42.))
                                    .h_full()
                                    .rounded_none()
                                    .window_control_area(WindowControlArea::Max),
                            )
                            .child(
                                crate::ui::control("close-window", IconName::Close)
                                    .w(px(42.))
                                    .h_full()
                                    .rounded_none()
                                    .hover(|s| s.bg(rgb(0xdf5151)).text_color(crate::theme::on_accent()))
                                    .active(|s| s.bg(rgb(0xb92d2d)).text_color(crate::theme::on_accent()))
                                    .window_control_area(WindowControlArea::Close),
                            ),
                    ),
            )
            .child(
                // The resize group asks for 100% height. Constrain it to the
                // space between the title/status bars so the settings footer stays visible.
                div().flex_1().min_h_0().overflow_hidden().child(
                    h_resizable(if self.settings.sidebar_open {
                        "workspace-with-sidebar"
                    } else {
                        "workspace-no-sidebar"
                    })
                    .on_resize({
                        let app = cx.entity();
                        move |state, _, cx| {
                            let sidebar_width = f32::from(state.read(cx).sizes()[0]);
                            app.update(cx, |app, cx| {
                                if app.settings.sidebar_open {
                                    app.settings.sidebar_width = sidebar_width;
                                    if let Err(error) = storage::save_settings(&app.settings) {
                                        app.report(error, cx);
                                    }
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .when(self.settings.sidebar_open, |body| {
                        body.child(
                            resizable_panel()
                                .size(px(self.settings.sidebar_width))
                                .size_range(px(170.)..px(520.))
                                .fixed()
                                .child(self.render_sidebar(cx)),
                        )
                    })
                    .child(resizable_panel().child(if self.show_trash { self.render_trash(cx) } else { editor_with_terminal.into_any_element() })),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(24.))
                    .px_4()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        self.directory_workspace()
                            .map(|project| project.folder.clone())
                            .unwrap_or_else(|| format!("独立文件 · {} 个文件", self.tabs.len())),
                    )
                    .children(save_status)
                    .child("Ctrl+S 保存  ·  Ctrl+Shift+O 打开文件  ·  Ctrl+Enter 编译"),
            )
            .when_some(self.notice.clone(), |app, notice| {
                app.child(
                    div()
                        .id("notice-toast")
                        .absolute()
                        .bottom(px(32.))
                        .right_3()
                        .max_w(px(360.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl_3()
                        .pr_1p5()
                        .py_1p5()
                        .rounded(px(8.))
                        .bg(rgb(0xfffaf0))
                        .border_1()
                        .border_color(rgb(0xf0dec0))
                        .shadow_md()
                        .text_xs()
                        .text_color(rgb(0x5c4628))
                        .child(div().flex_1().min_w_0().child(notice))
                        .child(
                            crate::ui::control("dismiss-notice", IconName::Close)
                                .size(px(20.))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.notice = None;
                                    app.notice_until = None;
                                    cx.notify();
                                })),
                        )
                        .with_animation(
                            "notice-toast-in",
                            Animation::new(Duration::from_millis(140))
                                .with_easing(ease_out_quint()),
                            |toast, t| toast.opacity(t),
                        ),
                )
            });
        if let Some(action) = &self.name_action {
            let title = match action {
                NameAction::Create => "新建文件",
                NameAction::CreateFolder => "新建文件夹",
                NameAction::RenameFile(_) => "重命名文件",
                NameAction::RenameFolder(_) => "重命名文件夹",
                NameAction::RenameProject(_) => "重命名项目",
            };
            app = app.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x00000030))
                    .occlude()
                    .child(
                        div()
                            .w(px(420.))
                            .p_6()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(0xe6e5e1))
                            .shadow_lg()
                            .bg(rgb(0xffffff))
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(div().font_semibold().child(title))
                                    .child(
                                        crate::ui::control("close-name-dialog", IconName::Close)
                                            .on_click(cx.listener(|app, _, _, cx| {
                                                app.name_action = None;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(Input::new(&self.name_input))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap_3()
                                    .child(
                                        crate::ui::button_secondary("cancel-name", "取消")
                                            .on_click(cx.listener(|app, _, _, cx| {
                                                app.name_action = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        crate::ui::button_primary("apply-name", "确定").on_click(
                                            cx.listener(|app, _, window, cx| {
                                                if let Err(error) = app.apply_name(window, cx) {
                                                    app.report(error, cx);
                                                }
                                            }),
                                        ),
                                    ),
                            ),
                    ),
            );
        }
        if self.show_preferences {
            app = app.child(
                div()
                    .id("preferences-overlay")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x00000030))
                    // Occlude background hitboxes while leaving the dialog's
                    // child inputs and scroll container interactive.
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .relative()
                            .w(px(680.))
                            .max_h(relative(0.92))
                            .p_7()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(0xe6e5e1))
                            .shadow_lg()
                            .bg(rgb(0xffffff))
                            .child(
                                crate::ui::control("close-preferences", IconName::Close)
                                    .absolute()
                                    .top_3()
                                    .right_3()
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.show_preferences = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("preferences-content")
                                    .max_h(
                                        px(f32::from(window.viewport_size().height) * 0.92 - 56.),
                                    )
                                    .overflow_y_scroll()
                                    .child(self.preferences.clone()),
                            ),
                    ),
            );
        }
        if let Some(prompt) = self.dialog_prompt.clone() {
            // Confirm dialogs are only used for destructive actions.
            let destructive = matches!(prompt.buttons, DialogButtons::Confirm(_));
            let choices = match prompt.buttons {
                DialogButtons::SaveDiscardCancel => vec![
                    (
                        "继续编辑".to_string(),
                        rfd::MessageDialogResult::Cancel,
                        false,
                    ),
                    ("放弃修改".to_string(), rfd::MessageDialogResult::No, false),
                    ("保存".to_string(), rfd::MessageDialogResult::Yes, true),
                ],
                DialogButtons::Confirm(confirm) => vec![
                    ("取消".to_string(), rfd::MessageDialogResult::Cancel, false),
                    (confirm, rfd::MessageDialogResult::Ok, true),
                ],
                DialogButtons::RestoreConflict => vec![
                    ("跳过".to_string(), rfd::MessageDialogResult::Cancel, false),
                    ("保留两个".to_string(), rfd::MessageDialogResult::No, false),
                    ("替换".to_string(), rfd::MessageDialogResult::Yes, true),
                ],
            };
            let choices =
                choices
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, result, primary))| {
                        let id = ("prompt-choice", index);
                        let button = match (primary, destructive) {
                            (true, true) => crate::ui::button_danger(id, label),
                            (true, false) => crate::ui::button_primary(id, label),
                            (false, _) => crate::ui::button_secondary(id, label).px_3(),
                        }
                        .min_w(px(64.))
                        .on_click(cx.listener(
                            move |app, _, window, cx| app.answer_dialog(result.clone(), window, cx),
                        ));
                        button.into_any_element()
                    });
            app = app.child(crate::ui::modal_overlay(
                prompt.title,
                prompt.description,
                div().flex().justify_end().gap_2().children(choices),
            ));
        }
        app
    }
}

fn reveal_path(path: &Path, cx: &App) {
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        crate::files::reveal_path_windows(path);
    }
    #[cfg(not(target_os = "windows"))]
    {
        cx.reveal_path(path);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MarkdownLinkTarget {
    External(String),
    Local {
        path: PathBuf,
        anchor: Option<String>,
    },
    Invalid(String),
}

fn classify_markdown_link(
    source_file: &Path,
    workspace_root: Option<&Path>,
    raw_url: &str,
) -> MarkdownLinkTarget {
    let raw = raw_url.trim();
    if raw.is_empty() {
        return MarkdownLinkTarget::Invalid("空链接。".to_owned());
    }
    if let Ok(parsed) = url::Url::parse(raw) {
        if parsed.scheme().eq_ignore_ascii_case("file") {
            return match parsed.to_file_path() {
                Ok(path) => MarkdownLinkTarget::Local {
                    path,
                    anchor: parsed.fragment().map(str::to_owned),
                },
                Err(()) => MarkdownLinkTarget::Invalid(format!("无法解析本地链接：{raw}")),
            };
        }
        // Ignore 1-character schemes so Windows drive paths like `C:\...` stay local.
        if parsed.scheme().len() > 1 {
            return MarkdownLinkTarget::External(raw.to_owned());
        }
    }
    let (path_part, anchor) = match raw.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment.to_owned())),
        None => (raw, None),
    };
    let decoded = files::percent_decode(path_part);
    if decoded.is_empty() {
        return MarkdownLinkTarget::Invalid(format!("无效链接：{raw}"));
    }
    let candidate = if let Some(ws_rel) = decoded.strip_prefix('/')
        && !ws_rel.starts_with('/')
        && let Some(root) = workspace_root
    {
        let from_root = root.join(ws_rel);
        if from_root.exists() {
            from_root
        } else {
            source_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&decoded)
        }
    } else {
        let path = PathBuf::from(&decoded);
        if path.is_absolute() {
            path
        } else {
            source_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(path)
        }
    };
    MarkdownLinkTarget::Local {
        path: candidate,
        anchor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_markdown_link_handles_http_relative_and_windows_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let source = docs.join("intro.md");
        std::fs::write(&source, "# Intro").unwrap();
        std::fs::write(root.join("guide.md"), "# Guide").unwrap();

        assert_eq!(
            classify_markdown_link(&source, Some(root), "https://example.com/a?b=1"),
            MarkdownLinkTarget::External("https://example.com/a?b=1".to_owned())
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), "mailto:test@example.com"),
            MarkdownLinkTarget::External("mailto:test@example.com".to_owned())
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), "../guide.md#快速%20开始"),
            MarkdownLinkTarget::Local {
                path: docs.join("../guide.md"),
                anchor: Some("快速%20开始".to_owned()),
            }
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), "/guide.md"),
            MarkdownLinkTarget::Local {
                path: root.join("guide.md"),
                anchor: None,
            }
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), r"C:\notes\my%20file.md"),
            MarkdownLinkTarget::Local {
                path: PathBuf::from(r"C:\notes\my file.md"),
                anchor: None,
            }
        );
    }
}
