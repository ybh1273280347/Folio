//! Appearance and shortcut preferences are persisted independently of project files and PDF typesetting.
use crate::theme::color as rgb;
use crate::{
    model::{AppSettings, InternalLinkOpenTarget, MathFont, SHORTCUT_DEFS, ToolExecutionView},
    storage,
    theme::ColorRole,
};
use gpui_kit::{
    assets::IconName,
    base::StyledExt,
    component::{
        color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        radio::{Radio, RadioGroup},
        select::{SearchableVec, Select, SelectEvent, SelectState},
        slider::{Slider, SliderEvent, SliderState},
    },
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreferencesTab {
    Fonts,
    Theme,
    View,
    Shortcuts,
    Terminal,
    Api,
}

pub struct SettingsChanged(pub AppSettings);
pub struct Preferences {
    settings: AppSettings,
    tab: PreferencesTab,
    appearance_expanded: bool,
    recording: Option<&'static str>,
    focus: FocusHandle,
    interface_font: Entity<SelectState<SearchableVec<String>>>,
    editor_font: Entity<SelectState<SearchableVec<String>>>,
    markdown_font: Entity<SelectState<SearchableVec<String>>>,
    ai_font: Entity<SelectState<SearchableVec<String>>>,
    cjk_fallback_font: Entity<SelectState<SearchableVec<String>>>,
    inline_code_font: Entity<SelectState<SearchableVec<String>>>,
    block_code_font: Entity<SelectState<SearchableVec<String>>>,
    inline_code_scale: Entity<SliderState>,
    block_code_scale: Entity<SliderState>,
    math_font: Entity<SelectState<SearchableVec<String>>>,
    sizes: [Entity<SliderState>; 4],
    math_scale: Entity<SliderState>,
    inline_math_scale: Entity<SliderState>,
    background_image_opacity: Entity<SliderState>,
    background_image_error: Option<String>,
    theme_name: Entity<InputState>,
    theme_colors: Vec<(ColorRole, Entity<ColorPickerState>)>,
    terminal_shell: Entity<InputState>,
    terminal_args: Entity<TextareaState>,
    terminal_font_family: Entity<SelectState<SearchableVec<String>>>,
    terminal_font_size: Entity<SliderState>,
    terminal_panel_width: Entity<SliderState>,
    baidu_app_id: Entity<InputState>,
    baidu_api_key: Entity<InputState>,
    ai_base_url: Entity<InputState>,
    ai_title_model: Entity<InputState>,
    ai_exa_key: Entity<InputState>,
    ai_qianfan_key: Entity<InputState>,
    copied_api_field: Option<&'static str>,
    copied_api_task: Task<()>,
    editing_theme: bool,
    error: Option<String>,
}
impl EventEmitter<SettingsChanged> for Preferences {}

impl Preferences {
    pub fn new(settings: AppSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let background_image_opacity = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(f32::from(settings.background_image_opacity))
        });
        cx.subscribe(&background_image_opacity, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                let opacity = value.end().round() as u8;
                if panel.settings.background_image_opacity != opacity {
                    panel.settings.background_image_opacity = opacity;
                    panel.changed(cx);
                }
            }
        })
        .detach();
        let theme_name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("主题名称");
            if settings.theme.active != "自定义（未保存）" {
                input.set_value(settings.theme.active.clone(), window, cx);
            }
            input
        });
        let theme_colors = ColorRole::ALL.iter().map(|(role, _)| {
            let role = *role;
            let picker = cx.new(|cx| {
                let mut state = ColorPickerState::new(window, cx);
                state.set_value(gpui_kit::rgb(settings.theme.palette.get(role)), window, cx);
                state
            });
            cx.subscribe(&picker, move |panel, _, event: &ColorPickerEvent, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    // Pickers can supply alpha; theme surfaces are deliberately opaque.
                    let color = color.to_rgb();
                    let hex = ((color.r * 255.).round() as u32) << 16
                        | ((color.g * 255.).round() as u32) << 8
                        | (color.b * 255.).round() as u32;
                    panel.settings.theme.palette.set(role, hex);
                    panel.settings.theme.active = "自定义（未保存）".into();
                    panel.changed(cx);
                }
            }).detach();
            (role, picker)
        }).collect();
        let terminal_shell = cx.new(|cx| InputState::new(window, cx).placeholder("自动使用 PowerShell"));
        if !settings.terminal.shell.is_empty() {
            terminal_shell.update(cx, |input, cx| {
                input.set_value(settings.terminal.shell.clone(), window, cx);
            });
        }
        cx.subscribe(&terminal_shell, |panel, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                panel.settings.terminal.shell = state.read(cx).value().to_string();
                panel.changed(cx);
            }
        })
        .detach();
        let terminal_args_value = settings.terminal.args.join("\n");
        let terminal_args = cx.new(|cx| {
            TextareaState::new(window, cx).default_value(terminal_args_value)
        });
        cx.subscribe(&terminal_args, |panel, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let lines = state.read(cx).value().to_string();
                // Each line is already one process argument; quotes and spaces stay literal.
                panel.settings.terminal.args = lines.lines().map(str::to_owned).collect();
                panel.changed(cx);
            }
        })
        .detach();
        let terminal_font_size = cx.new(|_| {
            SliderState::new()
                .min(10.)
                .max(32.)
                .step(1.)
                .default_value(settings.terminal.font_size as f32)
        });
        cx.subscribe(&terminal_font_size, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.terminal.font_size = value.end().round() as u8;
                panel.changed(cx);
            }
        })
        .detach();
        let terminal_panel_width = cx.new(|_| {
            SliderState::new()
                .min(280.)
                .max(1600.)
                .step(10.)
                .default_value(settings.terminal.panel_width)
        });
        cx.subscribe(&terminal_panel_width, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.terminal.panel_width = (value.end() / 10.).round() * 10.;
                panel.changed(cx);
            }
        })
        .detach();
        let mut fonts = cx.text_system().all_font_names();
        fonts.sort();
        fonts.dedup();
        let terminal_font_family = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts.clone()), None, window, cx).searchable(true)
        });
        terminal_font_family.update(cx, |state, cx| {
            state.set_selected_value(
                &if settings.terminal.font_family.is_empty() {
                    "Consolas".into()
                } else {
                    settings.terminal.font_family.clone()
                },
                window,
                cx,
            );
        });
        cx.subscribe(
            &terminal_font_family,
            |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                if let SelectEvent::Confirm(Some(font)) = event {
                    panel.settings.terminal.font_family = font.clone();
                    panel.changed(cx);
                }
            },
        ).detach();
        fonts.insert(0, "系统默认".into());
        let mut font_selector = || {
            cx.new(|cx| {
                SelectState::new(SearchableVec::new(fonts.clone()), None, window, cx).searchable(true)
            })
        };
        let interface_font = font_selector();
        let editor_font = font_selector();
        let markdown_font = font_selector();
        let ai_font = font_selector();
        let cjk_fallback_font = font_selector();
        fonts[0] = "默认等宽字体".into();
        let inline_code_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts.clone()), None, window, cx).searchable(true)
        });
        let block_code_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true)
        });
        let inline_code_scale = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(1.5)
                .step(0.025)
                .default_value(settings.inline_code_scale)
        });
        cx.subscribe(&inline_code_scale, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.inline_code_scale = (value.end() * 40.).round() / 40.;
                panel.changed(cx);
            }
        })
        .detach();
        let block_code_scale = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(1.5)
                .step(0.025)
                .default_value(settings.block_code_scale)
        });
        cx.subscribe(&block_code_scale, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.block_code_scale = (value.end() * 40.).round() / 40.;
                panel.changed(cx);
            }
        })
        .detach();
        for (selector, selected, inline) in [
            (&inline_code_font, &settings.inline_code_font, true),
            (&block_code_font, &settings.block_code_font, false),
        ] {
            selector.update(cx, |state, cx| {
                state.set_selected_value(
                    &if selected.is_empty() {
                        "默认等宽字体".into()
                    } else {
                        selected.clone()
                    },
                    window,
                    cx,
                );
            });
            cx.subscribe(
                selector,
                move |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                    if let SelectEvent::Confirm(Some(font)) = event {
                        let selected = if inline {
                            &mut panel.settings.inline_code_font
                        } else {
                            &mut panel.settings.block_code_font
                        };
                        *selected = if font == "默认等宽字体" { String::new() } else { font.clone() };
                        panel.changed(cx);
                    }
                },
            )
            .detach();
        }
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
                            settings.ai_font_size,
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
                    2 => panel.settings.markdown_font_size = size,
                    _ => panel.settings.ai_font_size = size,
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
        let font_selectors: [(_, _, fn(&mut AppSettings) -> &mut String); 5] = [
            (&interface_font, &settings.interface_font, |s| &mut s.interface_font),
            (&editor_font, &settings.editor_font, |s| &mut s.editor_font),
            (&markdown_font, &settings.markdown_font, |s| &mut s.markdown_font),
            (&ai_font, &settings.ai_font, |s| &mut s.ai_font),
            (&cjk_fallback_font, &settings.cjk_fallback_font, |s| &mut s.cjk_fallback_font),
        ];
        for (state, selected, field) in font_selectors {
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
            cx.subscribe(state, move |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                if let SelectEvent::Confirm(Some(font)) = event {
                    *field(&mut panel.settings) = if font == "系统默认" { String::new() } else { font.clone() };
                    panel.changed(cx);
                }
            })
            .detach();
        }
        let baidu_app_id = cx.new(|cx| InputState::new(window, cx).placeholder("填写百度翻译 APP ID").masked(true));
        if !settings.translate.baidu_app_id.is_empty() {
            baidu_app_id.update(cx, |input, cx| {
                input.set_value(settings.translate.baidu_app_id.clone(), window, cx);
            });
        }
        cx.subscribe(&baidu_app_id, |panel, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                panel.settings.translate.baidu_app_id = state.read(cx).value().trim().to_owned();
                panel.changed(cx);
            }
        })
        .detach();
        let baidu_api_key = cx.new(|cx| InputState::new(window, cx).placeholder("填写百度翻译密钥（API Key）").masked(true));
        if !settings.translate.baidu_api_key.is_empty() {
            baidu_api_key.update(cx, |input, cx| {
                input.set_value(settings.translate.baidu_api_key.clone(), window, cx);
            });
        }
        cx.subscribe(&baidu_api_key, |panel, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                panel.settings.translate.baidu_api_key = state.read(cx).value().trim().to_owned();
                panel.changed(cx);
            }
        })
        .detach();
        let ai_base_url = cx.new(|cx| InputState::new(window, cx).placeholder("http://127.0.0.1:3425"));
        if !settings.ai.base_url.is_empty() {
            ai_base_url.update(cx, |input, cx| {
                input.set_value(settings.ai.base_url.clone(), window, cx);
            });
        }
        cx.subscribe(&ai_base_url, |panel, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                panel.settings.ai.base_url = state.read(cx).value().trim().to_owned();
                panel.changed(cx);
            }
        })
        .detach();
        let ai_title_model = cx.new(|cx| InputState::new(window, cx).placeholder("workbuddy-ai/hy4flash-free"));
        ai_title_model.update(cx, |input,cx| input.set_value(settings.ai.title_model_id.clone(),window,cx));
        cx.subscribe(&ai_title_model, |panel,input,event: &InputEvent,cx| {
            if matches!(event,InputEvent::Change) { panel.settings.ai.title_model_id=input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let ai_exa_key = cx.new(|cx| InputState::new(window, cx).placeholder("Exa API Key").masked(true));
        ai_exa_key.update(cx, |input,cx| input.set_value(settings.ai.exa_api_key.clone(),window,cx));
        cx.subscribe(&ai_exa_key, |panel,input,event: &InputEvent,cx| {
            if matches!(event,InputEvent::Change) { panel.settings.ai.exa_api_key=input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let ai_qianfan_key = cx.new(|cx| InputState::new(window, cx).placeholder("百度千帆 API Key").masked(true));
        ai_qianfan_key.update(cx, |input,cx| input.set_value(settings.ai.baidu_qianfan_api_key.clone(),window,cx));
        cx.subscribe(&ai_qianfan_key, |panel,input,event: &InputEvent,cx| {
            if matches!(event,InputEvent::Change) { panel.settings.ai.baidu_qianfan_api_key=input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        Self {
            settings,
            tab: PreferencesTab::Fonts,
            appearance_expanded: true,
            recording: None,
            focus: cx.focus_handle(),
            interface_font,
            editor_font,
            markdown_font,
            ai_font,
            cjk_fallback_font,
            inline_code_font,
            block_code_font,
            inline_code_scale,
            block_code_scale,
            math_font,
            math_scale,
            inline_math_scale,
            background_image_opacity,
            background_image_error: None,
            sizes,
            theme_name,
            theme_colors,
            terminal_shell,
            terminal_args,
            terminal_font_family,
            terminal_font_size,
            terminal_panel_width,
            baidu_app_id,
            baidu_api_key,
            ai_base_url,
            ai_title_model,
            ai_exa_key,
            ai_qianfan_key,
            copied_api_field: None,
            copied_api_task: Task::ready(()),
            editing_theme: false,
            error: None,
        }
    }
    pub fn sync(&mut self, settings: &AppSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        self.background_image_opacity.update(cx, |state, cx| {
            state.set_value(f32::from(settings.background_image_opacity), window, cx)
        });
        self.sync_theme_colors(window, cx);
        if settings.theme.active != "自定义（未保存）" {
            self.theme_name.update(cx, |input, cx| {
                input.set_value(settings.theme.active.clone(), window, cx);
            });
        }
        self.recording = None;
        for (selector, selected) in [
            (&self.interface_font, &settings.interface_font),
            (&self.editor_font, &settings.editor_font),
            (&self.markdown_font, &settings.markdown_font),
            (&self.ai_font, &settings.ai_font),
            (&self.cjk_fallback_font, &settings.cjk_fallback_font),
        ] {
            selector.update(cx, |state, cx| {
                state.set_selected_value(
                    &if selected.is_empty() { "系统默认".into() } else { selected.clone() },
                    window,
                    cx,
                );
            });
        }
        self.inline_code_scale.update(cx, |state, cx| {
            state.set_value(settings.inline_code_scale, window, cx)
        });
        self.block_code_scale.update(cx, |state, cx| {
            state.set_value(settings.block_code_scale, window, cx)
        });
        for (selector, selected) in [
            (&self.inline_code_font, &settings.inline_code_font),
            (&self.block_code_font, &settings.block_code_font),
        ] {
            selector.update(cx, |state, cx| {
                state.set_selected_value(
                    &if selected.is_empty() { "默认等宽字体".into() } else { selected.clone() },
                    window,
                    cx,
                );
            });
        }
        for (slider, size) in self.sizes.iter().zip([
            settings.interface_font_size,
            settings.editor_font_size,
            settings.markdown_font_size,
            settings.ai_font_size,
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
        self.terminal_shell.update(cx, |input, cx| {
            input.set_value(settings.terminal.shell.clone(), window, cx);
        });
        let terminal_args = settings.terminal.args.join("\n");
        self.terminal_args.update(cx, |input, cx| {
            input.set_value(terminal_args.clone(), window, cx);
        });
        self.terminal_font_family.update(cx, |state, cx| {
            state.set_selected_value(
                &if settings.terminal.font_family.is_empty() {
                    "Consolas".into()
                } else {
                    settings.terminal.font_family.clone()
                },
                window,
                cx,
            );
        });
        self.terminal_font_size.update(cx, |state, cx| {
            state.set_value(settings.terminal.font_size as f32, window, cx)
        });
        self.terminal_panel_width.update(cx, |state, cx| {
            state.set_value(settings.terminal.panel_width, window, cx)
        });
        self.baidu_app_id.update(cx, |input, cx| {
            input.set_value(settings.translate.baidu_app_id.clone(), window, cx);
        });
        self.baidu_api_key.update(cx, |input, cx| {
            input.set_value(settings.translate.baidu_api_key.clone(), window, cx);
        });
        self.ai_base_url.update(cx, |input, cx| {
            input.set_value(settings.ai.base_url.clone(), window, cx);
        });
        self.ai_title_model.update(cx, |input,cx| input.set_value(settings.ai.title_model_id.clone(),window,cx));
        self.ai_exa_key.update(cx, |input,cx| input.set_value(settings.ai.exa_api_key.clone(),window,cx));
        self.ai_qianfan_key.update(cx, |input,cx| input.set_value(settings.ai.baidu_qianfan_api_key.clone(),window,cx));
    }
    pub fn set_background_image_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.background_image_error = error;
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.error = storage::save_settings(&self.settings)
            .err()
            .map(|error| error.to_string());
        cx.emit(SettingsChanged(self.settings.clone()));
        cx.notify();
    }

    fn sync_theme_colors(&self, window: &mut Window, cx: &mut Context<Self>) {
        for (role, picker) in &self.theme_colors {
            picker.update(cx, |state, cx| {
                state.set_value(gpui_kit::rgb(self.settings.theme.palette.get(*role)), window, cx);
            });
        }
    }

    fn select_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme.select(name);
        self.sync_theme_colors(window, cx);
        self.theme_name.update(cx, |input, cx| {
            input.set_value(name, window, cx);
        });
        self.changed(cx);
    }

    fn render_theme(&self, cx: &mut Context<Self>) -> AnyElement {
        let choices = ["浅色".to_owned(), "深色".to_owned()].into_iter()
            .chain(self.settings.theme.saved.keys()
                .filter(|name| !matches!(name.as_str(), "浅色" | "深色")).cloned())
            .map(|name| {
                let selected = self.settings.theme.active == name;
                let select_name = name.clone();
                crate::ui::button_secondary(SharedString::from(format!("theme-{name}")), name)
                    .when(selected, |button| button.bg(rgb(0xe8e7e4)).border_color(rgb(0x52769a)))
                    .on_click(cx.listener(move |panel, _, window, cx| panel.select_theme(&select_name, window, cx)))
            });
        let colors = self.theme_colors.iter().zip(ColorRole::ALL).map(|((_, picker), (_, label))| {
            div().flex().items_center().justify_between().gap_3()
                .child(label).child(ColorPicker::new(picker))
        });
        let custom_selected = !matches!(self.settings.theme.active.as_str(), "浅色" | "深色")
            && self.settings.theme.saved.contains_key(&self.settings.theme.active);
        div().flex().flex_col().gap_3()
            .child(div().font_semibold().child("主题"))
            .child(div().flex().flex_wrap().gap_2().children(choices))
            .child(div().flex().items_center().justify_between()
                .child(format!("当前：{}", self.settings.theme.active))
                .child(crate::ui::button_secondary("edit-theme", if self.editing_theme { "收起配色" } else { "编辑配色" })
                    .on_click(cx.listener(|panel, _, _, cx| { panel.editing_theme = !panel.editing_theme; cx.notify(); }))))
            .when(self.editing_theme, |section| section
            .child(div().flex().flex_col().gap_1().children(colors))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(Input::new(&self.theme_name)))
                .child(crate::ui::button_secondary("save-theme", "保存主题")
                    .on_click(cx.listener(|panel, _, _, cx| {
                        let name = panel.theme_name.read(cx).value().to_string();
                        match panel.settings.theme.save(&name) {
                            Ok(()) => panel.changed(cx),
                            Err(error) => { panel.error = Some(error.into()); cx.notify(); }
                        }
                    })))
                .when(custom_selected, |row| row.child(crate::ui::button("delete-theme", "删除主题")
                    .on_click(cx.listener(|panel, _, window, cx| {
                        panel.settings.theme.saved.remove(&panel.settings.theme.active);
                        panel.select_theme("浅色", window, cx);
                    }))))))
            .child(div().flex().justify_start().child(
                control("reset-theme", "恢复默认主题")
                    .on_click(cx.listener(|panel, _, window, cx| {
                        panel.settings.theme.restore_presets();
                        panel.select_theme("浅色", window, cx);
                    })),
            ))
            .into_any_element()
    }
    fn render_fonts(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = &self.settings;
        let palette = settings.theme.palette;
        let section = |title: &'static str| {
            div().flex().flex_col().gap_4().border_t_1().border_color(rgb(palette.border)).pt_4()
                .child(div().font_semibold().child(title))
        };
        let slider_row = |label: &'static str, value: String, slider: &Entity<SliderState>| {
            div().flex().flex_col().gap_2()
                .child(div().flex().items_center().justify_between().gap_2()
                    .child(label)
                    .child(div().flex_shrink_0().text_color(rgb(palette.muted)).child(value)))
                .child(Slider::new(slider))
        };
        let font_row = |label: &'static str,
                        family: &Entity<SelectState<SearchableVec<String>>>,
                        size: &Entity<SliderState>,
                        value: u8| {
            div().flex().flex_col().gap_2()
                .child(label)
                .child(div().flex().items_center().gap_4()
                    .child(div().flex_1().min_w_0().child(Select::new(family).w_full()))
                    .child(slider_row("字号", format!("{value} px"), size).w(px(112.)).flex_shrink_0()))
        };
        let [interface_size, editor_size, markdown_size, ai_size] = &self.sizes;
        let surfaces = section("各区域字体")
            .child(font_row("界面 · 左侧栏和设置", &self.interface_font, interface_size, settings.interface_font_size))
            .child(font_row("Markdown 预览", &self.markdown_font, markdown_size, settings.markdown_font_size))
            .child(font_row("AI 面板", &self.ai_font, ai_size, settings.ai_font_size))
            .child(font_row("终端", &self.terminal_font_family, &self.terminal_font_size, settings.terminal.font_size))
            .child(font_row("源码编辑器", &self.editor_font, editor_size, settings.editor_font_size))
            .child(div().p_3().rounded(px(5.)).bg(rgb(palette.background))
                .font_family(if settings.editor_font.is_empty() {
                    SharedString::from("Consolas")
                } else {
                    settings.editor_font.clone().into()
                })
                .text_size(px(settings.editor_font_size as f32))
                .child("\\section{Introduction} % 中文注释"));

        let markdown_size = px(settings.markdown_font_size as f32);
        let preview_style = gpui_kit::base::text::TextViewStyle::from_theme(&gpui_kit::base::Theme::global(cx))
            .with_inline_code_style(crate::ui::inline_code_style(settings, markdown_size))
            .with_code_block(crate::ui::block_code_style(settings, markdown_size));
        let shared = section("Markdown 与 AI 共享排版")
            .child(div().text_sm().text_color(rgb(palette.muted))
                .child("代码和公式按各自的正文字号缩放。"))
            .child(div().flex().gap_4()
                .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                    .child("行内代码字体")
                    .child(Select::new(&self.inline_code_font).w_full())
                    .child(slider_row("字号比例", format!("{:.1}%", settings.inline_code_scale * 100.), &self.inline_code_scale)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                    .child("代码块字体")
                    .child(Select::new(&self.block_code_font).w_full())
                    .child(slider_row("字号比例", format!("{:.1}%", settings.block_code_scale * 100.), &self.block_code_scale))))
            .child(div().p_3().rounded(px(5.)).bg(rgb(palette.background))
                .font_family(crate::ui::sans_font(&settings.markdown_font))
                .text_size(markdown_size)
                .child(gpui_kit::base::text::markdown("调用 `merge(39, 61)` 合并两个结点。\n\n```rust\nlet root = merge(39, 61);\n```")
                    .style(preview_style)))
            .child(div().flex().flex_col().gap_2()
                .child("数学字体")
                .child(Select::new(&self.math_font).w_full()))
            .child(div().flex().gap_4()
                .child(slider_row("行内公式比例", format!("{:.1}×", settings.inline_math_scale), &self.inline_math_scale)
                    .flex_1().min_w_0())
                .child(slider_row("块级公式比例", format!("{:.1}×", settings.math_scale), &self.math_scale)
                    .flex_1().min_w_0()));

        let fallback = section("字体回退")
            .child(div().flex().flex_col().gap_2()
                .child(Select::new(&self.cjk_fallback_font).w_full())
                .child(div().text_sm().text_color(rgb(palette.muted))
                    .child("界面、终端、Markdown 预览和 AI 面板的主字体缺少字形时，优先使用此字体。"))
                .child(div().p_3().rounded(px(5.)).bg(rgb(palette.background))
                    .child("Aa 中文字体预览")));
        div().flex().flex_col().gap_5()
            .child(div().text_lg().font_semibold().child("外观 / 字体"))
            .child(surfaces)
            .child(shared)
            .child(fallback)
            .child(
                div().flex().justify_start().child(
                    control("reset-fonts", "恢复默认字体").on_click(cx.listener(
                        |panel, _, window, cx| {
                            // Each appearance page resets only its own settings.
                            let defaults = AppSettings::default();
                            panel.settings.interface_font = defaults.interface_font;
                            panel.settings.editor_font = defaults.editor_font;
                            panel.settings.markdown_font = defaults.markdown_font;
                            panel.settings.ai_font = defaults.ai_font;
                            panel.settings.cjk_fallback_font = defaults.cjk_fallback_font;
                            panel.settings.inline_code_font = defaults.inline_code_font;
                            panel.settings.block_code_font = defaults.block_code_font;
                            panel.settings.math_font = defaults.math_font;
                            panel.settings.interface_font_size = defaults.interface_font_size;
                            panel.settings.editor_font_size = defaults.editor_font_size;
                            panel.settings.markdown_font_size = defaults.markdown_font_size;
                            panel.settings.ai_font_size = defaults.ai_font_size;
                            panel.settings.inline_code_scale = defaults.inline_code_scale;
                            panel.settings.block_code_scale = defaults.block_code_scale;
                            panel.settings.math_scale = defaults.math_scale;
                            panel.settings.inline_math_scale = defaults.inline_math_scale;
                            panel.settings.terminal.font_family = defaults.terminal.font_family;
                            panel.settings.terminal.font_size = defaults.terminal.font_size;
                            let settings = panel.settings.clone();
                            panel.sync(&settings, window, cx);
                            panel.changed(cx);
                        },
                    )),
                ),
            )
            .into_any_element()
    }

    fn render_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let tool_views = [
            (ToolExecutionView::Minimal, "极简模式"),
            (ToolExecutionView::Standard, "标准模式"),
            (ToolExecutionView::Detailed, "详细模式"),
        ];
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_semibold().child("外观 / 视图"))
            .child(
                div().flex().flex_col().gap_2()
                    .child(
                        div().flex().items_center().justify_between().gap_3()
                            .child("背景图")
                            .child(
                                div().flex().gap_2()
                                    .child(control("choose-background-image", "选择图片").on_click(
                                        cx.listener(|_, _, window, cx| {
                                            let dialog = rfd::AsyncFileDialog::new()
                                                .set_parent(window)
                                                .set_title("选择背景图")
                                                .add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp"]);
                                            cx.spawn(async move |this, cx| {
                                                if let Some(file) = dialog.pick_file().await {
                                                    let _ = this.update(cx, |panel, cx| {
                                                        panel.settings.background_image = Some(file.path().to_path_buf());
                                                        panel.changed(cx);
                                                    });
                                                }
                                            }).detach();
                                        }),
                                    ))
                                    .when(self.settings.background_image.is_some(), |actions| {
                                        actions.child(control("remove-background-image", "移除图片").on_click(
                                            cx.listener(|panel, _, _, cx| {
                                                panel.settings.background_image = None;
                                                panel.changed(cx);
                                            }),
                                        ))
                                    }),
                            ),
                    )
                    .when_some(self.settings.background_image.as_ref(), |section, path| {
                        section
                            .child(div().min_w_0().text_ellipsis().text_sm()
                                .text_color(rgb(0x787774)).child(path.display().to_string()))
                            .child(div().flex().items_center().justify_between()
                                .child("图片不透明度")
                                .child(format!("{}%", self.settings.background_image_opacity)))
                            .child(Slider::new(&self.background_image_opacity))
                    })
                    .when_some(self.background_image_error.as_ref(), |section, error| {
                        section.child(div().text_sm().text_color(rgb(0xb92d2d)).child(error.clone()))
                    }),
            )
            .child(
                div().flex().items_center().justify_between().gap_3()
                    .child("工具执行视图")
                    .child(
                        RadioGroup::horizontal("tool-execution-view")
                            .flex_none()
                            .children(tool_views.iter().map(|&(_, label)| Radio::new(label).label(label)))
                            .selected_index(tool_views.iter().position(|&(mode, _)| mode == self.settings.tool_execution_view))
                            .on_change(cx.listener(move |panel, index: &usize, _, cx| {
                                panel.settings.tool_execution_view = tool_views[*index].0;
                                panel.changed(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("显示行号（TeX / Markdown 源码）")
                    .child(
                        control("show-line-numbers", if self.settings.show_line_numbers { "已开启" } else { "已关闭" })
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.settings.show_line_numbers = !panel.settings.show_line_numbers;
                                panel.changed(cx);
                            })),
                    ),
            )
            .child(
                div().flex().items_center().justify_between()
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
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child("内部文件链接跳转位置"))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        choice_button(
                                            "link-target-main",
                                            "主编辑区",
                                            self.settings.internal_link_open_target
                                                == InternalLinkOpenTarget::MainEditor,
                                        )
                                        .on_click(cx.listener(|panel, _, _, cx| {
                                            panel.settings.internal_link_open_target =
                                                InternalLinkOpenTarget::MainEditor;
                                            panel.changed(cx);
                                        })),
                                    )
                                    .child(
                                        choice_button(
                                            "link-target-split",
                                            "右侧分屏",
                                            self.settings.internal_link_open_target
                                                == InternalLinkOpenTarget::SplitPane,
                                        )
                                        .on_click(cx.listener(|panel, _, _, cx| {
                                            panel.settings.internal_link_open_target =
                                                InternalLinkOpenTarget::SplitPane;
                                            panel.changed(cx);
                                        })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(rgb(0x787774))
                            .child("Ctrl+左键点击 Markdown 内部文件链接时的打开方式（非分屏类型仍在主编辑区打开）"),
                    ),
            )
            .into_any_element()
    }

    fn render_terminal(&self) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_semibold().child("终端设置"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("Shell 命令")
                    .child(Input::new(&self.terminal_shell))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x787774))
                            .child("留空时自动使用 PowerShell。默认参数不含 -NoProfile，因此会加载 shell 的用户 profile。"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("启动参数")
                    .child(Textarea::new(&self.terminal_args).h(px(112.)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x787774))
                            .child("每行作为一个独立参数传给 shell；不会按空格、引号或 shell 语法拆分。"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child("终端面板宽度")
                            .child(format!("{:.0} px", self.settings.terminal.panel_width)),
                    )
                    .child(Slider::new(&self.terminal_panel_width)),
            )
            .into_any_element()
    }

    fn copy_api_value(&mut self, field: &'static str, value: String, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied_api_field = Some(field);
        cx.notify();
        self.copied_api_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |panel, cx| {
                if panel.copied_api_field == Some(field) {
                    panel.copied_api_field = None;
                    cx.notify();
                }
            });
        });
    }

    fn render_api(&self, cx: &mut Context<Self>) -> AnyElement {
        let baidu_ready = !self.settings.translate.baidu_app_id.trim().is_empty()
            && !self.settings.translate.baidu_api_key.trim().is_empty();
        let ai_ready = !self.settings.ai.base_url.trim().is_empty();

        let copy_btn = |id: &'static str, value: String, copied: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px_2()
                .rounded(px(5.))
                .border_1()
                .border_color(rgb(0xe2e1dd))
                .bg(rgb(0xffffff))
                .flex()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .cursor_pointer()
                .text_xs()
                .text_color(if copied { rgb(0x237844) } else { rgb(0x5b5b58) })
                .hover(|s| s.bg(rgb(0xf2f3f5)))
                .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                .child(
                    crate::ui::icon(if copied { IconName::Check } else { IconName::Copy })
                        .w(px(12.))
                        .h(px(12.)),
                )
                .child(if copied { "已复制" } else { "复制" })
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.copy_api_value(id, value.clone(), cx);
                }))
        };

        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_semibold().child("API 服务"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .rounded(px(8.))
                    .bg(rgb(0xfafaf8))
                    .border_1()
                    .border_color(rgb(0xe8e7e3))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .min_w_0()
                                    .child(div().font_semibold().child("AI 服务（Magpie 网关）"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x787774))
                                    .child("原生 AI 界面经由 pi SDK 宿主调用本地 Magpie 统一网关（免密钥）"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .when(ai_ready, |b| {
                                        b.bg(rgb(0xeaf5ee))
                                            .border_1()
                                            .border_color(rgb(0xc6e5d0))
                                            .text_color(rgb(0x237844))
                                    })
                                    .when(!ai_ready, |b| {
                                        b.bg(rgb(0xfff4e5))
                                            .border_1()
                                            .border_color(rgb(0xf2d3a6))
                                            .text_color(rgb(0x9a5b13))
                                    })
                                    .text_xs()
                                    .child(if ai_ready { "已配置" } else { "请填写地址" }),
                            ),
                    )
                    .child(div().h(px(1.)).bg(rgb(0xebebe6)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(96.))
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(rgb(0x5b5b58))
                                    .child("Base URL"),
                            )
                            .child(div().flex_1().min_w_0().child(Input::new(&self.ai_base_url)))
                            .child(copy_btn(
                                "copy-ai-base-url",
                                self.settings.ai.base_url.clone(),
                                self.copied_api_field == Some("copy-ai-base-url"),
                            ))
                            .child(
                                crate::ui::button_secondary("reset-ai-base-url", "默认")
                                    .px_2()
                                    .on_click(cx.listener(|panel, _, window, cx| {
                                        let default_url = crate::model::AiSettings::default().base_url;
                                        panel.settings.ai.base_url = default_url.clone();
                                        panel.ai_base_url.update(cx, |input, cx| {
                                            input.set_value(default_url, window, cx);
                                        });
                                        panel.changed(cx);
                                    })),
                            ),
                    )
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(96.)).text_xs().child("标题模型 ID"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.ai_title_model))))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(96.)).text_xs().child("Exa API Key"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.ai_exa_key).mask_toggle())))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(96.)).text_xs().child("千帆 API Key"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.ai_qianfan_key).mask_toggle()))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .rounded(px(8.))
                    .bg(rgb(0xfafaf8))
                    .border_1()
                    .border_color(rgb(0xe8e7e3))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(div().font_semibold().child("翻译服务"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x787774))
                                    .child("支持中英互译 · 优先 Google，失败时自动切换百度"),
                            ),
                    )
                    .child(div().h(px(1.)).bg(rgb(0xebebe6)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .min_w_0()
                                    .child(div().font_medium().child("Google 翻译"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x787774))
                                            .child("内置网页翻译接口，无需填写 API Key（依赖当前网络或系统代理）"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .bg(rgb(0xeaf5ee))
                                    .border_1()
                                    .border_color(rgb(0xc6e5d0))
                                    .text_xs()
                                    .text_color(rgb(0x237844))
                                    .child("免配置"),
                            ),
                    )
                    .child(div().h(px(1.)).bg(rgb(0xebebe6)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2p5()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .min_w_0()
                                            .child(div().font_medium().child("百度翻译"))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x787774))
                                                    .child("通用文本翻译 API（fanyi-api.baidu.com），需配置 APP ID 与密钥"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .px_2()
                                            .py(px(2.))
                                            .rounded(px(4.))
                                            .when(baidu_ready, |b| {
                                                b.bg(rgb(0xeef3f8))
                                                    .border_1()
                                                    .border_color(rgb(0xcbd9e6))
                                                    .text_color(rgb(0x3a5a78))
                                            })
                                            .when(!baidu_ready, |b| {
                                                b.bg(rgb(0xfff4e5))
                                                    .border_1()
                                                    .border_color(rgb(0xf2d3a6))
                                                    .text_color(rgb(0x9a5b13))
                                            })
                                            .text_xs()
                                            .child(if baidu_ready { "已配置" } else { "需配置密钥" }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(96.))
                                                    .flex_shrink_0()
                                                    .text_xs()
                                                    .text_color(rgb(0x5b5b58))
                                                    .child("APP ID"),
                                            )
                                            .child(div().flex_1().min_w_0().child(Input::new(&self.baidu_app_id).mask_toggle()))
                                            .child(copy_btn(
                                                "copy-baidu-app-id",
                                                self.settings.translate.baidu_app_id.clone(),
                                                self.copied_api_field == Some("copy-baidu-app-id"),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(96.))
                                                    .flex_shrink_0()
                                                    .text_xs()
                                                    .text_color(rgb(0x5b5b58))
                                                    .child("API Key（密钥）"),
                                            )
                                            .child(div().flex_1().min_w_0().child(Input::new(&self.baidu_api_key).mask_toggle()))
                                            .child(copy_btn(
                                                "copy-baidu-api-key",
                                                self.settings.translate.baidu_api_key.clone(),
                                                self.copied_api_field == Some("copy-baidu-api-key"),
                                            )),
                                    ),
                            ),
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

