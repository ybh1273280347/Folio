//! Applies settings from the window's authoritative copy to their consumers.

use super::*;

/// Holds the backdrop currently being drawn and the latest requested physical size.
#[derive(Default)]
pub(super) struct Backdrop {
    image: Option<Arc<RenderImage>>,
    target: Option<(u32, u32)>,
    load: Option<Task<()>>,
}

impl Workspace {
    pub(super) fn render_backdrop(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let opacity = self.settings.background_image_opacity;
        if opacity == 0 || self.settings.background_image.is_none() {
            return None;
        }
        let workspace = cx.weak_entity();
        Some(
            div()
                .absolute()
                .inset_0()
                .size_full()
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let scale = window.scale_factor();
                            let width = (f32::from(bounds.size.width) * scale).ceil() as u32;
                            let height = (f32::from(bounds.size.height) * scale).ceil() as u32;
                            let target = workspace.read_with(cx, |app, _| app.backdrop.target);
                            if width > 0
                                && height > 0
                                && target.is_ok_and(|target| target != Some((width, height)))
                            {
                                window.defer(cx, move |window, cx| {
                                    let _ = workspace.update(cx, |app, cx| {
                                        app.load_background_image((width, height), window, cx);
                                    });
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
                .children(self.backdrop.image.iter().map(|image| {
                    img(image.clone())
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(f32::from(opacity) / 100.)
                })),
        )
    }

    fn load_background_image(
        &mut self,
        target: (u32, u32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.background_image_opacity == 0 || self.backdrop.target == Some(target) {
            return;
        }
        let Some(path) = self.settings.background_image.clone() else {
            return;
        };
        // Replacing the task cancels the old debounce and prevents stale decode results from painting.
        // Keep the rendered texture while layout is changing, including during sidebar transitions.
        let resizing = self.backdrop.target.replace(target).is_some();
        let decode = ImageAssetLoader::load_cover(path, target, cx);
        self.backdrop.load = Some(cx.spawn_in(window, async move |this, cx| {
            if resizing {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
            let result = cx.background_executor().spawn(decode).await;
            let _ = this.update_in(cx, |app, window, cx| {
                let error = match result {
                    Ok(image) => {
                        app.set_background_image(Some(image), window, cx);
                        None
                    }
                    Err(error) => Some(crate::i18n::workspace::err_read_background_image(&error)),
                };
                app.preferences
                    .update(cx, |panel, cx| panel.set_background_image_error(error, cx));
            });
        }));
    }

    pub(super) fn has_visible_background(&self) -> bool {
        self.backdrop.image.is_some() && self.settings.background_image_opacity > 0
    }

    fn sync_backdrop_visible(&self, cx: &mut Context<Self>) {
        self.documents.set_backdrop_visible(self.has_visible_background(), cx);
    }

    fn set_background_image(
        &mut self,
        image: Option<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = std::mem::replace(&mut self.backdrop.image, image) {
            cx.drop_image(previous, Some(window));
        }
        // Document surfaces follow the decoded image only when its opacity is non-zero.
        self.sync_backdrop_visible(cx);
        cx.notify();
    }

    pub(super) fn persist_settings(&mut self, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        if let Some(snapshot) = &self.zen_snapshot {
            settings.sidebar_open = snapshot.sidebar_open;
        }
        let error = storage::save_settings(&settings).err().map(|error| error.to_string());
        if let Some(error) = &error {
            self.report(error, cx);
        }
        self.preferences.update(cx, |panel, cx| panel.set_save_error(error, cx));
    }

    pub(super) fn apply_settings(&mut self, next: AppSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings == next {
            return;
        }
        let previous = std::mem::replace(&mut self.settings, next);
        let settings = &self.settings;
        let palette_changed = previous.theme.palette != settings.theme.palette;
        let fallback_changed = previous.cjk_fallback_font != settings.cjk_fallback_font;
        let cjk_scale_changed = (previous.cjk_font_scale - settings.cjk_font_scale).abs() > f32::EPSILON;
        let locale_changed = previous.locale != settings.locale;

        if palette_changed || fallback_changed || cjk_scale_changed
            || previous.interface_font != settings.interface_font
            || previous.interface_font_size != settings.interface_font_size
            || previous.smooth_caret != settings.smooth_caret
            || crate::theme::tool_typography(&previous) != crate::theme::tool_typography(settings)
        {
            crate::theme::apply_settings(settings, cx);
        }
        if previous.terminal != settings.terminal || palette_changed {
            self.terminal.update(cx, |pane, cx| pane.sync(&settings.terminal, settings.theme.palette, cx));
        }
        if previous.ai != settings.ai || previous.ai_font != settings.ai_font
            || previous.ai_font_size != settings.ai_font_size || previous.tool_execution_view != settings.tool_execution_view
            || previous.inline_code_font != settings.inline_code_font || previous.block_code_font != settings.block_code_font
            || previous.inline_code_scale != settings.inline_code_scale || previous.block_code_scale != settings.block_code_scale
        {
            self.ai.update(cx, |pane, cx| pane.sync_settings(settings, cx));
        }
        self.documents.apply_editor_appearance(&previous, settings, window, cx);
        if previous.math_font != settings.math_font || fallback_changed
            || previous.math_scale != settings.math_scale || previous.inline_math_scale != settings.inline_math_scale
            || previous.markdown_font_size != settings.markdown_font_size || previous.ai_font_size != settings.ai_font_size
        {
            self.math.update(cx, |renderer, cx| renderer.set_appearance(settings, cx));
        }
        if previous.translate != settings.translate {
            cx.set_global(settings.translate.clone());
        }
        if previous.keybindings != settings.keybindings {
            // Unbind in the same context in which the action was registered.
            for (id, action, context) in [
                ("toggle_ai", ToggleAi.name(), "FolioWorkspace"),
                ("toggle_terminal", ToggleTerminal.name(), "FolioWorkspace"),
                ("toggle_zen", ToggleZen.name(), "FolioWorkspace"),
                ("ask_ai", AskAi.name(), "!GpuiXterm"),
            ] {
                let old_key = previous.key_for(id);
                if !old_key.is_empty() && old_key != settings.key_for(id) {
                    cx.bind_keys([KeyBinding::new(
                        Box::leak(old_key.into_boxed_str()), Unbind(action.into()), Some(context),
                    )]);
                }
            }
            bind_shortcuts(settings, cx);
        }
        if locale_changed {
            crate::i18n::apply_settings(settings);
            self.right_panel.refresh_locale(window, cx);
            self.project_menu.refresh_locale(window, cx);
            self.tags.refresh_locale(window, cx);
            self.preferences.update(cx, |pane, cx| pane.refresh_locale(window, cx));
            self.ai.update(cx, |pane, cx| pane.refresh_locale(window, cx));
            self.documents.refresh_locale(window, cx);
            self.build.refresh_locale(window, cx);
        }
        if locale_changed || fallback_changed || cjk_scale_changed {
            window.refresh();
        }
        if previous.background_image != settings.background_image
            || (previous.background_image_opacity > 0 && settings.background_image_opacity == 0)
        {
            self.backdrop.load = None;
            self.backdrop.target = None;
            self.set_background_image(None, window, cx);
            self.preferences
                .update(cx, |panel, cx| panel.set_background_image_error(None, cx));
        }
        if previous.rag != self.settings.rag {
            self.refresh_kb_snapshot(cx);
        }
        if previous.extra_text_extensions != self.settings.extra_text_extensions {
            crate::files::set_extra_text_extensions(&self.settings.extra_text_extensions);
            self.schedule_document_tree_refresh();
        }
        self.persist_settings(cx);
        cx.notify();
    }
}
