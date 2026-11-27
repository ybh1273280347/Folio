//! Applies settings from the window's authoritative copy to their consumers.

use super::*;

/// Holds the decoded workspace backdrop and the task for the selected image.
#[derive(Default)]
pub(super) struct Backdrop {
    image: Option<Arc<RenderImage>>,
    load: Option<Task<()>>,
}

impl Backdrop {
    pub(super) fn image(&self) -> Option<&Arc<RenderImage>> {
        self.image.as_ref()
    }
}

impl Workspace {
    pub(super) fn load_background_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping the retained task prevents an older selection from publishing after replacement.
        self.backdrop.load = None;
        if self.backdrop.image.is_some() {
            self.set_background_image(None, window, cx);
        }
        self.preferences.update(cx, |panel, cx| panel.set_background_image_error(None, cx));
        let Some(path) = self.settings.background_image.clone() else {
            return;
        };
        let load = ImageAssetLoader::load(path.into(), cx);
        let load = cx.background_executor().spawn(load);
        self.backdrop.load = Some(cx.spawn_in(window, async move |this, cx| {
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
            // Rebind the global panel shortcuts even when focus belongs to a child entity.
            for (id, action) in [
                ("toggle_ai", ToggleAi.name()),
                ("toggle_terminal", ToggleTerminal.name()),
                ("toggle_zen", ToggleZen.name()),
            ] {
                let old_key = previous.key_for(id);
                if old_key != settings.key_for(id) {
                    cx.bind_keys([KeyBinding::new(
                        Box::leak(old_key.into_boxed_str()), Unbind(action.into()), Some("FolioWorkspace"),
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
        if previous.background_image != settings.background_image {
            self.load_background_image(window, cx);
        } else if previous.background_image_opacity != settings.background_image_opacity {
            self.sync_backdrop_visible(cx);
        }
        if previous.rag != self.settings.rag {
            self.refresh_kb_snapshot(cx);
        }
        if previous.extra_text_extensions != self.settings.extra_text_extensions {
            crate::files::set_extra_text_extensions(&self.settings.extra_text_extensions);
            if let Err(error) = self.refresh_files(window, cx) {
                self.report(error, cx);
            }
        }
        self.persist_settings(cx);
        cx.notify();
    }
}
