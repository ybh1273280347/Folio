//! Applies settings from the window's authoritative copy to their consumers.

use super::*;

impl Workspace {
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

        if palette_changed || fallback_changed || cjk_scale_changed || previous.interface_font != settings.interface_font
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
        if previous.show_line_numbers != settings.show_line_numbers {
            for document in self.documents.open.values() {
                document.editor.update(cx, |editor, cx| editor.set_line_number(settings.show_line_numbers, window, cx));
            }
        }
        if previous.show_line_numbers != settings.show_line_numbers || previous.line_width_limited != settings.line_width_limited
            || previous.editor_font != settings.editor_font || previous.editor_font_size != settings.editor_font_size
            || previous.markdown_font != settings.markdown_font || previous.markdown_font_size != settings.markdown_font_size
            || previous.interface_font != settings.interface_font
            || previous.inline_code_font != settings.inline_code_font || previous.block_code_font != settings.block_code_font
            || previous.inline_code_scale != settings.inline_code_scale || previous.block_code_scale != settings.block_code_scale
        {
            for pane in self.documents.markdown_panes.values() {
                pane.update(cx, |pane, cx| pane.apply_settings(settings, window, cx));
            }
        }
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
            self.right_panel_search.update(cx, |input, cx| {
                input.set_placeholder(crate::i18n::workspace::SEARCH_HUB_PLACEHOLDER.get(), window, cx);
            });
            self.tags.refresh_locale(window, cx);
            self.preferences.update(cx, |pane, cx| pane.refresh_locale(window, cx));
            self.ai.update(cx, |pane, cx| pane.refresh_locale(window, cx));
            for pane in self.documents.markdown_panes.values() {
                pane.update(cx, |pane, cx| pane.refresh_locale(window, cx));
            }
            for pane in self.documents.pdf_panes.values().chain(self.build_pdf.iter()) {
                pane.update(cx, |pane, cx| pane.refresh_locale(window, cx));
            }
        }
        if locale_changed || fallback_changed || cjk_scale_changed {
            window.refresh();
        }
        if previous.background_image != settings.background_image {
            self.load_background_image(window, cx);
        } else if previous.background_image_opacity != settings.background_image_opacity {
            self.sync_backdrop_visible(cx);
        }
        self.persist_settings(cx);
        cx.notify();
    }
}
