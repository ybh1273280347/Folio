//! Appearance and shortcut preferences are persisted independently of project files and PDF typesetting.
use crate::{
    model::{AppSettings, MathFont, SHORTCUT_DEFS},
    storage,
};
use gpui_kit::{
    assets::IconName,
    base::StyledExt,
    component::{
        select::{SearchableVec, Select, SelectEvent, SelectState},
        slider::{Slider, SliderEvent, SliderState},
    },
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreferencesTab {
    Appearance,
    Shortcuts,
}

pub struct SettingsChanged(pub AppSettings);
pub struct Preferences {
    settings: AppSettings,
    tab: PreferencesTab,
    recording: Option<&'static str>,
    focus: FocusHandle,
    interface_font: Entity<SelectState<SearchableVec<String>>>,
    editor_font: Entity<SelectState<SearchableVec<String>>>,
    math_font: Entity<SelectState<SearchableVec<String>>>,
    sizes: [Entity<SliderState>; 3],
    math_scale: Entity<SliderState>,
    inline_math_scale: Entity<SliderState>,
    error: Option<String>,
}
impl EventEmitter<SettingsChanged> for Preferences {}

impl Preferences {
    pub fn new(settings: AppSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fonts = cx.text_system().all_font_names();
        fonts.sort();
        fonts.dedup();
        fonts.insert(0, "系统默认".into());
        let interface_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts.clone()), None, window, cx).searchable(true)
        });
        let editor_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true)
        });
        let sizes = std::array::from_fn(|index| {
            cx.new(|_| {
                SliderState::new()
                    .min(11.)
                    .max(match index {
                        0 => 20.,
                        _ => 26.,
                    })
                    .step(1.)
                    .default_value(
                        [
                            settings.interface_font_size,
                            settings.editor_font_size,
                            settings.markdown_font_size,
                        ][index] as f32,
                    )
            })
        });
        for (index, slider) in sizes.iter().enumerate() {
            cx.subscribe(slider, move |panel, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event else {
                    return;
                };
                let size = value.end().round() as u8;
                match index {
                    0 => panel.settings.interface_font_size = size,
                    1 => panel.settings.editor_font_size = size,
                    _ => panel.settings.markdown_font_size = size,
                }
                panel.changed(cx);
            })
            .detach();
        }
        let math_scale = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(3.)
                .step(0.1)
                .default_value(settings.math_scale)
        });
        cx.subscribe(&math_scale, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.math_scale = (value.end() * 10.).round() / 10.;
                panel.changed(cx);
            }
        })
        .detach();
        let math_font = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    MathFont::ALL
                        .iter()
                        .map(|font| font.family().to_owned())
                        .collect::<Vec<_>>(),
                ),
                None,
                window,
                cx,
            )
        });
        math_font.update(cx, |state, cx| {
            state.set_selected_value(&settings.math_font.family().to_owned(), window, cx)
        });
        cx.subscribe(
            &math_font,
            |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                if let SelectEvent::Confirm(Some(family)) = event {
                    panel.settings.math_font = MathFont::ALL
                        .into_iter()
                        .find(|font| font.family() == family)
                        .expect("listed math font");
                    panel.changed(cx);
                }
            },
        )
        .detach();
        let inline_math_scale = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(3.)
                .step(0.1)
                .default_value(settings.inline_math_scale)
        });
        cx.subscribe(&inline_math_scale, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.inline_math_scale = (value.end() * 10.).round() / 10.;
                panel.changed(cx);
            }
        })
        .detach();
        for (state, selected) in [
            (&interface_font, &settings.interface_font),
            (&editor_font, &settings.editor_font),
        ] {
            state.update(cx, |state, cx| {
                state.set_selected_value(
                    &if selected.is_empty() {
                        "系统默认".into()
                    } else {
                        selected.clone()
                    },
                    window,
                    cx,
                )
            });
        }
        cx.subscribe(
            &interface_font,
            |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(Some(font)) = event else {
                    return;
                };
                panel.settings.interface_font = if font == "系统默认" {
                    String::new()
                } else {
                    font.clone()
                };
                panel.changed(cx);
            },
        )
        .detach();
        cx.subscribe(
            &editor_font,
            |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(Some(font)) = event else {
                    return;
                };
                panel.settings.editor_font = if font == "系统默认" {
                    String::new()
                } else {
                    font.clone()
                };
                panel.changed(cx);
            },
        )
        .detach();
        Self {
            settings,
            tab: PreferencesTab::Appearance,
            recording: None,
            focus: cx.focus_handle(),
            interface_font,
            editor_font,
            math_font,
            math_scale,
            inline_math_scale,
            sizes,
            error: None,
        }
    }
    pub fn sync(&mut self, settings: &AppSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        self.recording = None;
        for (slider, size) in self.sizes.iter().zip([
            settings.interface_font_size,
            settings.editor_font_size,
            settings.markdown_font_size,
        ]) {
            slider.update(cx, |state, cx| state.set_value(size as f32, window, cx));
        }
        self.math_scale.update(cx, |state, cx| {
            state.set_value(settings.math_scale, window, cx)
        });
        self.inline_math_scale.update(cx, |state, cx| {
            state.set_value(settings.inline_math_scale, window, cx)
        });
        self.math_font.update(cx, |state, cx| {
            state.set_selected_value(&settings.math_font.family().to_owned(), window, cx)
        });
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.error = storage::save_settings(&self.settings)
            .err()
            .map(|error| error.to_string());
        cx.emit(SettingsChanged(self.settings.clone()));
        cx.notify();
    }
    fn size_row(
        &self,
        id: &'static str,
        label: &'static str,
        value: u8,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let index = match id {
            "ui-size" => 0,
            "editor-size" => 1,
            _ => 2,
        };
        let _ = cx;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label)
                    .child(format!("{value} px")),
            )
            .child(Slider::new(&self.sizes[index]))
            .into_any_element()
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .child("外观设置"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("界面字体")
                    .child(Select::new(&self.interface_font)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("编辑器字体")
                    .child(Select::new(&self.editor_font)),
            )
            .child(self.size_row("ui-size", "界面字号", self.settings.interface_font_size, cx))
            .child(self.size_row("editor-size", "编辑器字号", self.settings.editor_font_size, cx))
            .child(self.size_row("markdown-size", "Markdown 字号", self.settings.markdown_font_size, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("数学字体")
                    .child(Select::new(&self.math_font)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child("块级公式 / 正文字号")
                            .child(format!("{:.1}×", self.settings.math_scale)),
                    )
                    .child(Slider::new(&self.math_scale)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child("行内公式 / 正文字号")
                            .child(format!("{:.1}×", self.settings.inline_math_scale)),
                    )
                    .child(Slider::new(&self.inline_math_scale)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("编辑器限制行宽")
                    .child(
                        control(
                            "limit-width",
                            if self.settings.line_width_limited {
                                "已开启"
                            } else {
                                "已关闭"
                            },
                        )
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.settings.line_width_limited = !panel.settings.line_width_limited;
                            panel.changed(cx);
                        })),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .rounded(px(5.))
                    .bg(rgb(0xf7f6f5))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .font_family(if self.settings.interface_font.is_empty() {
                        SharedString::from("Segoe UI")
                    } else {
                        self.settings.interface_font.clone().into()
                    })
                    .child("Aa 中文字体")
                    .child(
                        div()
                            .font_family(if self.settings.editor_font.is_empty() {
                                SharedString::from("Consolas")
                            } else {
                                self.settings.editor_font.clone().into()
                            })
                            .text_size(px(self.settings.editor_font_size as f32))
                            .child("\\section{Introduction}"),
                    ),
            )
            .child(
                div().flex().justify_start().child(
                    control("reset-appearance", "恢复默认外观").on_click(cx.listener(
                        |panel, _, window, cx| {
                            let keybindings = panel.settings.keybindings.clone();
                            panel.settings = AppSettings {
                                keybindings,
                                ..AppSettings::default()
                            };
                            let settings = panel.settings.clone();
                            panel.sync(&settings, window, cx);
                            panel.interface_font.update(cx, |state, cx| {
                                state.set_selected_value(&"系统默认".into(), window, cx)
                            });
                            panel.editor_font.update(cx, |state, cx| {
                                state.set_selected_value(&"系统默认".into(), window, cx)
                            });
                            panel.changed(cx);
                        },
                    )),
                ),
            )
            .into_any_element()
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut categories: Vec<&'static str> = Vec::new();
        for def in SHORTCUT_DEFS {
            if !categories.contains(&def.category) {
                categories.push(def.category);
            }
        }
        let sections = categories.into_iter().map(|cat| {
            let rows = SHORTCUT_DEFS
                .iter()
                .filter(|def| def.category == cat)
                .map(|def| {
                    let id = def.id;
                    let current = self.settings.key_for(id);
                    let is_recording = self.recording == Some(id);
                    let is_modified = self.settings.keybindings.contains_key(id)
                        && current != def.default_key;
                    let conflict = SHORTCUT_DEFS
                        .iter()
                        .find(|other| {
                            other.id != id
                                && self
                                    .settings
                                    .key_for(other.id)
                                    .eq_ignore_ascii_case(&current)
                        })
                        .map(|other| other.label);
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py_2()
                        .px_3()
                        .rounded(px(6.))
                        .hover(|s| s.bg(rgb(0xf7f6f5)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_sm().child(def.label))
                                .when_some(conflict, |row, other| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .bg(rgb(0xfef3c7))
                                            .text_color(rgb(0xb45309))
                                            .child(format!("与「{other}」冲突")),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .when(is_modified, |row| {
                                    row.child(
                                        crate::ui::control(
                                            SharedString::from(format!("reset-key-{id}")),
                                            IconName::RotateCcw,
                                        )
                                        .on_click(cx.listener(move |panel, _, _, cx| {
                                            panel.settings.keybindings.remove(id);
                                            panel.recording = None;
                                            panel.changed(cx);
                                        })),
                                    )
                                })
                                .child(
                                    div()
                                        .id(SharedString::from(format!("record-key-{id}")))
                                        .px_3()
                                        .py_1()
                                        .min_w(px(110.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.))
                                        .border_1()
                                        .cursor_pointer()
                                        .text_xs()
                                        .font_family("Consolas")
                                        .when(is_recording, |badge| {
                                            badge
                                                .bg(rgb(0xeef4fb))
                                                .border_color(rgb(0x4b7bb5))
                                                .text_color(rgb(0x2b5b94))
                                                .child("请按下快捷键… (Esc取消)")
                                        })
                                        .when(!is_recording, |badge| {
                                            badge
                                                .bg(rgb(0xfafaf9))
                                                .border_color(rgb(0xe2e1dd))
                                                .text_color(rgb(0x37352f))
                                                .hover(|s| s.bg(rgb(0xefefed)))
                                                .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                                                .child(AppSettings::format_key_badge(&current))
                                        })
                                        .on_click(cx.listener(move |panel, _, window, cx| {
                                            panel.recording = if panel.recording == Some(id) {
                                                None
                                            } else {
                                                Some(id)
                                            };
                                            window.focus(&panel.focus, cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                });
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(rgb(0x787774))
                        .px_3()
                        .pb_1()
                        .child(cat),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .border_1()
                        .border_color(rgb(0xe8e7e3))
                        .rounded(px(6.))
                        .p_1()
                        .children(rows),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_lg().font_semibold().child("快捷键设置"))
                    .child(
                        control("reset-all-shortcuts", "全部恢复默认").on_click(cx.listener(
                            |panel, _, _, cx| {
                                panel.settings.keybindings.clear();
                                panel.recording = None;
                                panel.changed(cx);
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x787774))
                    .child("点击右侧按键框后直接按下新的组合键即可修改；在 Markdown 所见即所得模式下还可随时输入 / 呼出快捷块菜单。"),
            )
            .children(sections)
            .into_any_element()
    }
}
fn control(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    crate::ui::button_secondary(id, label).px_3()
}
impl Render for Preferences {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav_item = |id: &'static str,
                        tab: PreferencesTab,
                        icon_name: IconName,
                        title: &'static str,
                        active: bool,
                        cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_sm()
                .when(active, |s| {
                    s.bg(rgb(0xe8e7e4))
                        .font_semibold()
                        .text_color(rgb(0x1f2328))
                        .active(|a| a.bg(rgb(0xdddcda)))
                })
                .when(!active, |s| {
                    s.text_color(rgb(0x57606a))
                        .hover(|h| h.bg(rgb(0xefefed)))
                        .active(|a| a.bg(rgb(crate::ui::PRESSED_BG)))
                })
                .child(crate::ui::icon(icon_name))
                .child(title)
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.tab = tab;
                    panel.recording = None;
                    cx.notify();
                }))
        };
        let body = match self.tab {
            PreferencesTab::Appearance => self.render_appearance(cx),
            PreferencesTab::Shortcuts => self.render_shortcuts(cx),
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|panel, event: &KeyDownEvent, _, cx| {
                let Some(id) = panel.recording else {
                    return;
                };
                cx.stop_propagation();
                let key = event.keystroke.key.to_ascii_lowercase();
                if matches!(
                    key.as_str(),
                    "control" | "ctrl" | "shift" | "alt" | "meta" | "super" | "win"
                ) {
                    return;
                }
                if key == "escape"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.shift
                {
                    panel.recording = None;
                    cx.notify();
                    return;
                }
                let mut parts = Vec::new();
                if event.keystroke.modifiers.control || event.keystroke.modifiers.platform {
                    parts.push("ctrl".to_owned());
                }
                if event.keystroke.modifiers.alt {
                    parts.push("alt".to_owned());
                }
                if event.keystroke.modifiers.shift {
                    parts.push("shift".to_owned());
                }
                parts.push(key);
                let combo = parts.join("-");
                panel.settings.keybindings.insert(id.to_owned(), combo);
                panel.recording = None;
                panel.changed(cx);
            }))
            .flex()
            .w_full()
            .min_h(px(460.))
            .gap_6()
            .text_color(rgb(0x37352f))
            .child(
                div()
                    .w(px(148.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pr_4()
                    .border_r_1()
                    .border_color(rgb(0xe8e7e3))
                    .child(
                        div()
                            .px_3()
                            .pb_2()
                            .text_base()
                            .font_semibold()
                            .child("设置"),
                    )
                    .child(nav_item(
                        "pref-tab-appearance",
                        PreferencesTab::Appearance,
                        IconName::Palette,
                        "外观",
                        self.tab == PreferencesTab::Appearance,
                        cx,
                    ))
                    .child(nav_item(
                        "pref-tab-shortcuts",
                        PreferencesTab::Shortcuts,
                        IconName::Keyboard,
                        "快捷键",
                        self.tab == PreferencesTab::Shortcuts,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(body)
                    .children(
                        self.error
                            .iter()
                            .map(|error| div().text_color(rgb(0xa16b60)).child(error.clone())),
                    ),
            )
    }
}
