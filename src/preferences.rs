//! Appearance and shortcut preferences are persisted independently of project files and PDF typesetting.
use crate::i18n::{Locale, common as common_i18n, preferences as p};
use crate::theme::color as rgb;
use crate::{
    model::{AppSettings, InternalLinkOpenTarget, MathFont, SHORTCUT_DEFS, ToolExecutionView},
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

/// A preference edit is relative to the panel's last published values. Layout
/// and editor shortcuts can update the authoritative copy independently.
pub struct SettingsChanged {
    previous: AppSettings,
    next: AppSettings,
}

impl SettingsChanged {
    pub(crate) fn apply_to(&self, current: &AppSettings) -> AppSettings {
        let mut merged = current.clone();
        macro_rules! merge {
            ($($field:ident),+ $(,)?) => {
                $(if self.previous.$field != self.next.$field {
                    merged.$field.clone_from(&self.next.$field);
                })+
            };
        }
        merge!(
            locale, theme, background_image, background_image_opacity,
            interface_font, editor_font, markdown_font, ai_font, cjk_fallback_font, cjk_font_scale,
            inline_code_font, block_code_font, inline_code_scale, block_code_scale,
            interface_font_size, editor_font_size, markdown_font_size, ai_font_size,
            math_scale, inline_math_scale, math_font, line_width_limited,
            internal_link_open_target, tool_execution_view, show_line_numbers, smooth_caret,
            terminal, translate, ai, rag, keybindings,
        );
        merged
    }
}

pub struct Preferences {
    settings: AppSettings,
    published_settings: AppSettings,
    tab: PreferencesTab,
    appearance_expanded: bool,
    recording: Option<&'static str>,
    focus: FocusHandle,
    system_fonts: Vec<String>,
    interface_font: Entity<SelectState<SearchableVec<String>>>,
    editor_font: Entity<SelectState<SearchableVec<String>>>,
    markdown_font: Entity<SelectState<SearchableVec<String>>>,
    ai_font: Entity<SelectState<SearchableVec<String>>>,
    cjk_fallback_font: Entity<SelectState<SearchableVec<String>>>,
    cjk_font_scale: Entity<SliderState>,
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
    rag_ocr_token: Entity<InputState>,
    rag_server_bin: Entity<InputState>,
    rag_embed_url: Entity<InputState>,
    rag_embed_model_path: Entity<InputState>,
    rag_rerank_url: Entity<InputState>,
    rag_rerank_model_path: Entity<InputState>,
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
            let mut input = InputState::new(window, cx).placeholder(p::THEME_NAME_PLACEHOLDER.get());
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
        let terminal_shell = cx.new(|cx| InputState::new(window, cx).placeholder(p::TERMINAL_SHELL_PLACEHOLDER.get()));
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
        let system_fonts = fonts.clone();
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
        fonts.insert(0, p::SYSTEM_DEFAULT.get().into());
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
        let cjk_font_scale = cx.new(|_| {
            SliderState::new()
                .min(0.8)
                .max(1.4)
                .step(0.02)
                .default_value(settings.cjk_font_scale)
        });
        cx.subscribe(&cjk_font_scale, |panel, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                panel.settings.cjk_font_scale = (value.end() * 50.).round() / 50.;
                panel.changed(cx);
            }
        })
        .detach();
        fonts[0] = p::DEFAULT_MONOSPACE.get().into();
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
                        p::DEFAULT_MONOSPACE.get().into()
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
                        *selected = if font == p::DEFAULT_MONOSPACE.zh || font == p::DEFAULT_MONOSPACE.en {
                            String::new()
                        } else {
                            font.clone()
                        };
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
                        p::SYSTEM_DEFAULT.get().into()
                    } else {
                        selected.clone()
                    },
                    window,
                    cx,
                )
            });
            cx.subscribe(state, move |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                if let SelectEvent::Confirm(Some(font)) = event {
                    *field(&mut panel.settings) = if font == p::SYSTEM_DEFAULT.zh || font == p::SYSTEM_DEFAULT.en {
                        String::new()
                    } else {
                        font.clone()
                    };
                    panel.changed(cx);
                }
            })
            .detach();
        }
        let baidu_app_id = cx.new(|cx| InputState::new(window, cx).placeholder(p::BAIDU_APP_ID_PLACEHOLDER.get()).masked(true));
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
        let baidu_api_key = cx.new(|cx| InputState::new(window, cx).placeholder(p::BAIDU_API_KEY_PLACEHOLDER.get()).masked(true));
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
        let ai_qianfan_key = cx.new(|cx| InputState::new(window, cx).placeholder(p::QIANFAN_PLACEHOLDER.get()).masked(true));
        ai_qianfan_key.update(cx, |input,cx| input.set_value(settings.ai.baidu_qianfan_api_key.clone(),window,cx));
        cx.subscribe(&ai_qianfan_key, |panel,input,event: &InputEvent,cx| {
            if matches!(event,InputEvent::Change) { panel.settings.ai.baidu_qianfan_api_key=input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let rag_ocr_token = cx.new(|cx| InputState::new(window, cx).placeholder(p::RAG_OCR_PLACEHOLDER.get()).masked(true));
        rag_ocr_token.update(cx, |input, cx| input.set_value(settings.rag.paddleocr_token.clone(), window, cx));
        cx.subscribe(&rag_ocr_token, |panel, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { panel.settings.rag.paddleocr_token = input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let rag_server_bin = cx.new(|cx| InputState::new(window, cx).placeholder("llama-server"));
        rag_server_bin.update(cx, |input, cx| input.set_value(settings.rag.server_bin.clone(), window, cx));
        cx.subscribe(&rag_server_bin, |panel, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { panel.settings.rag.server_bin = input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let rag_embed_url = cx.new(|cx| InputState::new(window, cx).placeholder("http://127.0.0.1:18101"));
        rag_embed_url.update(cx, |input, cx| input.set_value(settings.rag.embed_url.clone(), window, cx));
        cx.subscribe(&rag_embed_url, |panel, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { panel.settings.rag.embed_url = input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let rag_embed_model_path = cx.new(|cx| InputState::new(window, cx).placeholder("Qwen3-Embedding-0.6B-Q8_0.gguf"));
        rag_embed_model_path.update(cx, |input, cx| input.set_value(settings.rag.embed_model_path.clone(), window, cx));
        cx.subscribe(&rag_embed_model_path, |panel, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { panel.settings.rag.embed_model_path = input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let rag_rerank_url = cx.new(|cx| InputState::new(window, cx).placeholder("http://127.0.0.1:18102"));
        rag_rerank_url.update(cx, |input, cx| input.set_value(settings.rag.rerank_url.clone(), window, cx));
        cx.subscribe(&rag_rerank_url, |panel, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { panel.settings.rag.rerank_url = input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        let rag_rerank_model_path = cx.new(|cx| InputState::new(window, cx).placeholder("Qwen3-Reranker-0.6B-Q8_0.gguf"));
        rag_rerank_model_path.update(cx, |input, cx| input.set_value(settings.rag.rerank_model_path.clone(), window, cx));
        cx.subscribe(&rag_rerank_model_path, |panel, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { panel.settings.rag.rerank_model_path = input.read(cx).value().trim().to_owned(); panel.changed(cx); }
        }).detach();
        Self {
            published_settings: settings.clone(),
            settings,
            tab: PreferencesTab::Fonts,
            appearance_expanded: true,
            recording: None,
            focus: cx.focus_handle(),
            system_fonts,
            interface_font,
            editor_font,
            markdown_font,
            ai_font,
            cjk_fallback_font,
            cjk_font_scale,
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
            rag_ocr_token,
            rag_server_bin,
            rag_embed_url,
            rag_embed_model_path,
            rag_rerank_url,
            rag_rerank_model_path,
            copied_api_field: None,
            copied_api_task: Task::ready(()),
            editing_theme: false,
            error: None,
        }
    }
    pub fn select_api_tab(&mut self, cx: &mut Context<Self>) {
        self.tab = PreferencesTab::Api;
        cx.notify();
    }

    pub fn sync(&mut self, settings: &AppSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.published_settings.clone_from(settings);
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
                    &if selected.is_empty() { p::SYSTEM_DEFAULT.get().into() } else { selected.clone() },
                    window,
                    cx,
                );
            });
        }
        self.cjk_font_scale.update(cx, |state, cx| {
            state.set_value(settings.cjk_font_scale, window, cx)
        });
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
                    &if selected.is_empty() { p::DEFAULT_MONOSPACE.get().into() } else { selected.clone() },
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
        self.rag_ocr_token.update(cx, |input, cx| input.set_value(settings.rag.paddleocr_token.clone(), window, cx));
        self.rag_server_bin.update(cx, |input, cx| input.set_value(settings.rag.server_bin.clone(), window, cx));
        self.rag_embed_url.update(cx, |input, cx| input.set_value(settings.rag.embed_url.clone(), window, cx));
        self.rag_embed_model_path.update(cx, |input, cx| input.set_value(settings.rag.embed_model_path.clone(), window, cx));
        self.rag_rerank_url.update(cx, |input, cx| input.set_value(settings.rag.rerank_url.clone(), window, cx));
        self.rag_rerank_model_path.update(cx, |input, cx| input.set_value(settings.rag.rerank_model_path.clone(), window, cx));
    }
    pub fn set_background_image_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.background_image_error = error;
        cx.notify();
    }

    pub fn refresh_locale(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_name.update(cx, |input, cx| {
            input.set_placeholder(p::THEME_NAME_PLACEHOLDER.get(), window, cx);
        });
        self.terminal_shell.update(cx, |input, cx| {
            input.set_placeholder(p::TERMINAL_SHELL_PLACEHOLDER.get(), window, cx);
        });
        self.baidu_app_id.update(cx, |input, cx| {
            input.set_placeholder(p::BAIDU_APP_ID_PLACEHOLDER.get(), window, cx);
        });
        self.baidu_api_key.update(cx, |input, cx| {
            input.set_placeholder(p::BAIDU_API_KEY_PLACEHOLDER.get(), window, cx);
        });
        self.ai_qianfan_key.update(cx, |input, cx| {
            input.set_placeholder(p::QIANFAN_PLACEHOLDER.get(), window, cx);
        });
        self.rag_ocr_token.update(cx, |input, cx| {
            input.set_placeholder(p::RAG_OCR_PLACEHOLDER.get(), window, cx);
        });
        let system_default = p::SYSTEM_DEFAULT.get().to_owned();
        let default_items: Vec<String> = std::iter::once(system_default.clone())
            .chain(self.system_fonts.iter().cloned())
            .collect();
        for (selector, selected) in [
            (&self.interface_font, &self.settings.interface_font),
            (&self.editor_font, &self.settings.editor_font),
            (&self.markdown_font, &self.settings.markdown_font),
            (&self.ai_font, &self.settings.ai_font),
            (&self.cjk_fallback_font, &self.settings.cjk_fallback_font),
        ] {
            selector.update(cx, |state, cx| {
                state.set_items(SearchableVec::new(default_items.clone()), window, cx);
                state.set_selected_value(
                    if selected.is_empty() { &system_default } else { selected },
                    window,
                    cx,
                );
            });
        }
        let default_mono = p::DEFAULT_MONOSPACE.get().to_owned();
        let mono_items: Vec<String> = std::iter::once(default_mono.clone())
            .chain(self.system_fonts.iter().cloned())
            .collect();
        for (selector, selected) in [
            (&self.inline_code_font, &self.settings.inline_code_font),
            (&self.block_code_font, &self.settings.block_code_font),
        ] {
            selector.update(cx, |state, cx| {
                state.set_items(SearchableVec::new(mono_items.clone()), window, cx);
                state.set_selected_value(
                    if selected.is_empty() { &default_mono } else { selected },
                    window,
                    cx,
                );
            });
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if self.published_settings != self.settings {
            let next = self.settings.clone();
            let previous = std::mem::replace(&mut self.published_settings, next.clone());
            cx.emit(SettingsChanged { previous, next });
        }
        cx.notify();
    }

    pub(crate) fn set_save_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        if self.error != error {
            self.error = error;
            cx.notify();
        }
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
                .child(label.get()).child(ColorPicker::new(picker))
        });
        let custom_selected = !matches!(self.settings.theme.active.as_str(), "浅色" | "深色")
            && self.settings.theme.saved.contains_key(&self.settings.theme.active);
        div().flex().flex_col().gap_3()
            .child(div().font_semibold().child(p::THEME_SECTION_TITLE.get()))
            .child(div().flex().flex_wrap().gap_2().children(choices))
            .child(div().flex().items_center().justify_between()
                .child(p::current_theme(&self.settings.theme.active))
                .child(crate::ui::button_secondary("edit-theme", if self.editing_theme { p::COLLAPSE_COLORS.get() } else { p::EDIT_COLORS.get() })
                    .on_click(cx.listener(|panel, _, _, cx| { panel.editing_theme = !panel.editing_theme; cx.notify(); }))))
            .when(self.editing_theme, |section| section
            .child(div().flex().flex_col().gap_1().children(colors))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(Input::new(&self.theme_name)))
                .child(crate::ui::button_secondary("save-theme", p::SAVE_THEME.get())
                    .on_click(cx.listener(|panel, _, _, cx| {
                        let name = panel.theme_name.read(cx).value().to_string();
                        match panel.settings.theme.save(&name) {
                            Ok(()) => panel.changed(cx),
                            Err(error) => { panel.error = Some(error.into()); cx.notify(); }
                        }
                    })))
                .when(custom_selected, |row| row.child(crate::ui::button("delete-theme", p::DELETE_THEME.get())
                    .on_click(cx.listener(|panel, _, window, cx| {
                        panel.settings.theme.saved.remove(&panel.settings.theme.active);
                        panel.select_theme("浅色", window, cx);
                    }))))))
            .child(div().flex().justify_start().child(
                control("reset-theme", p::RESET_THEME.get())
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
                    .child(slider_row(p::FONT_SIZE.get(), format!("{value} px"), size).w(px(112.)).flex_shrink_0()))
        };
        let [interface_size, editor_size, markdown_size, ai_size] = &self.sizes;
        let surfaces = section(p::SECTION_SURFACE_FONTS.get())
            .child(font_row(p::FONT_INTERFACE.get(), &self.interface_font, interface_size, settings.interface_font_size))
            .child(font_row(p::FONT_MARKDOWN.get(), &self.markdown_font, markdown_size, settings.markdown_font_size))
            .child(font_row(p::FONT_AI.get(), &self.ai_font, ai_size, settings.ai_font_size))
            .child(font_row(p::FONT_TERMINAL.get(), &self.terminal_font_family, &self.terminal_font_size, settings.terminal.font_size))
            .child(font_row(p::FONT_EDITOR.get(), &self.editor_font, editor_size, settings.editor_font_size))
            .child(div().p_3().rounded(px(5.)).bg(rgb(palette.background))
                .font_family(if settings.editor_font.is_empty() {
                    SharedString::from("Consolas")
                } else {
                    settings.editor_font.clone().into()
                })
                .text_size(px(settings.editor_font_size as f32))
                .child(p::EDITOR_FONT_SAMPLE.get()));

        let markdown_size = px(settings.markdown_font_size as f32);
        let preview_style = gpui_kit::base::text::TextViewStyle::from_theme(&gpui_kit::base::Theme::global(cx))
            .with_inline_code_style(crate::ui::inline_code_style(settings, markdown_size))
            .with_code_block(crate::ui::block_code_style(settings, markdown_size));
        let shared = section(p::SECTION_SHARED_TYPOGRAPHY.get())
            .child(div().text_sm().text_color(rgb(palette.muted))
                .child(p::SHARED_TYPOGRAPHY_HINT.get()))
            .child(div().flex().gap_4()
                .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                    .child(p::INLINE_CODE_FONT.get())
                    .child(Select::new(&self.inline_code_font).w_full())
                    .child(slider_row(p::FONT_SCALE.get(), format!("{:.1}%", settings.inline_code_scale * 100.), &self.inline_code_scale)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                    .child(p::BLOCK_CODE_FONT.get())
                    .child(Select::new(&self.block_code_font).w_full())
                    .child(slider_row(p::FONT_SCALE.get(), format!("{:.1}%", settings.block_code_scale * 100.), &self.block_code_scale))))
            .child(div().p_3().rounded(px(5.)).bg(rgb(palette.background))
                .font_family(crate::ui::sans_font(&settings.markdown_font))
                .text_size(markdown_size)
                .child(gpui_kit::base::text::markdown(p::MARKDOWN_FONT_SAMPLE.get())
                    .style(preview_style)))
            .child(div().flex().flex_col().gap_2()
                .child(p::MATH_FONT_LABEL.get())
                .child(Select::new(&self.math_font).w_full()))
            .child(div().flex().gap_4()
                .child(slider_row(p::INLINE_MATH_SCALE.get(), format!("{:.1}×", settings.inline_math_scale), &self.inline_math_scale)
                    .flex_1().min_w_0())
                .child(slider_row(p::BLOCK_MATH_SCALE.get(), format!("{:.1}×", settings.math_scale), &self.math_scale)
                    .flex_1().min_w_0()));

        let fallback = section(p::SECTION_FONT_FALLBACK.get())
            .child(div().flex().flex_col().gap_2()
                .child(Select::new(&self.cjk_fallback_font).w_full())
                .child(div().text_sm().text_color(rgb(palette.muted))
                    .child(p::FONT_FALLBACK_HINT.get())))
            .child(div().flex().flex_col().gap_2()
                .child(slider_row(
                    p::CJK_FONT_SCALE.get(),
                    format!("{:.0}%", settings.cjk_font_scale * 100.),
                    &self.cjk_font_scale,
                ))
                .child(div().text_sm().text_color(rgb(palette.muted))
                    .child(p::CJK_FONT_SCALE_HINT.get()))
                .child(div().p_3().rounded(px(5.)).bg(rgb(palette.background))
                    .child(p::FALLBACK_FONT_SAMPLE.get())));
        div().flex().flex_col().gap_5()
            .child(div().text_lg().font_semibold().child(p::PAGE_FONTS_TITLE.get()))
            .child(surfaces)
            .child(shared)
            .child(fallback)
            .child(
                div().flex().justify_start().child(
                    control("reset-fonts", p::RESET_FONTS.get()).on_click(cx.listener(
                        |panel, _, window, cx| {
                            // Each appearance page resets only its own settings.
                            let defaults = AppSettings::default();
                            panel.settings.interface_font = defaults.interface_font;
                            panel.settings.editor_font = defaults.editor_font;
                            panel.settings.markdown_font = defaults.markdown_font;
                            panel.settings.ai_font = defaults.ai_font;
                            panel.settings.cjk_fallback_font = defaults.cjk_fallback_font;
                            panel.settings.cjk_font_scale = defaults.cjk_font_scale;
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
                            panel.changed(cx);
                            let settings = panel.settings.clone();
                            panel.sync(&settings, window, cx);
                        },
                    )),
                ),
            )
            .into_any_element()
    }

    fn render_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let tool_views = [
            (ToolExecutionView::Minimal, p::TOOL_VIEW_MINIMAL.get()),
            (ToolExecutionView::Standard, p::TOOL_VIEW_STANDARD.get()),
            (ToolExecutionView::Detailed, p::TOOL_VIEW_DETAILED.get()),
        ];
        let locales = Locale::ALL;
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_semibold().child(p::PAGE_VIEW_TITLE.get()))
            .child(
                div().flex().items_center().justify_between().gap_3()
                    .child(p::LANGUAGE_LABEL.get())
                    .child(
                        RadioGroup::horizontal("interface-locale")
                            .flex_none()
                            .children(locales.iter().map(|loc| Radio::new(loc.label()).label(loc.label())))
                            .selected_index(locales.iter().position(|&loc| loc == self.settings.locale))
                            .on_change(cx.listener(move |panel, index: &usize, _, cx| {
                                let next = locales[*index];
                                if panel.settings.locale != next {
                                    panel.settings.locale = next;
                                    panel.changed(cx);
                                }
                            })),
                    ),
            )
            .child(
                div().flex().flex_col().gap_2()
                    .child(
                        div().flex().items_center().justify_between().gap_3()
                            .child(p::BACKGROUND_IMAGE.get())
                            .child(
                                div().flex().gap_2()
                                    .child(control("choose-background-image", p::CHOOSE_IMAGE.get()).on_click(
                                        cx.listener(|_, _, window, cx| {
                                            let dialog = rfd::AsyncFileDialog::new()
                                                .set_parent(window)
                                                .set_title(p::CHOOSE_BACKGROUND_TITLE.get())
                                                .add_filter(p::IMAGE_FILTER.get(), &["png", "jpg", "jpeg", "webp", "bmp"]);
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
                                        actions.child(control("remove-background-image", p::REMOVE_IMAGE.get()).on_click(
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
                                .child(p::IMAGE_OPACITY.get())
                                .child(format!("{}%", self.settings.background_image_opacity)))
                            .child(Slider::new(&self.background_image_opacity))
                    })
                    .when_some(self.background_image_error.as_ref(), |section, error| {
                        section.child(div().text_sm().text_color(rgb(0xb92d2d)).child(error.clone()))
                    }),
            )
            .child(
                div().flex().items_center().justify_between().gap_3()
                    .child(p::TOOL_EXECUTION_VIEW.get())
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
                    .child(p::SHOW_LINE_NUMBERS.get())
                    .child(
                        control("show-line-numbers", if self.settings.show_line_numbers { common_i18n::ENABLED.get() } else { common_i18n::DISABLED.get() })
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.settings.show_line_numbers = !panel.settings.show_line_numbers;
                                panel.changed(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(p::SMOOTH_CARET.get())
                    .child(
                        control("smooth-caret", if self.settings.smooth_caret { common_i18n::ENABLED.get() } else { common_i18n::DISABLED.get() })
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.settings.smooth_caret = !panel.settings.smooth_caret;
                                panel.changed(cx);
                            })),
                    ),
            )
            .child(
                div().flex().items_center().justify_between()
                    .child(p::LIMIT_LINE_WIDTH.get())
                    .child(
                        control(
                            "limit-width",
                            if self.settings.line_width_limited {
                                common_i18n::ENABLED.get()
                            } else {
                                common_i18n::DISABLED.get()
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
                            .child(div().flex_1().min_w_0().child(p::INTERNAL_LINK_TARGET.get()))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        choice_button(
                                            "link-target-main",
                                            p::TARGET_MAIN_EDITOR.get(),
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
                                            p::TARGET_SPLIT_PANE.get(),
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
                            .child(p::INTERNAL_LINK_HINT.get()),
                    ),
            )
            .into_any_element()
    }

    fn render_terminal(&self) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_semibold().child(p::PAGE_TERMINAL_TITLE.get()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(p::TERMINAL_SHELL.get())
                    .child(Input::new(&self.terminal_shell))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x787774))
                            .child(p::TERMINAL_SHELL_HINT.get()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(p::TERMINAL_ARGS.get())
                    .child(Textarea::new(&self.terminal_args).h(px(112.)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x787774))
                            .child(p::TERMINAL_ARGS_HINT.get()),
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
                            .child(p::TERMINAL_PANEL_WIDTH.get())
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
        let ocr_ready = !crate::rag::ocr::resolve_ocr_token(&self.settings.rag).is_empty();

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
                .child(if copied { common_i18n::COPIED.get() } else { common_i18n::COPY.get() })
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.copy_api_value(id, value.clone(), cx);
                }))
        };

        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_semibold().child(p::PAGE_API_TITLE.get()))
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
                                    .child(div().font_semibold().child(p::AI_SERVICE_TITLE.get()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x787774))
                                    .child(p::AI_SERVICE_DESC.get()),
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
                                    .child(if ai_ready { p::CONFIGURED.get() } else { p::ENTER_ADDRESS.get() }),
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
                                crate::ui::button_secondary("reset-ai-base-url", p::DEFAULT_BTN.get())
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
                        .child(div().w(px(96.)).text_xs().child(p::TITLE_MODEL_ID.get()))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.ai_title_model))))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(96.)).text_xs().child("Exa API Key"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.ai_exa_key).mask_toggle())))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(96.)).text_xs().child(p::QIANFAN_API_KEY.get()))
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
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .min_w_0()
                                    .child(div().font_semibold().child(p::RAG_SERVICE_TITLE.get()))
                                    .child(div().text_xs().text_color(rgb(0x787774)).child(p::RAG_SERVICE_DESC.get())),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .when(ocr_ready, |b| {
                                        b.bg(rgb(0xeaf5ee))
                                            .border_1()
                                            .border_color(rgb(0xc6e5d0))
                                            .text_color(rgb(0x237844))
                                    })
                                    .when(!ocr_ready, |b| {
                                        b.bg(rgb(0xfff4e5))
                                            .border_1()
                                            .border_color(rgb(0xf2d3a6))
                                            .text_color(rgb(0x9a5b13))
                                    })
                                    .text_xs()
                                    .child(if ocr_ready { p::CONFIGURED.get() } else { p::KEY_REQUIRED.get() }),
                            ),
                    )
                    .child(div().h(px(1.)).bg(rgb(0xebebe6)))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(108.)).flex_shrink_0().text_xs().text_color(rgb(0x5b5b58)).child("PaddleOCR Token"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.rag_ocr_token).mask_toggle())))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(108.)).flex_shrink_0().text_xs().text_color(rgb(0x5b5b58)).child("llama-server"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.rag_server_bin))))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(108.)).flex_shrink_0().text_xs().text_color(rgb(0x5b5b58)).child("Embedding URL"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.rag_embed_url)))
                        .child(
                            crate::ui::button_secondary("reset-rag-embed-url", p::DEFAULT_BTN.get())
                                .px_2()
                                .on_click(cx.listener(|panel, _, window, cx| {
                                    let default_url = crate::model::RagConfig::default().embed_url;
                                    panel.settings.rag.embed_url = default_url.clone();
                                    panel.rag_embed_url.update(cx, |input, cx| input.set_value(default_url, window, cx));
                                    panel.changed(cx);
                                })),
                        ))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(108.)).flex_shrink_0().text_xs().text_color(rgb(0x5b5b58)).child("Embedding GGUF"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.rag_embed_model_path))))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(108.)).flex_shrink_0().text_xs().text_color(rgb(0x5b5b58)).child("Rerank URL"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.rag_rerank_url)))
                        .child(
                            crate::ui::button_secondary("reset-rag-rerank-url", p::DEFAULT_BTN.get())
                                .px_2()
                                .on_click(cx.listener(|panel, _, window, cx| {
                                    let default_url = crate::model::RagConfig::default().rerank_url;
                                    panel.settings.rag.rerank_url = default_url.clone();
                                    panel.rag_rerank_url.update(cx, |input, cx| input.set_value(default_url, window, cx));
                                    panel.changed(cx);
                                })),
                        ))
                    .child(div().flex().items_center().gap_2()
                        .child(div().w(px(108.)).flex_shrink_0().text_xs().text_color(rgb(0x5b5b58)).child("Rerank GGUF"))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.rag_rerank_model_path))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(div().text_xs().text_color(rgb(0x5b5b58)).child(p::RAG_AUTO_MANAGE.get()))
                            .child(
                                crate::ui::button_secondary(
                                    "toggle-rag-auto-manage",
                                    if self.settings.rag.auto_manage_server {
                                        common_i18n::ENABLED.get()
                                    } else {
                                        common_i18n::DISABLED.get()
                                    },
                                )
                                .px_2()
                                .on_click(cx.listener(|panel, _, _, cx| {
                                    panel.settings.rag.auto_manage_server = !panel.settings.rag.auto_manage_server;
                                    panel.changed(cx);
                                })),
                            ),
                    ),
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
                            .child(div().font_semibold().child(p::TRANSLATE_SERVICE_TITLE.get()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x787774))
                                    .child(p::TRANSLATE_SERVICE_DESC.get()),
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
                                    .child(div().font_medium().child(p::GOOGLE_TRANSLATE_TITLE.get()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x787774))
                                            .child(p::GOOGLE_TRANSLATE_DESC.get()),
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
                                    .child(p::ZERO_CONFIG.get()),
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
                                            .child(div().font_medium().child(p::BAIDU_TRANSLATE_TITLE.get()))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x787774))
                                                    .child(p::BAIDU_TRANSLATE_DESC.get()),
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
                                            .child(if baidu_ready { p::CONFIGURED.get() } else { p::KEY_REQUIRED.get() }),
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
                                                    .child(p::API_KEY_SECRET_LABEL.get()),
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
        let mut categories: Vec<crate::i18n::BiText> = Vec::new();
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
                        .map(|other| other.label.get());
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
                                .child(div().text_sm().child(def.label.get()))
                                .when_some(conflict, |row, other| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .bg(rgb(0xfef3c7))
                                            .text_color(rgb(0xb45309))
                                            .child(p::shortcut_conflict(other)),
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
                                                .child(p::PRESS_SHORTCUT_PROMPT.get())
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
                        .child(cat.get()),
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
                    .child(div().text_lg().font_semibold().child(p::PAGE_SHORTCUTS_TITLE.get()))
                    .child(
                        control("reset-all-shortcuts", p::RESET_ALL_SHORTCUTS.get()).on_click(cx.listener(
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
                    .child(p::SHORTCUTS_HINT.get()),
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
                .child(div().text_lg().font_semibold().child(p::PAGE_THEME_TITLE.get()))
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
                            .child(p::SETTINGS_TITLE.get()),
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
                            .child(p::NAV_APPEARANCE.get())
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.appearance_expanded = !panel.appearance_expanded;
                                cx.notify();
                            })),
                    )
                    .when(self.appearance_expanded, |nav| nav.child(
                            div().ml_2().pl_2().border_l_1().border_color(rgb(0xe8e7e3))
                            .flex().flex_col().gap_1()
                            .child(nav_item("pref-tab-fonts", PreferencesTab::Fonts,
                                IconName::Type, p::NAV_FONTS.get(), self.tab == PreferencesTab::Fonts, cx))
                            .child(nav_item("pref-tab-theme", PreferencesTab::Theme,
                                IconName::Palette, p::NAV_THEME.get(), self.tab == PreferencesTab::Theme, cx))
                            .child(nav_item("pref-tab-view", PreferencesTab::View,
                                IconName::Columns2, p::NAV_VIEW.get(), self.tab == PreferencesTab::View, cx))
                    ))
                    .child(nav_item(
                        "pref-tab-shortcuts",
                        PreferencesTab::Shortcuts,
                        IconName::Keyboard,
                        p::NAV_SHORTCUTS.get(),
                        self.tab == PreferencesTab::Shortcuts,
                        cx,
                    ))
                    .child(nav_item(
                        "pref-tab-terminal",
                        PreferencesTab::Terminal,
                        IconName::CodeXml,
                        p::NAV_TERMINAL.get(),
                        self.tab == PreferencesTab::Terminal,
                        cx,
                    ))
                    .child(nav_item(
                        "pref-tab-api",
                        PreferencesTab::Api,
                        IconName::Globe,
                        p::NAV_API.get(),
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
