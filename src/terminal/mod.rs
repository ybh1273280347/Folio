//! Persistent terminal sessions hosted by a disposable, hideable workspace panel.
mod session;
pub mod theme;

use crate::{model::TerminalSettings, theme::Palette};
use gpui_kit::{
    assets::IconName,
    component::{
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
    },
    prelude::FluentBuilder,
    *,
};
use gpui_xterm::TerminalView;
use std::{collections::BTreeMap, path::PathBuf, sync::{Arc, Mutex, mpsc::TryRecvError}, time::Duration};

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
pub struct DraggedFolder(pub String, pub String);

pub struct PathsPreview {
    pub paths: Vec<PathBuf>,
    pub offset: Point<Pixels>,
    pub font: SharedString,
    pub font_size: u8,
    pub trash_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    pub is_folder: bool,
    pub can_trash: bool,
    pub folder_bounds: Arc<Mutex<BTreeMap<String, (bool, Bounds<Pixels>)>>>,
}

impl Render for PathsPreview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let count = self.paths.len();
        let over_trash = self.trash_bounds.lock().expect("trash bounds").is_some_and(|bounds| bounds.contains(&window.mouse_position()));
        let folders = self.folder_bounds.lock().expect("tree bounds");
        let over_root = self.can_trash && !self.is_folder && folders.get("").is_some_and(|(_, bounds)| bounds.contains(&window.mouse_position()));
        let over_folder = self.can_trash && !self.is_folder && folders.iter().any(|(path, (is_file, bounds))| !path.is_empty() && !is_file && bounds.contains(&window.mouse_position()));
        div().pl(self.offset.x + px(12.)).pt(self.offset.y + px(12.))
            .child(
                div().flex().flex_col().gap_1().p_2().min_w(px(160.)).max_w(px(300.))
                    .rounded(px(6.)).border_1().border_color(gpui_kit::rgb(0xd8d8d8))
                    .bg(gpui_kit::rgb(0xffffff)).text_color(gpui_kit::rgb(0x333333))
                    .font_family(self.font.clone()).text_size(px(self.font_size as f32)).shadow_sm()
                    .child(div().flex().items_center().gap_1()
                        .child(crate::ui::icon(IconName::Copy).w(px(14.)).h(px(14.)))
                        .child(if over_trash && !self.can_trash { "禁止删除：独立文件不支持删除".into() } else if over_trash { format!("移入废纸篓 {count} 项") } else if over_root { format!("移动到项目根目录 {count} 项") } else if over_folder { format!("移动到文件夹 {count} 项") } else if count == 1 { "复制路径".into() } else { format!("复制 {count} 个路径") }))
                    .children(self.paths.iter().take(4).map(|path| {
                        let name = path.file_name().expect("file has name").to_string_lossy().into_owned();
                        div().flex().items_center().gap_2().min_w_0()
                            .child(if self.is_folder { div().child(crate::ui::icon(IconName::Folder)).into_any_element() } else { crate::ui::file_icon(&name, false, false) })
                            .child(div().min_w_0().text_ellipsis().child(name))
                    }))
                    .when(count > 4, |preview| preview.child(div().text_color(gpui_kit::rgb(0x787774)).child(format!("还有 {} 项", count - 4)))),
            )
    }
}

fn terminal_control(id: impl Into<ElementId>, name: IconName, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(27.))
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
        .child(crate::ui::icon(name))
}

pub enum TerminalPaneEvent {
    Hide,
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
                                        Ok(status) => format!("已退出 · {}", status.exit_code()),
                                        Err(error) => format!("读取退出状态失败：{error}"),
                                    };
                                    // Release the job after shell exit to reap remaining descendants.
                                    tab.process = None;
                                    changed = true;
                                }
                                Some(Err(TryRecvError::Disconnected)) => {
                                    tab.status = "终端进程已结束".into();
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
            None => format!("终端 {id}"),
        };
        self.sessions.push(SessionTab {
            id,
            title: initial_title,
            cwd: directory.clone(),
            status: "正在启动…".into(),
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
                .h(px(32.))
                .rounded(px(4.))
                .cursor_pointer()
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
        let new_menu_btn = Button::new("new-terminal")
            .ghost()
            .compact()
            .w(px(26.))
            .h(px(26.))
            .icon(crate::ui::icon(IconName::Plus))
            .tooltip("新建终端或启动 Agent")
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
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(gpui_kit::rgb(palette.border))
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
                        terminal_control("maximize-terminal", IconName::Maximize2, palette)
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(TerminalPaneEvent::ToggleMaximize)
                            })),
                    )
                    .child(
                        terminal_control("hide-terminal", IconName::PanelRightOpen, palette)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TerminalPaneEvent::Hide))),
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
                            .child("点击“+”新建终端")
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