fn choice_button(id: impl Into<ElementId>, label: &'static str, selected: bool) -> Stateful<Div> {
    if selected {
        crate::ui::button_primary(id, label)
    } else {
        crate::ui::button_secondary(id, label).px_3()
    }
}
impl Render for Preferences {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font_size = px(self.settings.interface_font_size as f32);
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
                .px_2()
                .py_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(ui_font_size)
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
            PreferencesTab::Fonts => self.render_fonts(cx),
            PreferencesTab::Theme => div().flex().flex_col().gap_5()
                .child(div().text_lg().font_semibold().child("外观 / 主题"))
                .child(self.render_theme(cx))
                .into_any_element(),
            PreferencesTab::View => self.render_view(cx),
            PreferencesTab::Shortcuts => self.render_shortcuts(cx),
            PreferencesTab::Terminal => self.render_terminal(),
            PreferencesTab::Api => self.render_api(cx),
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
            .font_family(crate::ui::sans_font(&self.settings.interface_font))
            .text_size(ui_font_size)
            .text_color(rgb(0x37352f))
            .child(
                div()
                    .w(px(152.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pr_4()
                    .border_r_1()
                    .border_color(rgb(0xe8e7e3))
                    .child(
                        div()
                            .px_2()
                            .pb_2()
                            .text_size(ui_font_size)
                            .font_semibold()
                            .child("设置"),
                    )
                    .child(
                        div().id("pref-appearance-group")
                            .flex().items_center().gap_2().px_2().py_2()
                            .rounded(px(6.)).cursor_pointer().text_size(ui_font_size)
                            .hover(|s| s.bg(rgb(0xefefed)))
                            .child(crate::ui::icon(if self.appearance_expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            }))
                            .child("外观")
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.appearance_expanded = !panel.appearance_expanded;
                                cx.notify();
                            })),
                    )
                    .when(self.appearance_expanded, |nav| nav.child(
                            div().ml_2().pl_2().border_l_1().border_color(rgb(0xe8e7e3))
                            .flex().flex_col().gap_1()
                            .child(nav_item("pref-tab-fonts", PreferencesTab::Fonts,
                                IconName::Type, "字体", self.tab == PreferencesTab::Fonts, cx))
                            .child(nav_item("pref-tab-theme", PreferencesTab::Theme,
                                IconName::Palette, "主题", self.tab == PreferencesTab::Theme, cx))
                            .child(nav_item("pref-tab-view", PreferencesTab::View,
                                IconName::Columns2, "视图", self.tab == PreferencesTab::View, cx))
                    ))
                    .child(nav_item(
                        "pref-tab-shortcuts",
                        PreferencesTab::Shortcuts,
                        IconName::Keyboard,
                        "快捷键",
                        self.tab == PreferencesTab::Shortcuts,
                        cx,
                    ))
                    .child(nav_item(
                        "pref-tab-terminal",
                        PreferencesTab::Terminal,
                        IconName::CodeXml,
                        "终端",
                        self.tab == PreferencesTab::Terminal,
                        cx,
                    ))
                    .child(nav_item(
                        "pref-tab-api",
                        PreferencesTab::Api,
                        IconName::Globe,
                        "API 服务",
                        self.tab == PreferencesTab::Api,
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
