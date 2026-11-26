//! Persistent terminal sessions hosted by a disposable, hideable workspace panel.
mod session;
pub mod theme;

use crate::{model::TerminalSettings, theme::Palette};
use gpui_kit::{
    assets::IconName,
    component::{
        menu::{DropdownMenu, PopupMenuItem},
    },
    prelude::FluentBuilder,
    *,
};
use gpui_xterm::TerminalView;
use std::{collections::BTreeMap, path::{Path, PathBuf}, sync::{Arc, Mutex, mpsc::TryRecvError}, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiHarness {
    ClaudeCode,
    Codex,
    OhMyPi,
    Antigravity,
}

impl AiHarness {
    pub const ALL: [Self; 4] = [Self::ClaudeCode, Self::Codex, Self::OhMyPi, Self::Antigravity];

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OhMyPi => "oh-my-pi",
            Self::Antigravity => "Antigravity",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::OhMyPi => "omp",
            Self::Antigravity => "agy",
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            Self::ClaudeCode => "brand/claude.svg",
            Self::Codex => "brand/codex.svg",
            Self::OhMyPi => "brand/pi.svg",
            Self::Antigravity => "brand/antigravity.svg",
        }
    }
}

#[derive(Clone)]
// None identifies standalone files, which can supply paths but cannot be moved or trashed.
pub struct DraggedPaths(pub Vec<PathBuf>, pub Option<String>);

#[derive(Clone)]
// Tree moves use the relative path and project ID. Terminal drops use the source's full path.
pub struct DraggedFolder(pub String, pub String, pub PathBuf);

#[derive(Clone, Default)]
pub struct WorkAreaDropBounds {
    pub main: Option<Bounds<Pixels>>,
    pub split: Option<Bounds<Pixels>>,
}

pub struct PathsPreview {
    pub paths: Vec<PathBuf>,
    pub offset: Point<Pixels>,
    pub font: SharedString,
    pub font_size: u8,
    pub palette: Palette,
    pub trash_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    pub source_folder: Option<String>,
    pub can_trash: bool,
    pub row_bounds: Arc<Mutex<BTreeMap<String, (bool, Bounds<Pixels>)>>>,
    pub work_area_bounds: Arc<Mutex<WorkAreaDropBounds>>,
}

impl Render for PathsPreview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use crate::i18n::media;
        use gpui_kit::rgb;

        let count = self.paths.len();
        let mouse = window.mouse_position();
        let over_trash = self.trash_bounds.lock().expect("trash bounds")
            .is_some_and(|bounds| bounds.contains(&mouse));
        let work_area = self.work_area_bounds.lock().expect("work area drop bounds").clone();
        let over_split = self.source_folder.is_none()
            && work_area.split.is_some_and(|bounds| bounds.contains(&mouse));
        let over_main = self.source_folder.is_none()
            && !over_split
            && work_area.main.is_some_and(|bounds| bounds.contains(&mouse));
        let rows = self.row_bounds.lock().expect("tree bounds");
        let target = rows.iter()
            .find(|(_, (_, bounds))| bounds.contains(&mouse))
            .map(|(path, (is_file, _))| {
                // File rows receive drops on behalf of their parent directory.
                if *is_file { path.rsplit_once('/').map_or("", |(parent, _)| parent) } else { path.as_str() }
            });
        let invalid_folder = self.source_folder.as_ref().is_some_and(|source| {
            target.is_some_and(|target| Path::new(target).starts_with(source))
        });
        // The pointer hint describes the current destination. Away from a drop
        // target the chip only identifies what is being carried.
        let hint = if over_trash {
            Some(if self.can_trash { media::DRAG_TRASH } else { media::DRAG_LOOSE_FILE }.get().into())
        } else if over_split {
            Some(format!("→ {}", media::DRAG_OPEN_SPLIT.get()))
        } else if over_main {
            Some(format!("→ {}", media::DRAG_OPEN_MAIN.get()))
        } else if invalid_folder {
            Some(media::DRAG_INVALID_FOLDER.get().into())
        } else if self.can_trash {
            target.map(|path| {
                let name = Path::new(path).file_name().map_or_else(
                    || media::DRAG_PROJECT_ROOT.get().into(),
                    |name| name.to_string_lossy(),
                );
                format!("→ {name}")
            })
        } else {
            None
        };
        let invalid = (over_trash && !self.can_trash) || invalid_folder;
        let palette = self.palette;
        let shadow_strength = if palette.is_dark() { 2.0 } else { 1.0 };
        let name = self.paths[0].file_name().expect("dragged item has a name")
            .to_string_lossy().into_owned();
        let icon = if self.source_folder.is_some() {
            crate::ui::icon(IconName::Folder).size(px(15.)).into_any_element()
        } else {
            crate::ui::file_icon(&name, false, false)
        };
        // GPUI subtracts the original grab offset. A relative translation cancels
        // it without adding layout padding, even when the first move overshoots the row.
        div().child(div().relative().left(self.offset.x + px(7.)).top(self.offset.y + px(6.))
            .flex().flex_col().items_start().gap(px(3.))
            .font_family(self.font.clone()).text_size(px(f32::from(self.font_size).clamp(11., 13.)))
            .child(div().flex().items_center().gap(px(5.)).h(px(24.))
                .px_2().max_w(px(240.)).rounded(px(7.))
                .border_1().border_color(rgb(palette.border))
                .bg(rgb(palette.surface)).text_color(rgb(palette.text))
                .shadow(vec![
                    BoxShadow::new(px(0.), px(3.), hsla(0., 0., 0., 0.10 * shadow_strength))
                        .blur_radius(px(12.)),
                    BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., 0.08 * shadow_strength))
                        .blur_radius(px(3.)),
                ])
                .child(div().flex_shrink_0().child(icon))
                .child(div().min_w_0().text_ellipsis().child(name))
                .when(count > 1, |chip| chip.child(div()
                    .flex_shrink_0().rounded(px(8.)).px_1()
                    .bg(rgb(palette.text).opacity(0.07)).text_color(rgb(palette.muted))
                    .child(count.to_string()))))
            // Keep the filename chip the same size as the destination changes.
            .when_some(hint, |preview, hint| preview.child(div()
                    .max_w(px(160.)).text_ellipsis().px_2().rounded(px(4.))
                    .bg(rgb(palette.surface))
                    .text_color(rgb(if invalid { palette.danger } else { palette.muted }))
                    .child(hint))))
    }
}

