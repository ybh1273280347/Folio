//! Right-side panel navigation, keyboard focus and feature hub.

mod hub;
mod kb;

pub(in crate::workspace) use kb::{KbPanelState, PdfOcrPhase};

use super::*;
use crate::ai::AttachedFile;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RightPanelView {
    Hub,
    KnowledgeBase,
    Graph,
    Ai,
    Terminal,
}

pub(super) struct RightPanel {
    pub(super) open: bool,
    pub(super) maximized: bool,
    pub(super) snap_close: bool,
    pub(super) view: RightPanelView,
    search: Entity<InputState>,
    selected: usize,
    pub(in crate::workspace) kb: kb::KbPanelState,
}

impl RightPanel {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let kb = kb::KbPanelState::new(window, cx);
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
                        app.right_panel.selected = 0;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        if app.right_panel.open && app.right_panel.view == RightPanelView::Hub {
                            let props = app.hub_props(cx);
                            let items = hub::filtered_actions(&props);
                            if let Some(item) = items
                                .get(app.right_panel.selected.min(items.len().saturating_sub(1)))
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
        Self { open: false, maximized: false, snap_close: false, view: RightPanelView::Hub, search: right_panel_search, selected: 0, kb }
    }

    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::workspace::SEARCH_HUB_PLACEHOLDER.get(), window, cx);
        });
        self.kb.refresh_locale(window, cx);
    }
}

