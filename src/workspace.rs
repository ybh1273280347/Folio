//! Window composition, actions and orchestration across Workspace submodules.
mod appearance;
mod build;
mod dialogs;
mod documents;
mod file_tree;
mod panels;
mod projects;
mod tags;
mod tools;
use build::{Build, BuildPanelVisibility};
use dialogs::{Dialog, DialogButtons, Dialogs};
use documents::Documents;
use file_tree::FileTree;
use panels::{RightPanel, RightPanelView};
use projects::Projects;
use crate::ai::{AiPane, AiPaneEvent};
use gpui_kit::component::Sizable;
use crate::theme::color as rgb;
use crate::math::MathRenderer;
use crate::preferences::{Preferences, SettingsChanged};
use crate::terminal::{TerminalPane, TerminalPaneEvent};
use crate::{
    files,
    model::{
        AppSettings, Engine, FileKind,
        Project, ProjectFile, WorkspaceLocation,
    },
    storage,
};
use gpui_kit::{
    assets::IconName,
    base::{Easing, Scrollbar, ScrollbarMode, StyledExt, Transition, transition_with_status},
    component::{
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        progress::Progress,
        resizable::{HandleEdge, h_resizable, resizable_panel, resize_handle, v_resizable},
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
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
        ToggleZen,
        FindFile,
        CloseTab,
        CopyFilePath,
        PasteToFileTree,
        EditTags,
        DeleteFileTreeItem,
        UndoFileDelete,
        Dismiss
    ]
);
const BG: u32 = 0xf7f6f5;
const BORDER: u32 = 0xe8e7e3;
const MUTED: u32 = 0x787774;

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

