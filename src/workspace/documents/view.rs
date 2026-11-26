//! Editor and tab views of the document state.

use super::*;

impl Workspace {
    fn render_editor_tab_bar(&self, show_background: bool, cx: &mut Context<Self>) -> AnyElement {
        let fg_alpha = |hex: u32, alpha: f32| -> Rgba {
            let r = ((hex >> 16) & 0xff) as f32 / 255.0;
            let g = ((hex >> 8) & 0xff) as f32 / 255.0;
            let b = (hex & 0xff) as f32 / 255.0;
            Rgba { r, g, b, a: alpha }
        };
        let palette = crate::theme::palette();
        let trash_open = self.trash.is_open();
        let active_tab_bg = if palette.is_dark() {
            gpui_kit::rgb(palette.selection)
        } else {
            gpui_kit::rgb(palette.hover)
        };
        let mut bar = div()
            .h(px(crate::ui::PANE_HEADER_HEIGHT))
            .px_2()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .when(!show_background, |bar| bar.bg(gpui_kit::rgb(palette.surface)))
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border));

        if !trash_open
            && let Some(md_pane) = self.documents.markdown_panes.get(&PaneKey::tab(&self.documents.active))
        {
            let outline_open = md_pane.read(cx).is_outline_open();
            let outline_pane = md_pane.clone();
            bar = bar.child(
                Button::new("md-tab-outline")
                    .ghost()
                    .compact()
                    .with_size(px(16.))
                    .w(px(26.))
                    .h(px(26.))
                    .rounded(px(6.))
                    .when(outline_open, |b| b.bg(gpui_kit::rgb(palette.selection)))
                    .icon(crate::ui::icon(IconName::ListTree).size(px(15.)))
                    .tooltip(if outline_open {
                        crate::i18n::workspace::TOOLTIP_COLLAPSE_OUTLINE.get()
                    } else {
                        crate::i18n::workspace::TOOLTIP_EXPAND_OUTLINE.get()
                    })
                    .on_click(move |_, window, cx| {
                        outline_pane.update(cx, |pane, cx| {
                            pane.toggle_outline(cx);
                            if pane.is_outline_open() && !pane.is_focused(window, cx) {
                                pane.focus(window, cx);
                            }
                        });
                    }),
            );
        }

        let tab_hover_bg = fg_alpha(palette.text, if palette.is_dark() { 0.08 } else { 0.06 });
        let tab_shell = |id: SharedString, title: String, current: bool| {
            div()
                .id(id)
                .w(px(164.))
                .min_w_0()
                .flex_shrink(1.)
                .when(current, |tab| tab.flex_shrink_0().max_w(relative(0.65)))
                .h(px(crate::ui::TAB_HEIGHT))
                .px_2p5()
                .rounded(px(6.))
                .flex()
                .overflow_hidden()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .text_color(gpui_kit::rgb(palette.muted))
                .when(current, |s| {
                    s.font_medium()
                        .text_color(gpui_kit::rgb(palette.text))
                        .when(!show_background, |s| s.bg(active_tab_bg))
                        .when(show_background, |s| s.bg(tab_hover_bg))
                })
                .when(!current, |s| {
                    s.hover(move |h| {
                        h.bg(tab_hover_bg)
                            .text_color(gpui_kit::rgb(palette.text))
                    })
                })
                .when(show_background, |tab| {
                    tab.focusable().tab_stop(true).focus_visible(move |s| s.bg(tab_hover_bg))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(title),
                )
        };

        bar = bar.child(
            div()
                .id("editor-tabs")
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .overflow_hidden()
                .children(self.documents.tabs.iter().map(|path| {
                    let current = !trash_open && self.documents.active == *path;
                    let title = Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    let switch_path = path.clone();
                    let close_path = path.clone();
                    let tooltip_path = path.clone();
                    tab_shell(
                        SharedString::from(format!("editor-tab-{path}")),
                        title,
                        current,
                    )
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tooltip_path.clone())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.activate_tab(&switch_path, window, cx);
                        }))
                        .child(
                            Button::new(SharedString::from(format!("close-editor-tab-{path}")))
                                .ghost()
                                .compact()
                                .with_size(px(16.))
                                .w(px(18.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .icon(crate::ui::icon(IconName::Close).size(px(12.)))
                                .tooltip(crate::i18n::workspace::TOOLTIP_CLOSE_TAB.get())
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_tab(&close_path, window, cx);
                                })),
                        )
                        .into_any_element()
                }))
                .when(trash_open, |strip| {
                    strip.child(
                        tab_shell(
                            SharedString::from("editor-tab-trash"),
                            crate::i18n::workspace::TRASH_LABEL.get().to_owned(),
                            true,
                        )
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                crate::i18n::workspace::TRASH_LABEL.get(),
                            )
                            .build(window, cx)
                        })
                        .child(
                            Button::new("close-editor-tab-trash")
                                .ghost()
                                .compact()
                                .with_size(px(16.))
                                .w(px(18.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .icon(crate::ui::icon(IconName::Close).size(px(12.)))
                                .tooltip(crate::i18n::workspace::TOOLTIP_CLOSE_TAB.get())
                                .on_click(cx.listener(|app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_trash(window, cx);
                                })),
                        ),
                    )
                }),
        );

        if !trash_open && !self.documents.active_canvas_backlinks.is_empty() {
            let backlinks = self.documents.active_canvas_backlinks.clone();
            let count = backlinks.len();
            let app = cx.entity();
            bar = bar.child(
                Button::new("canvas-backlinks-pill")
                    .ghost()
                    .compact()
                    .with_size(px(14.))
                    .h(px(24.))
                    .px_2()
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .icon(crate::ui::icon(IconName::ListTree).size(px(14.)))
                    .label(format!("{count}"))
                    .tooltip(crate::i18n::canvas::TOOLTIP_BACKLINKS.get())
                    .folio_dropdown_menu(move |menu, _, _| {
                        backlinks.iter().fold(
                            menu.min_w(px(196.)).label(
                                crate::i18n::canvas::BACKLINKS_MENU_HEADER.get(),
                            ),
                            |menu, link| {
                                let app = app.clone();
                                let item = link.clone();
                                let canvas_name = Path::new(&link.canvas_path)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| link.canvas_path.clone());
                                let title = if link.label.is_empty() {
                                    canvas_name
                                } else {
                                    format!("{canvas_name} · {}", link.label)
                                };
                                menu.item(
                                    FolioMenuItem::new(title)
                                        .icon(IconName::ListTree)
                                        .on_click(move |_, window, cx| {
                                            app.update(cx, |app, cx| {
                                                app.open_canvas_backlink(&item, window, cx);
                                            });
                                        }),
                                )
                            },
                        )
                    }),
            );
        }

        if !trash_open
            && let Some(md_pane) = self.documents.markdown_panes.get(&PaneKey::tab(&self.documents.active))
        {
            let count = md_pane.read(cx).annotation_count();
            if count > 0 {
                let drawer_pane = md_pane.clone();
                bar = bar.child(
                    div()
                        .id("md-annotations-pill")
                        .px_2p5()
                        .h(px(24.))
                        .rounded_full()
                        .bg(rgb(0xfef9c3))
                        .border_1()
                        .border_color(rgb(0xfde047))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xfef08a)))
                        .active(|s| s.bg(rgb(0xfde047)))
                        .text_size(px(12.))
                        .text_color(rgb(0x854d0e))
                        .child(crate::ui::icon(IconName::MessageSquareText).size(px(14.)))
                        .child(format!("{count}"))
                        .on_click(move |_, _, cx| {
                            drawer_pane.update(cx, |pane, cx| pane.toggle_annotations_drawer(cx));
                        }),
                );
            }
            let copy_pane = md_pane.clone();
            let source_pane = md_pane.clone();
            let is_source = md_pane.read(cx).is_source();
            bar = bar
                .child(crate::ui::copy_document_button(
                    SharedString::from(format!("md-copy-document:{}", self.documents.active)),
                    move |cx| copy_pane.read(cx).source_text(),
                ))
                .child(
                    Button::new("md-tab-source")
                        .ghost()
                        .compact()
                        .with_size(px(16.))
                        .w(px(26.))
                        .h(px(26.))
                        .rounded(px(6.))
                        .when(is_source, |b| b.bg(gpui_kit::rgb(palette.selection)))
                        .icon(crate::ui::icon(IconName::CodeXml).size(px(15.)))
                        .tooltip(if is_source {
                            crate::i18n::workspace::TOOLTIP_SWITCH_WYSIWYG.get()
                        } else {
                            crate::i18n::workspace::TOOLTIP_SWITCH_SOURCE.get()
                        })
                        .on_click(move |_, window, cx| {
                            source_pane.update(cx, |pane, cx| pane.toggle_source(window, cx));
                        }),
                );
        } else if !trash_open
            && Self::is_canvas_path(&self.documents.active)
            && let Some(canvas_pane) = self.documents.canvas_panes.get(&PaneKey::tab(&self.documents.active))
        {
            let copy_pane = canvas_pane.clone();
            bar = bar.child(crate::ui::copy_document_button(
                SharedString::from(format!("canvas-copy-document:{}", self.documents.active)),
                move |cx| copy_pane.read(cx).document().to_json_pretty().unwrap_or_default(),
            ));
        } else if !trash_open
            && let Some(document) = self.documents.open.get(&self.documents.active)
        {
            let editor = document.editor.clone();
            let ask_editor = document.editor.clone();
            let ask_path = document.path.clone();
            let ask_source = document
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| crate::i18n::workspace::SOURCE_LABEL.get().to_owned());
            bar = bar
                .child(
                    Button::new("ask-ai-source")
                        .ghost()
                        .compact()
                        .with_size(px(16.))
                        .w(px(26.))
                        .h(px(26.))
                        .rounded(px(6.))
                        .icon(crate::ui::icon(IconName::Sparkles).size(px(15.)))
                        .tooltip(crate::i18n::workspace::TOOLTIP_SEND_TO_AI.get())
                        .on_click(cx.listener(move |app, _, window, cx| {
                            let selected = ask_editor.read(cx).selected_text().to_string();
                            app.open_ai_with_selection(
                                ask_source.clone(),
                                Some(ask_path.clone()),
                                selected,
                                window,
                                cx,
                            );
                        })),
                )
                .child(crate::ui::copy_document_button(
                    SharedString::from(format!("copy-document:{}", document.path.display())),
                    move |cx| editor.read(cx).value().to_string(),
                ));
        }

        bar.into_any_element()
    }

    pub(in crate::workspace) fn render_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let trash_open = self.trash.is_open();
        let document = self.documents.open.get(&self.documents.active);
        let show_background = !trash_open
            && self.has_visible_background()
            && (self.documents.active.is_empty() || document.is_some());
        let mut area = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .when(!show_background, |area| area.bg(rgb(0xffffff)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.documents.focused_markdown = None),
            );
        if !self.zen_mode && (!self.documents.tabs.is_empty() || trash_open) {
            area = area.child(self.render_editor_tab_bar(show_background, cx));
        }
        if trash_open {
            area = area.child(div().flex_1().min_h_0().child(self.render_trash(window, cx)));
        } else if let Some(document) = document {
            if document.buffer.deleted {
                area = area.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0xfff4e5))
                        .child(crate::i18n::workspace::BANNER_FILE_DELETED.get()),
                );
            } else if document.buffer.conflict.is_some() {
                let reload = self.documents.active.clone();
                let overwrite = self.documents.active.clone();
                area =
                    area.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_2()
                            .bg(rgb(0xfff4e5))
                            .child(crate::i18n::workspace::BANNER_DISK_MODIFIED.get())
                            .child(button("load-disk", crate::i18n::workspace::BTN_LOAD_DISK.get()).on_click(cx.listener(
                                move |app, _, window, cx| app.reload_disk(&reload, window, cx),
                            )))
                            .child(button("overwrite-disk", crate::i18n::workspace::BTN_OVERWRITE_DISK.get()).on_click(
                                cx.listener(move |app, _, _, cx| {
                                    if let Err(error) = app.save_file(&overwrite, true, cx) {
                                        app.report(error, cx);
                                    }
                                }),
                            )),
                    );
            }
            if let Some(markdown) = self.documents.markdown_panes.get(&PaneKey::tab(&self.documents.active)) {
                area = area.child(div().flex_1().min_h_0().child(markdown.clone()));
            } else if let Some(canvas) = self.documents.canvas_panes.get(&PaneKey::tab(&self.documents.active)) {
                area = area.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(canvas.clone()),
                );
            } else {
                area = area.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .p_2()
                        .flex()
                        .justify_center()
                        .child(crate::ui::ctrl_scroll_capture(cx.listener(|app, event: &ScrollEvent, window, cx| {
                            if event.modifiers.control {
                                let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                                    1
                                } else {
                                    -1
                                };
                                let mut settings = app.settings.clone();
                                settings.editor_font_size =
                                    (settings.editor_font_size as i16 + delta).clamp(11, 26) as u8;
                                app.apply_settings(settings, window, cx);
                                app.editor_zoom_hud.show(
                                    app.settings.editor_font_size,
                                    cx,
                                    |app| &mut app.editor_zoom_hud,
                                );
                                cx.stop_propagation();
                                cx.notify();
                            }
                        })))
                        .child(
                            div()
                                .size_full()
                                .when(self.settings.line_width_limited, |editor| {
                                    editor.max_w(px(850.))
                                })
                                .child(
                                    Editor::new(&document.editor)
                                        .appearance(false)
                                        .bordered(false)
                                        .size_full()
                                        .text_size(px(self.settings.editor_font_size as f32))
                                        .font_family(if self.settings.editor_font.is_empty() {
                                            SharedString::from("Consolas")
                                        } else {
                                            self.settings.editor_font.clone().into()
                                        }),
                                ),
                        )
                        .when(document.buffer.char_count > 0, |area| {
                            area.child(crate::ui::char_count_badge(document.buffer.char_count))
                        })
                        .when_some(
                            self.editor_zoom_hud.render("editor-zoom-hud"),
                            |area, hud| area.child(hud),
                        ),
                );
            }
        } else if !self.documents.active.is_empty() {
            let full = self.document_path(&self.documents.active);
            let tab = PaneKey::tab(&self.documents.active);
            if let Some(pane) = self.documents.html_panes.get(&tab) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else if let Some(pane) = self.documents.drawio_panes.get(&tab) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else if let Some(pane) = self.documents.pdf_panes.get(&tab) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else if let Some(pane) = self.documents.image_panes.get(&tab) {
                area = area.child(div().flex_1().min_h_0().child(pane.clone()));
            } else {
                area = area.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_6()
                        .child(img(full).size_full().object_fit(ObjectFit::Contain)),
                );
            }
        } else {
            area = area.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(rgb(MUTED))
                    .child(div().text_xl().child("Folio"))
                    .child(if self.projects.directory().is_none() {
                        crate::i18n::workspace::EMPTY_LOOSE_HINT.get()
                    } else {
                        crate::i18n::workspace::EMPTY_PROJECT_HINT.get()
                    })
                    .when(self.projects.directory().is_none(), |area| area.child(button("welcome-open-file", crate::i18n::workspace::OPEN_FILE.get()).on_click(
                        cx.listener(|app, _, window, cx| app.open_file_dialog(window, cx)),
                    )))
                    .child(if self.projects.directory().is_none() {
                        crate::i18n::workspace::SHORTCUTS_BAR_LOOSE.get()
                    } else {
                        crate::i18n::workspace::SHORTCUTS_BAR_PROJECT.get()
                    }),
            );
        }
        if self.zen_mode
            && let Some(md_pane) = self
                .documents
                .markdown_panes
                .get(&PaneKey::tab(&self.documents.active))
        {
            let md_read = md_pane.read(cx);
            let outline_open = md_read.is_outline_open();
            let docked_w = md_read.outline_docked_width();
            let outline_pane = md_pane.clone();
            let palette = crate::theme::palette();
            area = area.child(
                div()
                    .id("zen-outline-zone")
                    .absolute()
                    .top_2p5()
                    .left(px(docked_w + 12.0))
                    .opacity(if outline_open { 0.36 } else { 0.28 })
                    .hover(|s| s.opacity(1.0))
                    .child(
                        Button::new("zen-outline-toggle")
                            .ghost()
                            .compact()
                            .with_size(px(16.))
                            .w(px(26.))
                            .h(px(26.))
                            .rounded(px(6.))
                            .bg(gpui_kit::rgb(palette.surface))
                            .border_1()
                            .border_color(gpui_kit::rgb(palette.border))
                            .shadow_sm()
                            .icon(crate::ui::icon(IconName::ListTree))
                            .tooltip(if outline_open {
                                crate::i18n::workspace::TOOLTIP_COLLAPSE_OUTLINE.get()
                            } else {
                                crate::i18n::workspace::TOOLTIP_EXPAND_OUTLINE.get()
                            })
                            .on_click(move |_, window, cx| {
                                outline_pane.update(cx, |pane, cx| {
                                    pane.toggle_outline(cx);
                                    if pane.is_outline_open() && !pane.is_focused(window, cx) {
                                        pane.focus(window, cx);
                                    }
                                });
                            }),
                    ),
            );
        }
        area.into_any_element()
    }
}