impl Workspace {
    pub(super) fn open_right_panel(
        &mut self,
        view: RightPanelView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_panel.open = true;
        self.right_panel.maximized = false;
        self.right_panel.snap_close = false;
        self.right_panel.view = view;

        let directory = self.terminal_directory();
        match view {
            RightPanelView::Hub => {
                self.terminal.update(cx, |pane, _| pane.hide());
                self.right_panel.selected = 0;
                self.right_panel.search.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            RightPanelView::KnowledgeBase => {
                self.terminal.update(cx, |pane, _| pane.hide());
                self.right_panel.kb.flush_pending_clear(window, cx);
                self.refresh_kb_snapshot(cx);
                if self.kb_enabled() {
                    self.right_panel.kb.search.update(cx, |input, cx| {
                        input.focus(window, cx);
                    });
                }
            }
            RightPanelView::Graph => {
                self.terminal.update(cx, |pane, _| pane.hide());
                let active = self.documents.active().to_owned();
                self.local_graph.update(cx, |pane, cx| {
                    pane.set_active_document(&active, cx);
                    pane.focus(window, cx);
                });
                if self.local_graph_dirty {
                    self.refresh_graph_data(true, cx);
                }
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

    pub(super) fn close_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_right_panel_with_snap(false, window, cx);
    }

    pub(super) fn close_right_panel_immediate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_right_panel_with_snap(true, window, cx);
    }

    fn close_right_panel_with_snap(
        &mut self,
        snap_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.right_panel.open {
            return;
        }
        self.right_panel.open = false;
        self.right_panel.maximized = false;
        self.right_panel.snap_close = snap_close;
        self.terminal.update(cx, |pane, _| pane.hide());
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn toggle_right_shortcut(
        &mut self,
        view: RightPanelView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.right_panel.open && self.right_panel.view == view {
            self.close_right_panel(window, cx);
        } else {
            self.open_right_panel(view, window, cx);
        }
    }

    fn hub_props(&self, cx: &App) -> hub::RightPanelHubProps {
        hub::RightPanelHubProps {
            query: self.right_panel.search.read(cx).value().to_string(),
            selected_index: self.right_panel.selected,
            panel_width: self.settings.terminal.panel_width.clamp(280., 1600.),
            preview_open: self.build.panels.preview,
            logs_open: self.build.panels.logs,
            zen_mode: self.zen_mode,
            build_running: self.build.is_running(),
            build_failed: self.build.failed(),
            build_target: self.build.target().to_owned(),
            ai_shortcut: AppSettings::format_key_badge(&self.settings.key_for("toggle_ai")),
            terminal_shortcut: AppSettings::format_key_badge(
                &self.settings.key_for("toggle_terminal"),
            ),
            zen_shortcut: AppSettings::format_key_badge(&self.settings.key_for("toggle_zen")),
            terminal_sessions: self.terminal.read(cx).session_count(),
            ai_tasks: self.ai.read(cx).active_tasks(),
            kb_badge: self.kb_badge(),
            kb_syncing: self.kb_syncing(),
        }
    }

    fn execute_right_panel_action(
        &mut self,
        action: hub::RightPanelAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use hub::RightPanelAction;
        match action {
            RightPanelAction::OpenAi => {
                self.open_right_panel(RightPanelView::Ai, window, cx);
            }
            RightPanelAction::OpenKnowledgeBase => {
                self.open_right_panel(RightPanelView::KnowledgeBase, window, cx);
            }
            RightPanelAction::OpenTerminal => {
                self.open_right_panel(RightPanelView::Terminal, window, cx);
            }
            RightPanelAction::OpenGraph => {
                self.close_right_panel_immediate(window, cx);
                self.open_graph(window, cx);
            }
            RightPanelAction::OpenLocalGraph => {
                self.open_right_panel(RightPanelView::Graph, window, cx);
            }
            RightPanelAction::NewSplitPage => {
                let target_width = self.settings.terminal.panel_width.clamp(280., 1600.);
                self.close_right_panel_immediate(window, cx);
                if let Err(error) = self.new_markdown_split(Some(target_width), window, cx) {
                    self.report(error, cx);
                }
            }
            RightPanelAction::TogglePreview => {
                self.build.panels.preview = !self.build.panels.preview;
                cx.notify();
            }
            RightPanelAction::ToggleLogs => {
                self.build.panels.logs = !self.build.panels.logs;
                cx.notify();
            }
            RightPanelAction::ToggleZen => {
                self.toggle_zen_mode(window, cx);
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

    pub(super) fn focused_attached_file(&self) -> Option<AttachedFile> {
        let clean_key = self.documents.focused_reading_document().unwrap_or(self.documents.active());
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

    pub(super) fn open_ai_with_selection(
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

    pub(super) fn handle_right_panel_keys(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_panel.open
            && self.right_panel.view == RightPanelView::Hub
            && !self.show_preferences
        {
            let hub_focused = self
                .right_panel.search
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
            if hub_focused {
                match key {
                    "up" => {
                        let len = hub::filtered_actions(
                            &self.hub_props(cx),
                        )
                        .len();
                        if len > 0 {
                            self.right_panel.selected =
                                if self.right_panel.selected == 0 {
                                    len - 1
                                } else {
                                    self.right_panel.selected - 1
                                };
                            cx.stop_propagation();
                            cx.notify();
                        }
                        return;
                    }
                    "down" => {
                        let len = hub::filtered_actions(
                            &self.hub_props(cx),
                        )
                        .len();
                        if len > 0 {
                            self.right_panel.selected =
                                (self.right_panel.selected + 1) % len;
                            cx.stop_propagation();
                            cx.notify();
                        }
                        return;
                    }
                    "escape" => {
                        if !self.right_panel.search.read(cx).value().is_empty() {
                            self.right_panel.search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                            });
                            self.right_panel.selected = 0;
                        } else {
                            self.close_right_panel(window, cx);
                        }
                        cx.stop_propagation();
                        return;
                    }
                    _ => {}
                }
            }
        }
        if key == "escape"
            && !self.show_preferences
            && self.right_panel.open
            && self.right_panel.view == RightPanelView::Ai
            && self.ai.update(cx, |pane, cx| pane.dismiss_history(window, cx))
        {
            cx.stop_propagation();
        }
    }

    pub(super) fn render_right_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.right_panel.view {
            RightPanelView::Hub => {
                let props = self.hub_props(cx);
                let action_app = cx.entity();
                let clear_app = cx.entity();
                hub::render_hub(
                    &props,
                    &self.right_panel.search,
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
                            app.right_panel.search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                            app.right_panel.selected = 0;
                            cx.notify();
                        });
                    },
                )
            }
            RightPanelView::KnowledgeBase => self.render_knowledge_base_panel(window, cx),
            RightPanelView::Graph => self.render_local_graph_panel(window, cx),
            RightPanelView::Ai => self.ai.clone().into_any_element(),
            RightPanelView::Terminal => self.terminal.clone().into_any_element(),
        }
    }
}
