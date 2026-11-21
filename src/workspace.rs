//! Workspace orchestration: disk-backed drafts, project actions and asynchronous builds.
//! Disk revisions arbitrate external edits; conflicted drafts never autosave over them.
mod annotations;
mod agent_tags;
mod appearance;
mod build;
mod documents;
mod file_tree;
mod projects;
mod tags;
use build::Build;
use documents::{DocumentWatch, Documents, PaneKey, PaneRole};
use crate::ai::{AiPane, AiPaneEvent, AttachedFile, WorkspaceToolKind, WorkspaceToolRequest};
use gpui_kit::component::Sizable;
use crate::theme::color as rgb;
use crate::html::{HtmlPane, HtmlPaneEvent};
use crate::markdown::{MarkdownEvent, MarkdownPane};
use crate::math::MathRenderer;
use crate::pdf::{PdfPane, PdfPaneEvent};
use crate::preferences::{Preferences, SettingsChanged};
use crate::terminal::{TerminalPane, TerminalPaneEvent};
use crate::{
    files,
    model::{
        AppSettings, BuildStatus, DocumentBuffer, Engine, FileKind, InternalLinkOpenTarget,
        Project, ProjectConfig, ProjectFile, WorkspaceLocation, WorkspaceSession,
    },
    storage,
};
use gpui_kit::{
    assets::IconName,
    base::{Easing, Scrollbar, ScrollbarMode, StyledExt, Transition, transition_with_status},
    component::{
        button::{Button, ButtonVariants},
        input::{Editor, EditorState, Input, InputEvent, InputState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        progress::Progress,
        resizable::{HandleEdge, h_resizable, resizable_panel, resize_handle, v_resizable},
    },
    prelude::FluentBuilder,
    *,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
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
        ToggleAi,
        ToggleTerminal,
        FindFile,
        CloseTab,
        CopyFilePath,
        EditTags,
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

#[derive(Clone)]
struct SidebarResizeDrag;

impl Render for SidebarResizeDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct RightPanelResizeDrag;

impl Render for RightPanelResizeDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl TreeSelectionDrag {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.start.x.min(self.current.x), self.start.y.min(self.current.y)),
            point(self.start.x.max(self.current.x), self.start.y.max(self.current.y)),
        )
    }
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
        KeyBinding::new("alt-t", EditTags, Some("FolioFileTree")),
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
    // Panel shortcuts remain available inside terminal TUIs, unlike document-editing shortcuts.
    push_key(&mut bindings, settings.key_for("toggle_ai"), ToggleAi, Some("FolioWorkspace"));
    push_key(&mut bindings, settings.key_for("toggle_terminal"), ToggleTerminal, Some("FolioWorkspace"));
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
    bindings.push(KeyBinding::new(
        "ctrl-h",
        crate::markdown::ReplaceInDocument,
        Some("FolioMarkdownVisual"),
    ));
    cx.bind_keys(bindings);
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightPanelView {
    Hub,
    Ai,
    Terminal,
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
    DeleteTag,
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
    annotation_requests: BTreeSet<String>,
    tag_proposals: agent_tags::TagProposals,
    default_workspace: Project,
    config: ProjectConfig,
    workspace_sessions: BTreeMap<String, WorkspaceSession>,
    active_session: Option<String>,
    settings: AppSettings,
    background_image: Option<Arc<RenderImage>>,
    background_image_load: Option<Task<()>>,
    preferences: Entity<Preferences>,
    math: Entity<MathRenderer>,
    show_preferences: bool,
    searching: bool,
    search_input: Entity<InputState>,
    tags: tags::TagPanel,
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
    documents: Documents,
    build: Build,
    show_logs: bool,
    terminal: Entity<TerminalPane>,
    ai: Entity<AiPane>,
    right_panel_open: bool,
    right_panel_maximized: bool,
    right_panel_view: RightPanelView,
    right_panel_search: Entity<InputState>,
    right_panel_selected: usize,
    sidebar_drag: Option<(f32, f32)>,
    right_panel_drag: Option<(f32, f32)>,
    resize_save_task: Task<()>,
    preview: Option<PathBuf>,
    build_pdf: Option<Entity<PdfPane>>,
    notice: Option<String>,
    notice_until: Option<Instant>,
    name_action: Option<NameAction>,
    name_input: Entity<InputState>,
    focus: FocusHandle,
    close_requested: bool,
    dialog_response: Option<(Dialog, rfd::MessageDialogResult)>,
    dialog_prompt: Option<DialogPrompt>,
    dialog_retry: Option<Box<dyn FnOnce(&mut Self, &mut Window, &mut Context<Self>)>>,
    dialog_open: bool,
    editor_zoom_hud: crate::ui::ZoomHud,
    _tick: Task<()>,
    _history_escape: Subscription,
}

