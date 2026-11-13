//! Persistent terminal sessions hosted by a disposable, hideable workspace panel.
mod session;
mod theme;

use crate::{
    model::TerminalSettings,
    theme::{Palette, color as rgb},
};
use gpui_kit::{
    assets::IconName,
    component::button::{Button, ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use gpui_xterm::TerminalView;
use std::{path::PathBuf, sync::mpsc::TryRecvError, time::Duration};

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
            palette,
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
        let id = self.next_id;
        self.next_id += 1;
        let directory = self.directory.clone();
        let settings = self.settings.clone();
        self.sessions.push(SessionTab {
            id,
            title: format!("终端 {id}"),
            cwd: directory.clone(),
            status: "正在启动…".into(),
            view: None,
            process: None,
            _starting: Task::ready(()),
        });
        self.active = Some(id);
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { session::start(&settings, &directory) })
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
                        tab.title = format!("{} {id}", started.shell);
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
        let tabs = self.sessions.iter().map(|tab| {
            let id = tab.id;
            div()
                .id(("terminal-tab", id as usize))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(32.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(self.active == Some(id), |row| row.bg(rgb(0xefefed)))
                .child(
                    div()
                        .max_w(px(140.))
                        .text_ellipsis()
                        .child(tab.title.clone()),
                )
                .child(
                    crate::ui::control(("close-terminal", id as usize), IconName::Close)
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
        div()
            .id("folio-terminal")
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(gpui_kit::rgb(self.palette.surface))
            .text_color(gpui_kit::rgb(self.palette.text))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(38.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(gpui_kit::rgb(self.palette.border))
                    .child(div().flex_1().min_w_0().child("终端"))
                    .child(
                        Button::new("new-terminal")
                            .ghost()
                            .compact()
                            .label("新建")
                            .on_click(
                                cx.listener(|pane, _, window, cx| pane.new_session(window, cx)),
                            ),
                    )
                    .child(
                        crate::ui::control("maximize-terminal", IconName::Maximize2).on_click(
                            cx.listener(|_, _, _, cx| cx.emit(TerminalPaneEvent::ToggleMaximize)),
                        ),
                    )
                    .child(
                        crate::ui::control("hide-terminal", IconName::PanelRightOpen)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TerminalPaneEvent::Hide))),
                    ),
            )
            .child(
                div()
                    .id("terminal-tabs")
                    .flex()
                    .flex_shrink_0()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(tabs),
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
                        area.p_4().child("点击“新建”打开终端")
                    }),
            )
            .when_some(current, |area, tab| {
                area.child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .border_t_1()
                        .border_color(gpui_kit::rgb(self.palette.border))
                        .child(div().text_ellipsis().child(tab.cwd.display().to_string()))
                        .when(!tab.status.is_empty(), |row| row.child(tab.status.clone())),
                )
            })
    }
}