fn terminal_control(id: impl Into<ElementId>, name: IconName, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(18.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(gpui_kit::rgb(palette.muted))
        .hover(move |s| {
            s.bg(gpui_kit::rgb(palette.hover))
                .text_color(gpui_kit::rgb(palette.text))
        })
        .active(move |s| {
            s.bg(gpui_kit::rgb(palette.pressed))
                .text_color(gpui_kit::rgb(palette.text))
        })
        .child(crate::ui::icon(name).size(px(12.)))
}

pub enum TerminalPaneEvent {
    BackToHub,
    ToggleMaximize,
}
impl EventEmitter<TerminalPaneEvent> for TerminalPane {}

struct SessionTab {
    id: u64,
    title: String,
    cwd: PathBuf,
    status: String,
    view: Option<Entity<TerminalView>>,
    process: Option<session::TerminalProcess>,
    _starting: Task<()>,
}

pub struct TerminalPane {
    sessions: Vec<SessionTab>,
    active: Option<u64>,
    visible: bool,
    next_id: u64,
    directory: PathBuf,
    settings: TerminalSettings,
    palette: Palette,
    _poll: Task<()>,
}

impl TerminalPane {
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    fn drop_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.sessions.iter().find(|tab| Some(tab.id) == self.active).and_then(|tab| tab.view.as_ref()) {
            // Use the terminal's paste path so bracketed-paste applications receive the right protocol.
            let text = paths.iter().map(|path| format!("\"{}\"", path.to_string_lossy())).collect::<Vec<_>>().join(" ");
            view.read(cx).paste_text(&text);
            self.focus(window, cx);
        }
    }
    pub fn new(settings: TerminalSettings, palette: Palette, cx: &mut Context<Self>) -> Self {
        cx.on_app_quit(|pane, _| {
            pane.sessions.clear();
            async {}
        })
        .detach();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this
                    .update(cx, |pane: &mut Self, cx| {
                        let mut changed = false;
                        for tab in &mut pane.sessions {
                            let result =
                                tab.process.as_ref().map(|process| process.exit.try_recv());
                            match result {
                                Some(Ok(result)) => {
                                    tab.status = match result {
                                        Ok(status) => crate::i18n::media::term_exited_code(status.exit_code()),
                                        Err(error) => crate::i18n::media::term_exit_status_err(&error),
                                    };
                                    // Release the job after shell exit to reap remaining descendants.
                                    tab.process = None;
                                    changed = true;
                                }
                                Some(Err(TryRecvError::Disconnected)) => {
                                    tab.status = crate::i18n::media::TERM_PROCESS_ENDED.get().into();
                                    tab.process = None;
                                    changed = true;
                                }
                                _ => {}
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            sessions: Vec::new(),
            active: None,
            visible: false,
            next_id: 1,
            directory: dirs::home_dir().expect("user home"),
            settings,
            palette: theme::terminal_palette(palette),
            _poll: poll,
        }
    }

    pub fn open(&mut self, directory: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        self.directory = directory;
        if self.sessions.is_empty() {
            self.new_session(window, cx);
        } else {
            self.focus(window, cx);
        }
    }

    pub fn set_directory(&mut self, directory: PathBuf) {
        self.directory = directory;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn sync(&mut self, settings: &TerminalSettings, palette: Palette, cx: &mut Context<Self>) {
        let palette = theme::terminal_palette(palette);
        if self.settings == *settings && self.palette == palette {
            return;
        }
        self.settings = settings.clone();
        self.palette = palette;
        for tab in &self.sessions {
            if let Some(view) = &tab.view {
                view.update(cx, |view, cx| {
                    let mut config = theme::config(settings, palette);
                    (config.cols, config.rows) = view.dimensions();
                    view.update_config(config, cx);
                });
            }
        }
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(view) = self
            .sessions
            .iter()
            .find(|tab| Some(tab.id) == self.active)
            .and_then(|tab| tab.view.as_ref())
        {
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        }
    }

    fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_session(None, window, cx);
    }

    pub fn new_harness_session(&mut self, harness: AiHarness, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_session(Some(harness), window, cx);
    }

    fn spawn_session(&mut self, harness: Option<AiHarness>, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        let directory = self.directory.clone();
        let settings = self.settings.clone();
        let initial_title = match harness {
            Some(h) => format!("{} {id}", h.label()),
            None => crate::i18n::media::term_default_title(id),
        };
        self.sessions.push(SessionTab {
            id,
            title: initial_title,
            cwd: directory.clone(),
            status: crate::i18n::media::TERM_STARTING.get().into(),
            view: None,
            process: None,
            _starting: Task::ready(()),
        });
        self.active = Some(id);
        let task = cx.spawn_in(window, async move |this, cx| {
            let cmd = harness.map(|h| h.command());
            let result = cx
                .background_executor()
                .spawn(async move { session::start_command(&settings, &directory, cmd) })
                .await;
            let _ = this.update_in(cx, |pane, window, cx| {
                let Some(index) = pane.sessions.iter().position(|tab| tab.id == id) else {
                    return;
                };
                match result {
                    Ok(started) => {
                        let master = started.process.master.clone();
                        let config = theme::config(&pane.settings, pane.palette);
                        let weak_pane = cx.weak_entity();
                        let view = cx.new(|cx| {
                            TerminalView::new(started.writer, started.reader, config, cx)
                                .with_resize_callback(move |cols, rows| {
                                    let _ = master.lock().expect("PTY master").resize(
                                        portable_pty::PtySize {
                                            rows: rows as u16,
                                            cols: cols as u16,
                                            pixel_width: 0,
                                            pixel_height: 0,
                                        },
                                    );
                                })
                                .with_title_callback(move |_, cx, title| {
                                    let weak_pane = weak_pane.clone();
                                    let title: String = title.chars().take(80).collect();
                                    // The terminal is updating here; defer parent updates to avoid
                                    // borrowing its host again during the same render traversal.
                                    cx.defer(move |cx| {
                                        let _ = weak_pane.update(cx, |pane, cx| {
                                            if !title.is_empty()
                                                && let Some(tab) = pane
                                                    .sessions
                                                    .iter_mut()
                                                    .find(|tab| tab.id == id)
                                            {
                                                tab.title = title;
                                                cx.notify();
                                            }
                                        });
                                    });
                                })
                        });
                        let tab = &mut pane.sessions[index];
                        tab.title = match harness {
                            Some(h) => format!("{} {id}", h.label()),
                            None => format!("{} {id}", started.shell),
                        };
                        tab.status.clear();
                        tab.view = Some(view);
                        tab.process = Some(started.process);
                        if pane.visible && pane.active == Some(id) {
                            pane.focus(window, cx);
                        }
                    }
                    Err(error) => pane.sessions[index].status = error,
                }
                cx.notify();
            });
        });
        self.sessions.last_mut().expect("new tab")._starting = task;
        cx.notify();
    }

    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.sessions.iter().position(|tab| tab.id == id) {
            self.sessions.remove(index);
            if self.active == Some(id) {
                self.active = self
                    .sessions
                    .get(index.min(self.sessions.len().saturating_sub(1)))
                    .map(|tab| tab.id);
            }
        }
        self.focus(window, cx);
        cx.notify();
    }
}

impl Render for TerminalPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let tabs = self.sessions.iter().map(|tab| {
            let id = tab.id;
            let active = self.active == Some(id);
            div()
                .id(("terminal-tab", id as usize))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(crate::ui::TAB_HEIGHT))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(12.5))
                .text_color(gpui_kit::rgb(if active {
                    palette.text
                } else {
                    palette.muted
                }))
                .when(active, |row| row.bg(gpui_kit::rgb(palette.hover)))
                .when(!active, |row| {
                    row.hover(move |s| {
                        s.bg(gpui_kit::rgb(palette.hover))
                            .text_color(gpui_kit::rgb(palette.text))
                    })
                })
                .child(
                    div()
                        .max_w(px(140.))
                        .text_ellipsis()
                        .child(tab.title.clone()),
                )
                .child(
                    terminal_control(("close-terminal", id as usize), IconName::Close, palette)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |pane, _, window, cx| {
                            pane.close(id, window, cx);
                            cx.stop_propagation();
                        })),
                )
                .on_click(cx.listener(move |pane, _, window, cx| {
                    pane.active = Some(id);
                    pane.focus(window, cx);
                    cx.notify();
                }))
        });
        let current = self.sessions.iter().find(|tab| Some(tab.id) == self.active);
        let pane_entity = cx.entity();
        let new_menu_btn = crate::ui::panel_toolbar_button("new-terminal", palette, cx)
            .icon(crate::ui::icon(IconName::Plus).size(px(15.)))
            .tooltip(crate::i18n::media::TERM_NEW_OR_AGENT.get())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let pane_term = pane_entity.clone();
                let mut menu = menu
                    .min_w(px(188.))
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .w_full()
                                .py(px(1.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .text_xs()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(crate::ui::icon(IconName::Terminal).w(px(14.)).h(px(14.)))
                                        .child("Terminal"),
                                )
                                .child(div().text_size(px(11.)).opacity(0.55).child("Shell"))
                        })
                        .on_click(move |_, window, cx| {
                            pane_term.update(cx, |pane, cx| pane.new_session(window, cx));
                        }),
                    )
                    .separator()
                    .item(PopupMenuItem::label("Start an agent"));
                for (idx, harness) in AiHarness::ALL.into_iter().enumerate() {
                    let pane_h = pane_entity.clone();
                    let index_label = (idx + 1).to_string();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .w_full()
                                .py(px(1.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .text_xs()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(img(harness.icon_path()).w(px(14.)).h(px(14.)))
                                        .child(harness.label()),
                                )
                                .child(div().text_size(px(11.)).opacity(0.5).child(index_label.clone()))
                        })
                        .on_click(move |_, window, cx| {
                            pane_h.update(cx, |pane, cx| pane.new_harness_session(harness, window, cx));
                        }),
                    );
                }
                menu
            });
        div()
            .id("folio-terminal")
            .on_drop(cx.listener(|pane, paths: &DraggedPaths, window, cx| pane.drop_paths(&paths.0, window, cx)))
            .on_drop(cx.listener(|pane, folder: &DraggedFolder, window, cx| {
                pane.drop_paths(std::slice::from_ref(&folder.2), window, cx);
            }))
            .on_drop(cx.listener(|pane, paths: &ExternalPaths, window, cx| pane.drop_paths(paths.paths(), window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(gpui_kit::rgb(palette.surface))
            .text_color(gpui_kit::rgb(palette.text))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(crate::ui::PANE_HEADER_HEIGHT))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .child(
                        crate::ui::panel_toolbar_button("terminal-back-to-hub", palette, cx)
                            .icon(crate::ui::icon(IconName::ChevronLeft).size(px(15.)))
                            .tooltip(crate::i18n::common::BACK_TO_HUB.get())
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(TerminalPaneEvent::BackToHub)
                            })),
                    )
                    .child(
                        div()
                            .id("terminal-tabs")
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .children(tabs),
                    )
                    .child(new_menu_btn)
                    .child(
                        crate::ui::panel_toolbar_button("maximize-terminal", palette, cx)
                            .icon(crate::ui::icon(IconName::Maximize2).size(px(15.)))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(TerminalPaneEvent::ToggleMaximize)
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when_some(current.and_then(|tab| tab.view.clone()), |area, view| {
                        area.child(view)
                    })
                    .when(current.is_none(), |area| {
                        area.p_4()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child(crate::i18n::media::TERM_CLICK_PLUS_HINT.get())
                    }),
            )
            .when_some(current, |area, tab| {
                area.child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .border_t_1()
                        .border_color(gpui_kit::rgb(palette.border))
                        .child(div().text_ellipsis().child(tab.cwd.display().to_string()))
                        .when(!tab.status.is_empty(), |row| row.child(tab.status.clone())),
                )
            })
    }
}