pub fn bind_shortcuts(settings: &AppSettings, cx: &mut App) {
    let mut bindings = vec![
        KeyBinding::new("ctrl-c", gpui_kit::component::input::Copy, Some("FolioPdf")),
        KeyBinding::new("ctrl-c", CopyFilePath, Some("FolioFileTree")),
        KeyBinding::new("ctrl-v", PasteToFileTree, Some("FolioFileTree")),
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
    push_key(&mut bindings, settings.key_for("toggle_zen"), ToggleZen, Some("FolioWorkspace"));
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
#[derive(Clone, Copy)]
struct ZenSnapshot {
    sidebar_open: bool,
    right_panel_open: bool,
    right_panel_maximized: bool,
    build_panels: BuildPanelVisibility,
    was_fullscreen: bool,
}

pub struct Workspace {
    tool_requests: tools::ToolRequests,
    rag_sync: tools::RagSyncState,
    projects: Projects,
    settings: AppSettings,
    backdrop: appearance::Backdrop,
    preferences: Entity<Preferences>,
    math: Entity<MathRenderer>,
    show_preferences: bool,
    tags: tags::TagPanel,
    tree: FileTree,
    documents: Documents,
    build: Build,
    zen_mode: bool,
    zen_snapshot: Option<ZenSnapshot>,
    terminal: Entity<TerminalPane>,
    ai: Entity<AiPane>,
    right_panel: RightPanel,
    sidebar_drag: Option<(f32, f32)>,
    right_panel_drag: Option<(f32, f32)>,
    resize_save_task: Task<()>,
    notice: Option<String>,
    notice_until: Option<Instant>,
    focus: FocusHandle,
    close_requested: bool,
    dialogs: Dialogs,
    editor_zoom_hud: crate::ui::ZoomHud,
    _tick: Task<()>,
    _history_escape: Subscription,
}

impl Workspace {
    pub fn new(settings: AppSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (projects, initial_location) = Projects::load();
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
                    app.right_panel.maximized = !app.right_panel.maximized;
                }
            }
            cx.notify();
        })
        .detach();
        let initial_dir = dirs::home_dir().expect("user home");
        let ai = cx.new(|cx| AiPane::new(initial_dir, settings.clone(), math.clone(), window, cx));
        cx.subscribe_in(&ai, window, |app, _, event: &AiPaneEvent, window, cx| match event {
            AiPaneEvent::WorkspaceTool(request) => app.dispatch_workspace_tool(request, window, cx),
            AiPaneEvent::UndoTags(items) => app.undo_tag_confirmations(items, cx),
            AiPaneEvent::OpenFile(path) => {
                app.open_path_from_ai(path, window, cx);
            }
            AiPaneEvent::BackToHub => {
                app.open_right_panel(RightPanelView::Hub, window, cx);
            }
            AiPaneEvent::ToggleMaximize => {
                app.right_panel.maximized = !app.right_panel.maximized;
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&preferences, window, |app, _, event: &SettingsChanged, window, cx| {
            app.apply_settings(event.apply_to(&app.settings), window, cx);
        })
        .detach();
        let tree = FileTree::new(window, cx);
        let right_panel = RightPanel::new(window, cx);
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
                        app.handle_right_panel_keys(key, window, cx);
                    });
                }
            }
        });
        cx.on_app_quit(|_, _| {
            crate::rag::client::shutdown_managed_servers();
            async {}
        })
        .detach();
        let mut app = Self {
            tool_requests: tools::ToolRequests::default(),
            rag_sync: tools::RagSyncState::default(),
            projects,
            settings,
            backdrop: appearance::Backdrop::default(),
            preferences,
            math,
            show_preferences: false,
            tags,
            tree,
            documents: Documents::default(),
            build: Build::default(),
            zen_mode: false,
            zen_snapshot: None,
            terminal,
            ai,
            right_panel,
            sidebar_drag: None,
            right_panel_drag: None,
            resize_save_task: Task::ready(()),
            notice: None,
            notice_until: None,
            focus: cx.focus_handle(),
            close_requested: false,
            dialogs: Dialogs::default(),
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

    fn focus_active_or_workspace(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_document_tab(self.documents.active(), window, cx) {
            self.focus.focus(window, cx);
        }
    }

    fn toggle_zen_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.zen_mode {
            let snapshot = ZenSnapshot {
                sidebar_open: self.settings.sidebar_open,
                right_panel_open: self.right_panel.open,
                right_panel_maximized: self.right_panel.maximized,
                build_panels: self.build.panels,
                was_fullscreen: window.is_fullscreen(),
            };
            self.zen_snapshot = Some(snapshot);
            self.zen_mode = true;
            self.tree.dismiss_trash();
            self.settings.sidebar_open = false;
            if self.right_panel.open {
                self.right_panel.open = false;
                self.right_panel.maximized = false;
                self.terminal.update(cx, |pane, _| pane.hide());
            }
            self.build.panels = BuildPanelVisibility::default();
            self.notice = None;
            self.notice_until = None;
            if !window.is_fullscreen() {
                window.toggle_fullscreen();
            }
            self.documents.set_zen_mode(self.zen_mode, cx);
            if snapshot.right_panel_open || !self.focus.contains_focused(window, cx) {
                self.focus_active_or_workspace(window, cx);
            }
            cx.notify();
            return;
        }

        self.zen_mode = false;
        if let Some(snapshot) = self.zen_snapshot.take() {
            self.settings.sidebar_open = snapshot.sidebar_open;
            self.build.panels = snapshot.build_panels;
            if snapshot.right_panel_open {
                self.open_right_panel(self.right_panel.view, window, cx);
                self.right_panel.maximized = snapshot.right_panel_maximized;
            } else if self.right_panel.open {
                self.close_right_panel(window, cx);
            }
            if window.is_fullscreen() != snapshot.was_fullscreen {
                window.toggle_fullscreen();
            }
        }
        self.documents.set_zen_mode(self.zen_mode, cx);
        if !self.focus.contains_focused(window, cx) {
            self.focus_active_or_workspace(window, cx);
        }
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
        if let Some(project) = self.projects.directory()
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
        self.dialogs.set_native_open(true);
        let app_entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let result = if let Some(destination) = dialog.save_file().await {
                    std::fs::copy(&source, destination.path()).map(|_| ())
                } else {
                    Ok(())
                };
                let _ = app_entity.update(cx, |app, cx| {
                    app.dialogs.set_native_open(false);
                    if let Err(error) = result {
                        app.report(error, cx);
                    }
                });
            })
            .detach();
    }
    fn confirm_drafts(
        &mut self,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        if !self.documents.open_buffers().any(|(_, buffer)| buffer.is_dirty())
            && !self.dialogs.has_response(Dialog::Drafts)
        {
            return true;
        }
        match self.ask(
            Dialog::Drafts,
            crate::i18n::workspace::DIALOG_UNSAVED_ALL_TITLE.get().into(),
            crate::i18n::workspace::DIALOG_UNSAVED_ALL_BODY.get().into(),
            DialogButtons::SaveDiscardCancel,
            cx,
            retry,
        ) {
            Some(rfd::MessageDialogResult::Yes) => self.save_all(cx),
            Some(rfd::MessageDialogResult::No) => true,
            _ => false,
        }
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.confirm_drafts(cx, |app, window, cx| {
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
        self.cancel_rag_sync();
        crate::rag::client::shutdown_managed_servers();
        self.release_all_html_panes(window, cx);
        self.release_all_drawio_panes(window, cx);
        true
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_trash(cx);
        self.poll_rag(cx);
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
        self.documents.poll_web_panes(window, cx);
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
        let trash_open = self.tree.is_trash_open();
        let modal_open = trash_open || self.show_preferences
            || self.tags.is_editing()
            || self.tree.name_dialog_open()
            || self.dialogs.blocks_workspace();
        self.documents.sync_pane_visibility(self.zen_mode, trash_open, modal_open, window, cx);
        self.build.sync_preview_visibility(!self.zen_mode && !trash_open, window, cx);
        let engine = self.projects.directory().map(|project| project.engine);
        let engine_app = cx.entity();
        let main_app = cx.entity();
        let main_label = self.projects.directory()
            .map(|project| project.main_file.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| crate::i18n::workspace::SELECT_MAIN_FILE.get().into());
        let main_files: Vec<_> = self
            .tree.files()
            .iter()
            .filter(|file| file.path.to_lowercase().ends_with(".tex"))
            .map(|file| file.path.clone())
            .collect();
        let resize_app = cx.entity();
        let right_panel_visible = self.right_panel.open;
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
            if right_panel_visible && !self.right_panel.maximized {
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
        let right_now_w = if right_panel_visible && self.right_panel.maximized {
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
        if self.build.panels.preview {
            work_area = work_area.child(
                resizable_panel()
                    .size(px(area_width * (1. - self.settings.editor_ratio)))
                    .child(div().relative().size_full().child(self.render_preview(cx))),
            );
        }
        if !self.zen_mode {
            for path in self.documents.reading_panes() {
                work_area =
                    work_area.child(resizable_panel().child(self.render_reading_pane(path, cx)));
            }
        }
        let label = if self.build.is_busy() {
            crate::i18n::workspace::BTN_STOP_BUILD.get()
        } else {
            crate::i18n::workspace::BTN_COMPILE_BUILD.get()
        };
        let status = self.build.status_label(self.documents.source_version());
        let mut vertical_work_area = v_resizable(if self.build.panels.logs {
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
                    if app.build.panels.logs {
                        app.settings.log_height = log_height;
                        app.persist_settings(cx);
                    }
                });
            }
        })
        .child(resizable_panel().child(work_area));
        if self.build.panels.logs {
            vertical_work_area = vertical_work_area.child(
                resizable_panel()
                    .size(px(self.settings.log_height))
                    .size_range(px(100.)..px(700.))
                    .fixed()
                    .child(self.render_build_log(cx)),
            );
        }
        let right_panel_content = right_rendered.then(|| self.render_right_panel(window, cx));

        let maximized = right_panel_visible && self.right_panel.maximized;
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
                app.when_some(self.backdrop.image().cloned(), |app, image| {
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
            .on_action(cx.listener(|app, _: &ToggleZen, window, cx| {
                if app.show_preferences
                    || app.dialogs.blocks_workspace()
                    || app.tree.name_dialog_open()
                {
                    cx.propagate();
                    return;
                }
                app.toggle_zen_mode(window, cx);
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
                app.documents.clear_pending_canvas_insert();
                app.tree.dismiss_overlays();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &SaveAll, _, cx| {
                app.save_all(cx);
            }))
            .on_action(cx.listener(|app, _: &CloseTab, window, cx| {
                if app.tree.is_trash_open() {
                    app.close_trash(window, cx);
                    return;
                }
                let path = app.documents.active().to_owned();
                app.close_tab(&path, window, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleSidebar, _, cx| {
                app.toggle_sidebar(cx);
            }))
            .when(!self.zen_mode, |app| {
                app.child(
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
                    .when(self.projects.active_id().is_some(), |header| {
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
                                                            app.set_main_file(&file, cx);
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
                                if app.right_panel.open {
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
            })
            .child(
                // Constrain the resize group to the space below the title bar.
                div().flex_1().min_h_0().overflow_hidden().child({
                    let main_content = if trash_open {
                        self.render_editor(cx)
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
                                            .child(self.render_sidebar(window, cx)),
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
            })
            .when(
                self.zen_mode
                    && !right_panel_visible
                    && !self.documents.active_markdown_search_open(cx),
                |app| {
                    let palette = crate::theme::palette();
                    let shortcut =
                        AppSettings::format_key_badge(&self.settings.key_for("toggle_zen"));
                    app.child(
                        div()
                            .id("zen-exit-pill")
                            .absolute()
                            .top_2p5()
                            .right_3()
                            .h(px(26.))
                            .px_2p5()
                            .rounded_full()
                            .bg(gpui_kit::rgb(palette.surface))
                            .border_1()
                            .border_color(gpui_kit::rgb(palette.border))
                            .shadow_sm()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .opacity(0.28)
                            .hover(move |s| {
                                s.opacity(1.0)
                                    .bg(gpui_kit::rgb(palette.hover))
                                    .text_color(gpui_kit::rgb(palette.text))
                            })
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.toggle_zen_mode(window, cx);
                            }))
                            .child(crate::ui::icon(IconName::Close).size(px(12.)))
                            .child(crate::i18n::workspace::exit_zen_label(&shortcut)),
                    )
                },
            );
        app = app.children(self.render_name_dialog(cx));
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
        app.children(self.render_dialog(cx))
            .children(self.render_canvas_target_picker(cx))
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

pub(crate) fn line_byte_range(text: &str, line0: usize) -> Option<std::ops::Range<usize>> {
    let mut current = 0usize;
    let mut start = 0usize;
    for chunk in text.split_inclusive('\n') {
        if current == line0 {
            let trimmed = chunk.trim_end_matches(['\r', '\n']);
            return Some(start..start + trimmed.len());
        }
        start += chunk.len();
        current += 1;
    }
    if current == line0 {
        return Some(start..text.len());
    }
    None
}

pub(in crate::workspace) fn compact_menu_icon_item(icon: IconName, label: &'static str) -> PopupMenuItem {
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