impl Workspace {
    pub fn new(settings: AppSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let math = cx.new(MathRenderer::new);
        math.update(cx, |renderer, cx| renderer.set_appearance(&settings, cx));
        let preferences = cx.new(|cx| Preferences::new(settings.clone(), window, cx));
        let terminal = cx.new(|cx| TerminalPane::new(settings.terminal.clone(), settings.theme.palette, cx));
        cx.subscribe_in(&terminal, window, |app, _, event: &TerminalPaneEvent, window, cx| {
            match event {
                TerminalPaneEvent::BackToHub => {
                    app.open_right_panel(RightPanelView::Hub, window, cx);
                    return;
                }
                TerminalPaneEvent::ToggleMaximize => {
                    app.right_panel_maximized = !app.right_panel_maximized;
                }
            }
            cx.notify();
        })
        .detach();
        let initial_dir = dirs::home_dir().expect("user home");
        let ai = cx.new(|cx| AiPane::new(initial_dir, settings.clone(), math.clone(), window, cx));
        cx.subscribe_in(&ai, window, |app, _, event: &AiPaneEvent, window, cx| match event {
            AiPaneEvent::WorkspaceTool(request) => match request.kind {
                WorkspaceToolKind::AnnotationRequest | WorkspaceToolKind::AnnotationCancel => {
                    app.annotation_request(request, window, cx)
                }
                WorkspaceToolKind::TagRequest | WorkspaceToolKind::TagCancel => app.tag_request(request, cx),
            },
            AiPaneEvent::UndoTags(items) => app.undo_tag_confirmations(items, cx),
            AiPaneEvent::OpenFile(path) => {
                app.open_path_from_ai(path, window, cx);
            }
            AiPaneEvent::BackToHub => {
                app.open_right_panel(RightPanelView::Hub, window, cx);
            }
            AiPaneEvent::ToggleMaximize => {
                app.right_panel_maximized = !app.right_panel_maximized;
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&preferences, window, |app, _, event: &SettingsChanged, window, cx| {
            app.apply_settings(event.apply_to(&app.settings), window, cx);
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
        cx.subscribe(&search_input, |app, _, _: &InputEvent, cx| {
            if app.tags.filter.is_some() {
                app.prune_tag_selection(cx);
            }
            cx.notify();
        })
            .detach();
        let right_panel_search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::workspace::SEARCH_HUB_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &right_panel_search,
            window,
            |app, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change => {
                        app.right_panel_selected = 0;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        if app.right_panel_open && app.right_panel_view == RightPanelView::Hub {
                            let props = app.hub_props(cx);
                            let items = crate::right_panel::filtered_actions(&props);
                            if let Some(item) = items
                                .get(app.right_panel_selected.min(items.len().saturating_sub(1)))
                            {
                                let action = item.action.clone();
                                app.execute_right_panel_action(action, window, cx);
                            }
                        }
                    }
                    _ => {}
                }
            },
        )
        .detach();
        let tags = tags::TagPanel::new(window, cx);
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
        let this = cx.entity().downgrade();
        let workspace_window = window.window_handle();
        // Handle Escape for overlays before their focused inputs consume it.
        let history_escape = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() == workspace_window
                && event.keystroke.modifiers.number_of_modifiers() == 0
            {
                let key = event.keystroke.key.as_str();
                if matches!(key, "escape" | "up" | "down") {
                    let _ = this.update(cx, |app, cx| {
                        if key == "escape" && app.tags.is_editing() {
                            app.close_tag_panel(window, cx);
                            cx.stop_propagation();
                            return;
                        }
                        if app.right_panel_open
                            && app.right_panel_view == RightPanelView::Hub
                            && !app.show_preferences
                        {
                            let hub_focused = app
                                .right_panel_search
                                .read(cx)
                                .focus_handle(cx)
                                .is_focused(window);
                            if hub_focused {
                                match key {
                                    "up" => {
                                        let len = crate::right_panel::filtered_actions(
                                            &app.hub_props(cx),
                                        )
                                        .len();
                                        if len > 0 {
                                            app.right_panel_selected =
                                                if app.right_panel_selected == 0 {
                                                    len - 1
                                                } else {
                                                    app.right_panel_selected - 1
                                                };
                                            cx.stop_propagation();
                                            cx.notify();
                                        }
                                        return;
                                    }
                                    "down" => {
                                        let len = crate::right_panel::filtered_actions(
                                            &app.hub_props(cx),
                                        )
                                        .len();
                                        if len > 0 {
                                            app.right_panel_selected =
                                                (app.right_panel_selected + 1) % len;
                                            cx.stop_propagation();
                                            cx.notify();
                                        }
                                        return;
                                    }
                                    "escape" => {
                                        if !app.right_panel_search.read(cx).value().is_empty() {
                                            app.right_panel_search.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                            });
                                            app.right_panel_selected = 0;
                                        } else {
                                            app.close_right_panel(window, cx);
                                        }
                                        cx.stop_propagation();
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if key == "escape"
                            && !app.show_preferences
                            && app.right_panel_open
                            && app.right_panel_view == RightPanelView::Ai
                            && app.ai.update(cx, |pane, cx| pane.dismiss_history(window, cx))
                        {
                            cx.stop_propagation();
                        }
                    });
                }
            }
        });
        let mut app = Self {
            annotation_requests: BTreeSet::new(),
            tag_proposals: Default::default(),
            default_workspace,
            config,
            workspace_sessions,
            active_session: None,
            settings,
            background_image: None,
            background_image_load: None,
            preferences,
            math,
            show_preferences: false,
            searching: false,
            search_input,
            tags,
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
            documents: Documents::default(),
            build: Build::default(),
            show_logs: false,
            terminal,
            ai,
            right_panel_open: false,
            right_panel_maximized: false,
            right_panel_view: RightPanelView::Hub,
            right_panel_search,
            right_panel_selected: 0,
            sidebar_drag: None,
            right_panel_drag: None,
            resize_save_task: Task::ready(()),
            preview: None,
            build_pdf: None,
            notice: None,
            notice_until: None,
            name_action: None,
            name_input,
            focus: cx.focus_handle(),
            close_requested: false,
            dialog_response: None,
            dialog_prompt: None,
            dialog_retry: None,
            dialog_open: false,
            editor_zoom_hud: crate::ui::ZoomHud::default(),
            _tick: tick,
            _history_escape: history_escape,
        };
        if app.settings.background_image.is_some() {
            app.load_background_image(window, cx);
        }
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

    fn load_background_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping the retained task prevents an older selection from publishing after replacement.
        self.background_image_load = None;
        if self.background_image.is_some() {
            self.set_background_image(None, window, cx);
        }
        self.preferences.update(cx, |panel, cx| panel.set_background_image_error(None, cx));
        let Some(path) = self.settings.background_image.clone() else {
            return;
        };
        let load = ImageAssetLoader::load(path.into(), cx);
        let load = cx.background_executor().spawn(load);
        self.background_image_load = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |app, window, cx| match result {
                Ok(image) => app.set_background_image(Some(image), window, cx),
                Err(error) => app.preferences.update(cx, |panel, cx| {
                    panel.set_background_image_error(
                        Some(crate::i18n::workspace::err_read_background_image(&error)),
                        cx,
                    );
                }),
            });
        }));
    }

    fn has_visible_background(&self) -> bool {
        self.background_image.is_some() && self.settings.background_image_opacity > 0
    }

    fn sync_backdrop_visible(&self, cx: &mut Context<Self>) {
        let visible = self.has_visible_background();
        for pane in self.documents.markdown_panes.values() {
            pane.update(cx, |pane, cx| pane.set_backdrop_visible(visible, cx));
        }
    }

    fn set_background_image(
        &mut self,
        image: Option<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = std::mem::replace(&mut self.background_image, image) {
            cx.drop_image(previous, Some(window));
        }
        // Document surfaces follow the decoded image only when its opacity is non-zero.
        self.sync_backdrop_visible(cx);
        cx.notify();
    }

    fn is_right_panel_open(&self) -> bool {
        self.right_panel_open
    }

    fn is_sidebar_open(&self) -> bool {
        self.settings.sidebar_open
    }

    fn schedule_resize_save(&mut self, cx: &mut Context<Self>) {
        self.resize_save_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.persist_settings(cx);
            });
        });
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.settings.sidebar_open = !self.settings.sidebar_open;
        self.persist_settings(cx);
        cx.notify();
    }

    fn open_right_panel(
        &mut self,
        view: RightPanelView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_panel_open = true;
        self.right_panel_maximized = false;
        self.right_panel_view = view;

        let directory = self.terminal_directory();
        match view {
            RightPanelView::Hub => {
                self.terminal.update(cx, |pane, _| pane.hide());
                self.right_panel_selected = 0;
                self.right_panel_search.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            RightPanelView::Terminal => {
                self.terminal
                    .update(cx, |pane, cx| pane.open(directory, window, cx));
            }
            RightPanelView::Ai => {
                self.terminal.update(cx, |pane, _| pane.hide());
                let project_name = self.active_project_name();
                self.ai.update(cx, |pane, cx| {
                    pane.set_directory(directory, cx);
                    pane.set_project_name(project_name, cx);
                    pane.open(window, cx);
                });
            }
        }
        cx.notify();
    }

    fn close_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.right_panel_open {
            return;
        }
        self.right_panel_open = false;
        self.right_panel_maximized = false;
        self.terminal.update(cx, |pane, _| pane.hide());
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn toggle_right_shortcut(
        &mut self,
        view: RightPanelView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_right_panel_open() && self.right_panel_view == view {
            self.close_right_panel(window, cx);
        } else {
            self.open_right_panel(view, window, cx);
        }
    }

    fn hub_props(&self, cx: &App) -> crate::right_panel::RightPanelHubProps {
        crate::right_panel::RightPanelHubProps {
            query: self.right_panel_search.read(cx).value().to_string(),
            selected_index: self.right_panel_selected,
            preview_open: self.show_build_preview,
            logs_open: self.show_logs,
            build_running: self.build.is_running(),
            build_failed: matches!(self.build.status, BuildStatus::Failed),
            build_target: self.build.target.clone(),
            ai_shortcut: AppSettings::format_key_badge(&self.settings.key_for("toggle_ai")),
            terminal_shortcut: AppSettings::format_key_badge(
                &self.settings.key_for("toggle_terminal"),
            ),
            terminal_sessions: self.terminal.read(cx).session_count(),
            ai_tasks: self.ai.read(cx).active_tasks(),
        }
    }

    fn execute_right_panel_action(
        &mut self,
        action: crate::right_panel::RightPanelAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::right_panel::RightPanelAction;
        match action {
            RightPanelAction::OpenAi => {
                self.open_right_panel(RightPanelView::Ai, window, cx);
            }
            RightPanelAction::OpenTerminal => {
                self.open_right_panel(RightPanelView::Terminal, window, cx);
            }
            RightPanelAction::TogglePreview => {
                self.show_build_preview = !self.show_build_preview;
                cx.notify();
            }
            RightPanelAction::ToggleLogs => {
                self.show_logs = !self.show_logs;
                cx.notify();
            }
            RightPanelAction::OpenFile => {
                self.open_file_dialog(window, cx);
            }
            RightPanelAction::OpenPreferences => {
                self.preferences.update(cx, |panel, cx| {
                    panel.sync(&self.settings, window, cx);
                });
                self.show_preferences = true;
                cx.notify();
            }
            RightPanelAction::ActivateAiTask(id) => {
                self.open_right_panel(RightPanelView::Ai, window, cx);
                self.ai.update(cx, |pane, cx| pane.activate(id, cx));
            }
        }
    }

    fn focused_attached_file(&self) -> Option<AttachedFile> {
        let clean_key = self.documents.focused_markdown.as_deref().unwrap_or(&self.documents.active);
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
        self.open_right_panel(RightPanelView::Ai, window, cx);
        let directory = self.terminal_directory();
        let attached = file_path.map(|path| AttachedFile {
            display: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| source.clone()),
            path,
        });
        let project_name = self.active_project_name();
        self.ai.update(cx, |pane, cx| {
            pane.set_directory(directory, cx);
            pane.set_project_name(project_name, cx);
            pane.attach_selection(source, attached, text, window, cx);
        });
        cx.notify();
    }

    fn open_path_from_ai(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(resolved) = dunce::canonicalize(path) else {
            self.report(crate::i18n::workspace::err_file_not_found(path), cx);
            return;
        };
        if !resolved.is_file() || !files::is_relevant(&resolved) {
            self.report(crate::i18n::workspace::err_unsupported_open(&resolved), cx);
            return;
        }
        if let Some(project) = self.directory_workspace()
            && let Ok(canonical_root) = dunce::canonicalize(&project.folder)
            && let Ok(relative) = resolved.strip_prefix(&canonical_root)
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
        if let Err(error) = self.open_standalone_file(&resolved, window, cx) {
            self.report(error, cx);
        }
    }

    fn report(&mut self, error: impl std::fmt::Display, cx: &mut Context<Self>) {
        self.notice = Some(error.to_string());
        self.notice_until = Some(Instant::now() + Duration::from_millis(2000));
        cx.notify();
    }
    fn export_pdf(&mut self, source: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(crate::i18n::workspace::DIALOG_EXPORT_PDF.get())
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
        if !self.documents.open
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
            crate::i18n::workspace::DIALOG_UNSAVED_ALL_TITLE.get().into(),
            crate::i18n::workspace::DIALOG_UNSAVED_ALL_BODY.get().into(),
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
        if self.build.is_running() {
            // Let the build worker terminate latexmk's child process tree before exit.
            self.build.cancel();
            self.close_requested = true;
            return false;
        }
        self.release_all_html_panes(window, cx);
        true
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_trash(cx);
        if self
            .notice_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.notice = None;
            self.notice_until = None;
            cx.notify();
        }
        self.poll_documents(window, cx);
        if self.poll_build(window, cx) {
            return;
        }
        for pane in self.documents.html_panes.values() {
            pane.update(cx, |pane, cx| pane.poll(window, cx));
        }
    }

    fn set_engine(&mut self, engine: Engine, cx: &mut Context<Self>) {
        if self.build.is_running() {
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

    fn render_editor_tab_bar(&self, show_background: bool, cx: &mut Context<Self>) -> AnyElement {
        let fg_alpha = |hex: u32, alpha: f32| -> Rgba {
            let r = ((hex >> 16) & 0xff) as f32 / 255.0;
            let g = ((hex >> 8) & 0xff) as f32 / 255.0;
            let b = (hex & 0xff) as f32 / 255.0;
            Rgba { r, g, b, a: alpha }
        };
        let palette = crate::theme::palette();
        let header_bg = if palette.is_dark() {
            palette.background
        } else {
            palette.hover
        };
        let mut bar = div()
            .h(px(32.))
            .px_1p5()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .when(!show_background, |bar| bar.bg(gpui_kit::rgb(header_bg)))
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border));

        if let Some(md_pane) = self.documents.markdown_panes.get(&PaneKey::tab(&self.documents.active)) {
            let outline_open = md_pane.read(cx).is_outline_open();
            let outline_pane = md_pane.clone();
            bar = bar.child(
                Button::new("md-tab-outline")
                    .ghost()
                    .compact()
                    .with_size(px(16.))
                    .w(px(22.))
                    .h(px(22.))
                    .when(outline_open, |b| b.bg(fg_alpha(palette.text, 0.08)))
                    .icon(crate::ui::icon(IconName::ListTree))
                    .tooltip(if outline_open {
                        crate::i18n::workspace::TOOLTIP_COLLAPSE_OUTLINE.get()
                    } else {
                        crate::i18n::workspace::TOOLTIP_EXPAND_OUTLINE.get()
                    })
                    .on_click(move |_, window, cx| {
                        outline_pane.update(cx, |pane, cx| {
                            pane.toggle_outline(cx);
                            if pane.is_outline_open() && !pane.is_focused(window, cx) {
                                pane.focus(window, cx);
                            }
                        });
                    }),
            );
        }

        bar = bar.child(
            div()
                .id("editor-tabs")
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .overflow_hidden()
                .children(self.documents.tabs.iter().map(|path| {
                    let current = self.documents.active == *path;
                    let title = Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    let switch_path = path.clone();
                    let close_path = path.clone();
                    let tooltip_path = path.clone();
                    let tab_hover_bg = fg_alpha(palette.text, 0.07);
                    div()
                        .id(SharedString::from(format!("editor-tab-{path}")))
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
                            s.text_color(gpui_kit::rgb(palette.text))
                                .when(!show_background, |s| s.bg(gpui_kit::rgb(palette.surface)))
                        })
                        .when(!current || show_background, |s| {
                            s.hover(move |h| {
                                h.bg(tab_hover_bg)
                                    .text_color(gpui_kit::rgb(palette.text))
                            })
                        })
                        .when(show_background, |tab| {
                            tab.focusable().tab_stop(true).focus_visible(move |s| s.bg(tab_hover_bg))
                        })
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tooltip_path.clone())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.activate_tab(&switch_path, window, cx);
                        }))
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
                            Button::new(SharedString::from(format!("close-editor-tab-{path}")))
                                .ghost()
                                .compact()
                                .with_size(px(16.))
                                .w(px(18.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .icon(crate::ui::icon(IconName::Close))
                                .tooltip(crate::i18n::workspace::TOOLTIP_CLOSE_TAB.get())
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_tab(&close_path, window, cx);
                                })),
                        )
                        .into_any_element()
                })),
        );

        if let Some(md_pane) = self.documents.markdown_panes.get(&PaneKey::tab(&self.documents.active)) {
            let count = md_pane.read(cx).annotation_count();
            if count > 0 {
                let drawer_pane = md_pane.clone();
                bar = bar.child(
                    div()
                        .id("md-annotations-pill")
                        .px_2()
                        .h(px(22.))
                        .rounded_full()
                        .bg(rgb(0xfef9c3))
                        .border_1()
                        .border_color(rgb(0xfde047))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xfef08a)))
                        .active(|s| s.bg(rgb(0xfde047)))
                        .text_xs()
                        .text_color(rgb(0x854d0e))
                        .child(crate::ui::icon(IconName::MessageSquareText))
                        .child(format!("{count}"))
                        .on_click(move |_, _, cx| {
                            drawer_pane.update(cx, |pane, cx| pane.toggle_annotations_drawer(cx));
                        }),
                );
            }
            let copy_pane = md_pane.clone();
            let source_pane = md_pane.clone();
            let is_source = md_pane.read(cx).is_source();
            bar = bar
                .child(crate::ui::copy_document_button(
                    SharedString::from(format!("md-copy-document:{}", self.documents.active)),
                    move |cx| copy_pane.read(cx).source_text(),
                ))
                .child(
                    Button::new("md-tab-source")
                        .ghost()
                        .compact()
                        .with_size(px(16.))
                        .w(px(22.))
                        .h(px(22.))
                        .when(is_source, |b| b.bg(fg_alpha(palette.text, 0.08)))
                        .icon(crate::ui::icon(IconName::CodeXml))
                        .tooltip(if is_source {
                            crate::i18n::workspace::TOOLTIP_SWITCH_WYSIWYG.get()
                        } else {
                            crate::i18n::workspace::TOOLTIP_SWITCH_SOURCE.get()
                        })
                        .on_click(move |_, window, cx| {
                            source_pane.update(cx, |pane, cx| pane.toggle_source(window, cx));
                        }),
                );
        } else if let Some(document) = self.documents.open.get(&self.documents.active) {
            let editor = document.editor.clone();
            let ask_editor = document.editor.clone();
            let ask_path = document.path.clone();
            let ask_source = document
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| crate::i18n::workspace::SOURCE_LABEL.get().to_owned());
            bar = bar
                .child(
                    Button::new("ask-ai-source")
                        .ghost()
                        .compact()
                        .with_size(px(16.))
                        .w(px(22.))
                        .h(px(22.))
                        .icon(crate::ui::icon(IconName::Sparkles))
                        .tooltip(crate::i18n::workspace::TOOLTIP_SEND_TO_AI.get())
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
                ));
        }

        bar.into_any_element()
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let document = self.documents.open.get(&self.documents.active);
        let show_background = self.has_visible_background()
            && (self.documents.active.is_empty() || document.is_some());
        let mut area = div()
            .flex()
            .flex_col()
            .size_full()
            .when(!show_background, |area| area.bg(rgb(0xffffff)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.documents.focused_markdown = None),
            );
        if !self.documents.tabs.is_empty() {
            area = area.child(self.render_editor_tab_bar(show_background, cx));
        }
        if let Some(document) = document {
            if document.buffer.deleted {
                area = area.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0xfff4e5))
                        .child(crate::i18n::workspace::BANNER_FILE_DELETED.get()),
                );
            } else if document.buffer.conflict.is_some() {
                let reload = self.documents.active.clone();
                let overwrite = self.documents.active.clone();
                area =
                    area.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_2()
                            .bg(rgb(0xfff4e5))
                            .child(crate::i18n::workspace::BANNER_DISK_MODIFIED.get())
                            .child(button("load-disk", crate::i18n::workspace::BTN_LOAD_DISK.get()).on_click(cx.listener(
                                move |app, _, window, cx| app.reload_disk(&reload, window, cx),
                            )))
                            .child(button("overwrite-disk", crate::i18n::workspace::BTN_OVERWRITE_DISK.get()).on_click(
                                cx.listener(move |app, _, _, cx| {
                                    if let Err(error) = app.save_file(&overwrite, true, cx) {
                                        app.report(error, cx);
                                    }
                                }),
                            )),
                    );
            }
            if let Some(markdown) = self.documents.markdown_panes.get(&PaneKey::tab(&self.documents.active)) {
                area = area.child(div().flex_1().min_h_0().child(markdown.clone()));
            } else {
                area = area.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .p_2()
                        .flex()
                        .justify_center()
                        .child(crate::ui::ctrl_scroll_capture(cx.listener(|app, event: &ScrollEvent, window, cx| {
                            if event.modifiers.control {
                                let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                                    1
                                } else {
                                    -1
                                };
                                let mut settings = app.settings.clone();
                                settings.editor_font_size =
                                    (settings.editor_font_size as i16 + delta).clamp(11, 26) as u8;
                                app.apply_settings(settings, window, cx);
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
        } else if !self.documents.active.is_empty() {
            let full = self.document_path(&self.documents.active);
            let tab = PaneKey::tab(&self.documents.active);
            if let Some(pane) = self.documents.html_panes.get(&tab) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else if let Some(pane) = self.documents.pdf_panes.get(&tab) {
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
                        crate::i18n::workspace::EMPTY_LOOSE_HINT.get()
                    } else {
                        crate::i18n::workspace::EMPTY_PROJECT_HINT.get()
                    })
                    .when(self.directory_workspace().is_none(), |area| area.child(button("welcome-open-file", crate::i18n::workspace::OPEN_FILE.get()).on_click(
                        cx.listener(|app, _, window, cx| app.open_file_dialog(window, cx)),
                    )))
                    .child(if self.directory_workspace().is_none() {
                        crate::i18n::workspace::SHORTCUTS_BAR_LOOSE.get()
                    } else {
                        crate::i18n::workspace::SHORTCUTS_BAR_PROJECT.get()
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
                        .child(crate::i18n::workspace::COMPILE_PREVIEW_TITLE.get())
                        .child(div().flex_1())
                        .when_some(self.preview.clone(), |bar, path| {
                            bar.child(button("export-build-pdf", crate::i18n::workspace::EXPORT_BTN.get()).on_click(cx.listener(
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
                            .child(crate::i18n::workspace::PREVIEW_TITLE.get()),
                    )
                    .when_some(self.preview.clone(), |toolbar, path| {
                        toolbar.child(
                            button("reveal-output", crate::i18n::workspace::MENU_REVEAL_IN_EXPLORER.get())
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
                    .child(if self.build.is_busy() {
                        crate::i18n::workspace::STATUS_COMPILING.get()
                    } else if self.preview.is_some() {
                        crate::i18n::workspace::STATUS_COMPILE_OK.get()
                    } else {
                        crate::i18n::workspace::STATUS_COMPILE_IDLE.get()
                    })
                    .when_some(self.preview.clone(), |area, path| {
                        area.child(
                            crate::ui::button_secondary("open-output", crate::i18n::workspace::BTN_OPEN_OUTPUT_PDF.get()).on_click(
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
        let project_name = self.active_project_name();
        self.ai.update(cx, |pane, cx| {
            pane.set_directory(directory, cx);
            pane.set_project_name(project_name, cx);
            pane.set_active_file(active_file, cx);
        });
        let exports: Vec<_> = self
            .build
            .capture()
            .into_iter()
            .filter_map(|pane| pane.read(cx).export_element())
            .collect();
        for (key, pane) in &self.documents.pdf_panes {
            let visible = !self.show_trash && (key.role == PaneRole::Reading || key.path == self.documents.active);
            pane.update(cx, |pane, cx| pane.set_visible(visible, window, cx));
        }
        let modal_open = self.show_trash || self.show_preferences
            || self.tags.is_editing()
            || self.name_action.is_some()
            || self.dialog_prompt.is_some()
            || self.dialog_open;
        for (key, pane) in &self.documents.html_panes {
            let visible = !modal_open && (key.role == PaneRole::Reading || key.path == self.documents.active);
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
            .unwrap_or_else(|| crate::i18n::workspace::SELECT_MAIN_FILE.get().into());
        let main_files: Vec<_> = self
            .files
            .iter()
            .filter(|file| file.path.to_lowercase().ends_with(".tex"))
            .map(|file| file.path.clone())
            .collect();
        let resize_app = cx.entity();
        let right_panel_visible = self.is_right_panel_open();
        let sidebar_visible = self.is_sidebar_open();
        let sidebar_target_w = self.settings.sidebar_width.clamp(170., 520.);
        let right_target_w = self.settings.terminal.panel_width.clamp(280., 1600.);
        let drawer_curve = Easing::CubicBezier {
            x1: 0.25,
            y1: 0.9,
            x2: 0.32,
            y2: 1.0,
        };
        let sidebar_policy = Transition::new(Duration::from_millis(220)).easing(drawer_curve.clone());
        let right_duration_ms = (200.0 + right_target_w * 0.16).clamp(240.0, 320.0) as u64;
        let right_policy =
            Transition::new(Duration::from_millis(right_duration_ms)).easing(drawer_curve);

        let sidebar_progress = transition_with_status(
            "workspace-left-sidebar-motion",
            if sidebar_visible { 1.0_f32 } else { 0.0_f32 },
            sidebar_policy,
            window,
            cx,
        )
        .value
        .clamp(0.0, 1.0);
        let sidebar_rendered = sidebar_visible || sidebar_progress > 0.001;
        let sidebar_now_w = sidebar_target_w * sidebar_progress;

        let right_progress = transition_with_status(
            "workspace-right-panel-motion",
            if right_panel_visible && !self.right_panel_maximized {
                1.0_f32
            } else {
                0.0_f32
            },
            right_policy,
            window,
            cx,
        )
        .value
        .clamp(0.0, 1.0);
        let right_rendered = right_panel_visible || right_progress > 0.001;
        let right_now_w = if right_panel_visible && self.right_panel_maximized {
            0.0
        } else {
            right_target_w * right_progress
        };
        let area_width =
            (f32::from(window.viewport_size().width) - right_now_w - sidebar_now_w - 16.)
                .max(200.0);
        let mut work_area = h_resizable("work-area")
            .on_resize(move |state, _, cx| {
                let sizes = state.read(cx).sizes();
                if sizes.len() >= 2 {
                    let ratio = f32::from(sizes[0]) / (f32::from(sizes[0]) + f32::from(sizes[1]));
                    resize_app.update(cx, |app, cx| {
                        app.settings.editor_ratio = ratio;
                        app.persist_settings(cx);
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
        for path in &self.documents.reading_panes {
            work_area =
                work_area.child(resizable_panel().child(self.render_reading_pane(path, cx)));
        }
        let label = if self.build.is_busy() {
            crate::i18n::workspace::BTN_STOP_BUILD.get()
        } else {
            crate::i18n::workspace::BTN_COMPILE_BUILD.get()
        };
        let status = if self.build.is_running() {
            format!(
                "{} · {:.1}s",
                if self.build.is_cancelling() {
                    crate::i18n::workspace::STATUS_STOPPING.get()
                } else {
                    crate::i18n::workspace::STATUS_BUILDING.get()
                },
                self.build.duration.as_secs_f32()
            )
        } else {
            match self.build.status {
                BuildStatus::Idle => String::new(),
                BuildStatus::Success => format!(
                    "✓ {:.1}s{}",
                    self.build.duration.as_secs_f32(),
                    if self.documents.source_version != self.build.source_version {
                        crate::i18n::workspace::STATUS_NEEDS_REBUILD.get()
                    } else {
                        ""
                    }
                ),
                BuildStatus::Failed => {
                    crate::i18n::workspace::status_failed_secs(self.build.duration.as_secs_f32())
                }
                BuildStatus::Cancelled => {
                    crate::i18n::workspace::status_cancelled_secs(self.build.duration.as_secs_f32())
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
                        app.persist_settings(cx);
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
                    .child(crate::build_log::panel(self.build.logs.clone(), cx.listener(|app, _, _, cx| {
                        app.show_logs = false;
                        cx.notify();
                    }))),
            );
        }
        let right_panel_content = right_rendered.then(|| match self.right_panel_view {
            RightPanelView::Hub => {
                let props = self.hub_props(cx);
                let action_app = cx.entity();
                let clear_app = cx.entity();
                crate::right_panel::render_hub(
                    &props,
                    &self.right_panel_search,
                    self.settings.theme.palette,
                    window,
                    cx,
                    move |action, window, cx| {
                        action_app.update(cx, |app, cx| {
                            app.execute_right_panel_action(action, window, cx);
                        });
                    },
                    move |window, cx| {
                        clear_app.update(cx, |app, cx| {
                            app.right_panel_search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                            app.right_panel_selected = 0;
                            cx.notify();
                        });
                    },
                )
            }
            RightPanelView::Ai => self.ai.clone().into_any_element(),
            RightPanelView::Terminal => self.terminal.clone().into_any_element(),
        });

        let maximized = right_panel_visible && self.right_panel_maximized;
        let editor_with_terminal: AnyElement = if maximized {
            div()
                .flex()
                .size_full()
                .min_w_0()
                .min_h_0()
                .children(right_panel_content)
                .into_any_element()
        } else {
            div()
                .id("editor-with-right-panel")
                .relative()
                .flex()
                .size_full()
                .min_w_0()
                .min_h_0()
                .on_drag_move(cx.listener(
                    |app, event: &DragMoveEvent<RightPanelResizeDrag>, _, cx| {
                        let Some((start_x, start_w)) = app.right_panel_drag else {
                            return;
                        };
                        let delta = f32::from(event.event.position.x) - start_x;
                        let next_w = (start_w - delta).clamp(280., 1600.).round();
                        if (app.settings.terminal.panel_width - next_w).abs() >= 1.0 {
                            app.settings.terminal.panel_width = next_w;
                            app.schedule_resize_save(cx);
                            cx.notify();
                        }
                    },
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(vertical_work_area),
                )
                .when(right_rendered, |row| {
                    row.child(
                        div()
                            .relative()
                            .w(px(right_now_w))
                            .h_full()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .relative()
                                    .w(px(right_target_w))
                                    .h_full()
                                    .opacity(0.6 + 0.4 * right_progress)
                                    .children(right_panel_content),
                            )
                            .when(right_panel_visible && right_progress >= 0.999, |panel| {
                                let drag_app = cx.entity();
                                panel.child(
                                    resize_handle("right-panel-resize", Axis::Horizontal)
                                        .inside(HandleEdge::Leading)
                                        .on_drag(RightPanelResizeDrag, move |_, _, window, cx| {
                                            let start_x = f32::from(window.mouse_position().x);
                                            drag_app.update(cx, |app, _| {
                                                app.right_panel_drag = Some((
                                                    start_x,
                                                    app.settings
                                                        .terminal
                                                        .panel_width
                                                        .clamp(280., 1600.),
                                                ));
                                            });
                                            cx.new(|_| RightPanelResizeDrag)
                                        }),
                                )
                            }),
                    )
                })
                .into_any_element()
        };
        let show_backdrop = self.has_visible_background();
        let mut app = div()
            .children(exports)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(if show_backdrop {
                gpui_kit::rgb(self.settings.theme.palette.surface)
            } else {
                rgb(BG)
            })
            // Paint once at window coordinates so text panes share the same crop as they scroll.
            .when(show_backdrop, |app| {
                app.when_some(self.background_image.clone(), |app, image| {
                    app.child(
                        img(image)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .opacity(f32::from(self.settings.background_image_opacity) / 100.),
                    )
                })
            })
            .text_color(rgb(0x37352f))
            .text_size(px(14.))
            .font_family(crate::ui::sans_font(&self.settings.interface_font))
            .track_focus(&self.focus)
            .key_context("FolioWorkspace")
            .on_action(cx.listener(|app, _: &ToggleAi, window, cx| {
                if app.show_preferences {
                    cx.propagate();
                    return;
                }
                app.toggle_right_shortcut(RightPanelView::Ai, window, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleTerminal, window, cx| {
                if app.show_preferences {
                    cx.propagate();
                    return;
                }
                app.toggle_right_shortcut(RightPanelView::Terminal, window, cx);
            }))
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
                let path = app.documents.active.clone();
                app.close_tab(&path, window, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleSidebar, _, cx| {
                app.toggle_sidebar(cx);
            }))
            .child(
                div()
                    .flex()
                    .h(px(38.))
                    .flex_shrink_0()
                    .items_center()
                    .pl_4()
                    .gap_2()
                    .bg(rgb(BG))
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
                            if sidebar_visible {
                                IconName::PanelLeftClose
                            } else {
                                IconName::PanelLeftOpen
                            },
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.toggle_sidebar(cx);
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
                                    let target = self
                                        .compile_target()
                                        .unwrap_or_else(|| crate::i18n::workspace::MAIN_FILE_FALLBACK.get())
                                        .to_owned();
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            crate::i18n::workspace::tooltip_compile_target(&target),
                                        )
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
                                .tooltip(crate::i18n::workspace::TOOLTIP_BUILD_SETTINGS.get())
                                .dropdown_menu(move |menu, _, _| {
                                    let menu =
                                        [Engine::Xelatex, Engine::Pdflatex, Engine::Lualatex]
                                            .into_iter()
                                            .fold(
                                                menu.item(PopupMenuItem::label(
                                                    crate::i18n::workspace::LABEL_COMPILE_ENGINE.get(),
                                                )),
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
                                        menu.separator().item(PopupMenuItem::label(
                                            crate::i18n::workspace::LABEL_MAIN_FILE.get(),
                                        )),
                                        |menu, file| {
                                            let app = main_app.clone();
                                            let file = file.clone();
                                            menu.item(
                                                PopupMenuItem::new(file.clone())
                                                    .checked(file == main_label)
                                                    .on_click(move |_, _, cx| {
                                                        app.update(cx, |app, cx| {
                                                            if app.build.is_running() {
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
                            .when(self.build.is_running(), |status| {
                                status
                                    .w(px(140.))
                                    .text_color(rgb(crate::ui::MAIN_TEX_COLOR))
                            })
                            .child(status)
                            .when(self.build.is_running(), |status| {
                                // latexmk has no reliable percentage; show activity without inventing one.
                                status.child(
                                    Progress::new("build-progress")
                                        .loading(true)
                                        .accessibility_label(
                                            crate::i18n::workspace::STATUS_BUILDING.get(),
                                        )
                                        .color(rgb(crate::ui::MAIN_TEX_COLOR))
                                        .h(px(3.)),
                                )
                            }),
                    )
                    .child(
                        crate::ui::control("toggle-right-panel", IconName::PanelRightOpen)
                            .when(right_panel_visible, |c| {
                                let palette = self.settings.theme.palette;
                                c.bg(if palette.is_dark() {
                                    gpui_kit::rgb(palette.hover)
                                } else {
                                    rgb(0xe8e7e4)
                                })
                                .text_color(gpui_kit::rgb(palette.text))
                            })
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if right_panel_visible {
                                    crate::i18n::workspace::TOOLTIP_COLLAPSE_RIGHT_HUB.get()
                                } else {
                                    crate::i18n::workspace::TOOLTIP_EXPAND_RIGHT_HUB.get()
                                })
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|app, _, window, cx| {
                                if app.is_right_panel_open() {
                                    app.close_right_panel(window, cx);
                                } else {
                                    app.open_right_panel(RightPanelView::Hub, window, cx);
                                }
                            })),
                    )
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
                // Constrain the resize group to the space below the title bar.
                div().flex_1().min_h_0().overflow_hidden().child({
                    let main_content = if self.show_trash {
                        self.render_trash(cx)
                    } else {
                        editor_with_terminal
                    };
                    div()
                        .id("workspace-body")
                        .relative()
                        .flex()
                        .size_full()
                        .min_w_0()
                        .min_h_0()
                        .on_drag_move(cx.listener(
                            |app, event: &DragMoveEvent<SidebarResizeDrag>, _, cx| {
                                let Some((start_x, start_w)) = app.sidebar_drag else {
                                    return;
                                };
                                let delta = f32::from(event.event.position.x) - start_x;
                                let next_w = (start_w + delta).clamp(170., 520.).round();
                                if (app.settings.sidebar_width - next_w).abs() >= 1.0 {
                                    app.settings.sidebar_width = next_w;
                                    app.schedule_resize_save(cx);
                                    cx.notify();
                                }
                            },
                        ))
                        .when(sidebar_rendered, |row| {
                            row.child(
                                div()
                                    .relative()
                                    .w(px(sidebar_now_w))
                                    .h_full()
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .relative()
                                            .w(px(sidebar_target_w))
                                            .h_full()
                                            .left(px(-sidebar_target_w * (1.0 - sidebar_progress)))
                                            .child(self.render_sidebar(cx)),
                                    )
                                    .when(
                                        sidebar_visible && sidebar_progress >= 0.999,
                                        |panel| {
                                            let drag_app = cx.entity();
                                            panel.child(
                                                resize_handle("sidebar-resize", Axis::Horizontal)
                                                    .inside(HandleEdge::Trailing)
                                                    .on_drag(
                                                        SidebarResizeDrag,
                                                        move |_, _, window, cx| {
                                                            let start_x = f32::from(
                                                                window.mouse_position().x,
                                                            );
                                                            drag_app.update(cx, |app, _| {
                                                                app.sidebar_drag = Some((
                                                                    start_x,
                                                                    app.settings
                                                                        .sidebar_width
                                                                        .clamp(170., 520.),
                                                                ));
                                                            });
                                                            cx.new(|_| SidebarResizeDrag)
                                                        },
                                                    ),
                                            )
                                        },
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .min_h_0()
                                .child(main_content),
                        )
                }),
            )
            .when(self.tags.is_editing(), |app| app.child(self.render_tag_editor(cx)))
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
                NameAction::Create => crate::i18n::workspace::MODAL_CREATE_FILE.get(),
                NameAction::CreateFolder => crate::i18n::workspace::MODAL_CREATE_FOLDER.get(),
                NameAction::RenameFile(_) => crate::i18n::workspace::MODAL_RENAME_FILE.get(),
                NameAction::RenameFolder(_) => crate::i18n::workspace::MODAL_RENAME_FOLDER.get(),
                NameAction::RenameProject(_) => crate::i18n::workspace::MODAL_RENAME_PROJECT.get(),
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
                                        crate::ui::button_secondary("cancel-name", crate::i18n::common::CANCEL.get())
                                            .on_click(cx.listener(|app, _, _, cx| {
                                                app.name_action = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        crate::ui::button_primary("apply-name", crate::i18n::common::CONFIRM.get()).on_click(
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
                deferred(
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
                                        .max_h(px(f32::from(window.viewport_size().height) * 0.92
                                            - 56.))
                                        .overflow_y_scroll()
                                        .child(self.preferences.clone()),
                                ),
                        ),
                )
                .with_priority(10),
            );
        }
        if let Some(prompt) = self.dialog_prompt.clone() {
            // Confirm dialogs are only used for destructive actions.
            let destructive = matches!(prompt.buttons, DialogButtons::Confirm(_));
            let choices = match prompt.buttons {
                DialogButtons::SaveDiscardCancel => vec![
                    (
                        crate::i18n::workspace::BTN_KEEP_EDITING.get().to_string(),
                        rfd::MessageDialogResult::Cancel,
                        false,
                    ),
                    (
                        crate::i18n::workspace::BTN_DISCARD_CHANGES.get().to_string(),
                        rfd::MessageDialogResult::No,
                        false,
                    ),
                    (
                        crate::i18n::common::SAVE.get().to_string(),
                        rfd::MessageDialogResult::Yes,
                        true,
                    ),
                ],
                DialogButtons::Confirm(confirm) => vec![
                    (
                        crate::i18n::common::CANCEL.get().to_string(),
                        rfd::MessageDialogResult::Cancel,
                        false,
                    ),
                    (confirm, rfd::MessageDialogResult::Ok, true),
                ],
                DialogButtons::RestoreConflict => vec![
                    (
                        crate::i18n::common::SKIP.get().to_string(),
                        rfd::MessageDialogResult::Cancel,
                        false,
                    ),
                    (
                        crate::i18n::common::KEEP_BOTH.get().to_string(),
                        rfd::MessageDialogResult::No,
                        false,
                    ),
                    (
                        crate::i18n::common::REPLACE.get().to_string(),
                        rfd::MessageDialogResult::Yes,
                        true,
                    ),
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

fn compact_menu_item(label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        div()
            .h(px(18.))
            .flex()
            .items_center()
            .text_size(px(11.5))
            .child(label)
    })
}

fn compact_menu_icon_item(icon: IconName, label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        div()
            .h(px(18.))
            .flex()
            .items_center()
            .gap_1p5()
            .text_size(px(11.5))
            .child(crate::ui::icon(icon).size(px(12.)))
            .child(label)
    })
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
        return MarkdownLinkTarget::Invalid(crate::i18n::workspace::ERR_EMPTY_LINK.get().to_owned());
    }
    if let Ok(parsed) = url::Url::parse(raw) {
        if parsed.scheme().eq_ignore_ascii_case("file") {
            return match parsed.to_file_path() {
                Ok(path) => MarkdownLinkTarget::Local {
                    path,
                    anchor: parsed.fragment().map(str::to_owned),
                },
                Err(()) => MarkdownLinkTarget::Invalid(
                    crate::i18n::workspace::err_resolve_local_link(raw),
                ),
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
        return MarkdownLinkTarget::Invalid(crate::i18n::workspace::err_invalid_link(raw));
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
    use super::{classify_markdown_link, MarkdownLinkTarget};
    use std::path::PathBuf;

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
