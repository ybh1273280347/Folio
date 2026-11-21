//! Main Markdown pane presentation and input surface.

use super::*;

impl Render for MarkdownPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_bounds = self.scroll.bounds();
        let pane_origin = pane_bounds.origin;
        let pane_size = pane_bounds.size;
        // Keep room below the last line without letting the canvas scroll indefinitely.
        let trailing_space = (pane_size.height * 0.4).clamp(px(160.), px(480.));

        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .pr(px(38.))
            .border_b_1()
            .border_color(rgb(0xe8e7e3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .when(!self.source, |row| {
                        row.child(outline::render_toggle_button(
                            "md-outline",
                            self.is_outline_open(),
                            cx,
                        ))
                    })
                    .child(crate::ui::icon(IconName::FileText))
                    .child(
                        div().min_w_0().text_ellipsis().child(
                            self.path
                                .file_name()
                                .expect("Markdown has filename")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .when(self.source, |row| {
                        row.child(
                            crate::ui::control("md-outline", IconName::ListTree)
                                .on_click(cx.listener(|pane, _, _, cx| pane.toggle_outline(cx))),
                        )
                    })
                    .child(
                        action(
                            "md-font-size",
                            format!("{}px", self.settings.markdown_font_size),
                        )
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.settings.markdown_font_size = 14;
                            pane.visual_editor.update(cx, |editor, cx| {
                                let body_size = px(pane.settings.markdown_font_size as f32);
                                editor.set_code_styles(
                                    crate::ui::inline_code_style(&pane.settings, body_size),
                                    crate::ui::block_code_style(&pane.settings, body_size),
                                    cx,
                                );
                            });
                            cx.emit(MarkdownEvent::Zoom(14));
                            cx.notify();
                        })),
                    )
                    .child(
                        action("md-source", crate::i18n::workspace::SOURCE_LABEL.get()).on_click(
                            cx.listener(|pane, _, window, cx| pane.toggle_source(window, cx)),
                        ),
                    )
                    .child({
                        let pane = cx.entity();
                        crate::ui::copy_document_button("md-copy-document", move |cx| {
                            // The pane retains the full Markdown, including content not yet rendered.
                            pane.read(cx).text.clone()
                        })
                    }),
            );

        let mut content = div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden();
        if self.source && self.outline {
            let headings = self.headings.iter().enumerate().map(|(index, heading)| {
                div()
                    .id(("md-heading", index))
                    .pl(px(12. + (heading.level.saturating_sub(1) as f32) * 12.))
                    .pr_2()
                    .py_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xefefed)))
                    .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                    .child(div().text_ellipsis().child(heading.title.clone()))
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.navigate_heading(index, window, cx)
                    }))
            });
            content = content
                .child(
                    div()
                        .id("markdown-outline")
                        .w(px(self.outline_width))
                        .flex_shrink_0()
                        .h_full()
                        .overflow_y_scroll()
                        .bg(rgb(0xfafaf9))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(crate::ui::label(
                            crate::i18n::markdown::HEADING_NAV.get().into(),
                        ))
                        .children(headings),
                )
                .child(
                    div()
                        .id("markdown-outline-resize")
                        .w(px(5.))
                        .flex_shrink_0()
                        .h_full()
                        .border_l_1()
                        .border_color(rgb(0xe8e7e3))
                        .cursor_col_resize()
                        .hover(|s| s.bg(rgb(0xe8e7e3)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|pane, ev: &MouseDownEvent, _, cx| {
                                pane.outline_drag =
                                    Some((f32::from(ev.position.x), pane.outline_width));
                                cx.stop_propagation();
                            }),
                        )
                        .on_drag(OutlineResizeDrag, |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| OutlineResizeDrag)
                        })
                        .on_drag_move(cx.listener(
                            |pane, e: &DragMoveEvent<OutlineResizeDrag>, _, cx| {
                                if let Some((start_x, start_w)) = pane.outline_drag {
                                    let delta = f32::from(e.event.position.x) - start_x;
                                    pane.outline_width = (start_w + delta).clamp(140., 480.);
                                    cx.notify();
                                }
                            },
                        )),
                );
        }

        if self.source {
            content = content.child(
                div().relative().flex_1().min_w_0().p_2().child(
                    Editor::new(&self.source_editor)
                        .appearance(false)
                        .bordered(false)
                        .size_full()
                        .font_family(if self.settings.editor_font.is_empty() {
                            SharedString::from("Consolas")
                        } else {
                            self.settings.editor_font.clone().into()
                        })
                        .text_size(px(self.settings.editor_font_size as f32)),
                ),
            );
        } else {
            // The title is document content, so it shares the body's scroll
            // owner and width instead of occupying a fixed row above the canvas.
            let document_title = div()
                .id("markdown-title")
                // The input uses percentage width; give it a full-width flex
                // row and a definite height inside the scrollable document.
                .flex()
                .w_full()
                .h(px(80.))
                .flex_shrink_0()
                .pb_6()
                .on_key_down(cx.listener(|pane, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        let path = pane.path.clone();
                        pane.set_path(path, window, cx);
                        pane.focus(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    Input::new(&self.title_input)
                        .appearance(false)
                        .bordered(false)
                        .w_full()
                        .h(px(56.))
                        .px_0()
                        .py_0()
                        .text_size(px(32.))
                        .line_height(relative(1.25))
                        .font_semibold()
                        .text_color(rgb(0x37352f)),
                );
            let floating_toolbar =
                self.render_floating_selection_toolbar(pane_origin, pane_size, cx);
            let slash_menu = self.render_slash_menu(pane_origin, pane_size, cx);
            let block_handle = self.render_left_block_handle(pane_origin, cx);
            let annotation_popover =
                self.render_active_annotation_popover(pane_origin, pane_size, cx);
            let translate_popover =
                self.translate
                    .render("md-translate", None, None, Some(pane_size), cx, |this| {
                        &mut this.translate
                    });

            let outline_overlay = self.is_outline_overlay();
            if self.is_outline_rendered() && !outline_overlay {
                content = content.child(outline::render_docked(self, cx));
            }

            content = content.child(
                // Floating controls must be outside the scroll owner: their
                // screen-relative positions otherwise enlarge its scroll bounds.
                div().relative().flex_1().min_w_0().min_h_0()
                .child(div()
                    .id("markdown-content")
                    .size_full()
                    .flex()
                    .items_start()
                    .justify_center()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll(cx.listener(|pane, _: &ScrollEvent, _, cx| {
                        pane.sync_active_heading(cx);
                    }))
                    .on_mouse_move(cx.listener(|pane, ev: &MouseMoveEvent, _, cx| {
                        pane.mouse_pos = Some(ev.position);
                        let next_id = pane
                            .visual_editor
                            .read(cx)
                            .top_block_at_or_active(Some(ev.position))
                            .map(|(id, _, _)| id);
                        if next_id != pane.hovered_block_id {
                            pane.hovered_block_id = next_id;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .flex_shrink_0()
                            .font_family(crate::ui::sans_font(&self.settings.markdown_font))
                            .text_size(px(self.settings.markdown_font_size as f32))
                            .when(self.settings.line_width_limited, |content| {
                                content.max_w(px(850.))
                            })
                            .pl(px(24.))
                            .pr_6()
                            .pt_6()
                            .pb(trailing_space)
                            .child(document_title)
                            .child(BlockEditor::new(&self.visual_editor).when(self.backdrop_visible, |editor| {
                                editor.with_content_background_opacity(0.35)
                            }))
                            .when(self.loading.is_some(), |content| {
                                content.child(
                                    div().mt_4().text_sm().text_color(rgb(0x787774))
                                        .child(crate::i18n::markdown::LOADING_MORE_CONTENT.get()),
                                )
                            }),
                    ))
                    .children(block_handle)
                    .children(floating_toolbar)
                    .children(slash_menu)
                    .children(annotation_popover)
                    .children(translate_popover)
                    .when(self.reading && !self.annotations.is_empty(), |area| {
                        let count = self.annotations.len();
                        area.child(
                            div()
                                .id("md-annotations-pill-reading")
                                .absolute()
                                .top_3()
                                .right_4()
                                .px_2p5()
                                .py_1()
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
                                .text_xs()
                                .text_color(rgb(0x854d0e))
                                .child(crate::ui::icon(IconName::MessageSquareText))
                                .child(format!("{count}"))
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.show_annotations_drawer = !pane.show_annotations_drawer;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!self.zen_mode, |area| {
                        area.when_some(self.hovered_link.as_ref(), |area, url| {
                            let label = crate::i18n::markdown::link_tooltip_click(
                                cfg!(target_os = "macos"),
                                url.starts_with('#'),
                            );
                            area.child(
                                div()
                                    .id("md-link-hover-pill")
                                    .absolute()
                                    .bottom_2()
                                    .left_3()
                                    .max_w(px(520.))
                                    .px_2()
                                    .py_1()
                                    .rounded(px(6.))
                                    .bg(rgb(0xf7f7f5))
                                    .border_1()
                                    .border_color(rgb(0xe2e4e8))
                                    .shadow_sm()
                                    .text_xs()
                                    .text_color(rgb(0x5b5b58))
                                    .text_ellipsis()
                                    .child(format!("{label}：{url}")),
                            )
                        })
                    })
                    .when(self.search_open && !self.source, |area| {
                        let palette = crate::theme::palette();
                        let query = self.search_input.read(cx).value().to_string();
                        let count_label = if query.is_empty() {
                            None
                        } else if self.search_match_total == 0 {
                            Some("0/0".to_owned())
                        } else {
                            Some(format!("{}/{}", self.search_match_index, self.search_match_total))
                        };
                        let right_offset = if self.reading && !self.annotations.is_empty() {
                            px(72.)
                        } else {
                            px(16.)
                        };
                        let active_bg = gpui_kit::rgb(0xdbeafe);
                        let active_text = gpui_kit::rgb(palette.accent);
                        let muted_text = gpui_kit::rgb(0x787774);
                        let case_on = self.search_case_sensitive;
                        let word_on = self.search_whole_word;
                        let replace_on = self.search_replace_open;

                        area.child(
                            div()
                                .id("md-search-bar")
                                .absolute()
                                .top_2p5()
                                .right(right_offset)
                                .px_2()
                                .py_1p5()
                                .rounded(px(8.))
                                .bg(gpui_kit::rgb(palette.surface))
                                .border_1()
                                .border_color(gpui_kit::rgb(palette.border))
                                .shadow_md()
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w(px(230.))
                                                .h(px(26.))
                                                .px_1p5()
                                                .rounded(px(6.))
                                                .bg(gpui_kit::rgb(palette.background))
                                                .border_1()
                                                .border_color(gpui_kit::rgb(palette.border))
                                                .flex()
                                                .items_center()
                                                .gap_0p5()
                                                .child(
                                                    Input::new(&self.search_input)
                                                        .appearance(false)
                                                        .bordered(false)
                                                        .xsmall()
                                                        .h(px(22.))
                                                        .text_size(px(12.))
                                                        .flex_1()
                                                        .min_w_0(),
                                                )
                                                .child(
                                                    crate::ui::control("toggle-md-case", IconName::CaseSensitive)
                                                        .size(px(20.))
                                                        .rounded(px(4.))
                                                        .when(case_on, |s| s.bg(active_bg).text_color(active_text))
                                                        .tooltip(|window, cx| {
                                                            Tooltip::new(crate::i18n::markdown::FIND_CASE_SENSITIVE.get()).build(window, cx)
                                                        })
                                                        .on_click(cx.listener(|pane, _, _, cx| {
                                                            pane.search_case_sensitive = !pane.search_case_sensitive;
                                                            let q = pane.search_input.read(cx).value().to_string();
                                                            pane.run_search(&q, 0, cx);
                                                            cx.notify();
                                                        })),
                                                )
                                                .child(
                                                    crate::ui::control("toggle-md-word", IconName::WholeWord)
                                                        .size(px(20.))
                                                        .rounded(px(4.))
                                                        .when(word_on, |s| s.bg(active_bg).text_color(active_text))
                                                        .tooltip(|window, cx| {
                                                            Tooltip::new(crate::i18n::markdown::FIND_WHOLE_WORD.get()).build(window, cx)
                                                        })
                                                        .on_click(cx.listener(|pane, _, _, cx| {
                                                            pane.search_whole_word = !pane.search_whole_word;
                                                            let q = pane.search_input.read(cx).value().to_string();
                                                            pane.run_search(&q, 0, cx);
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .when(!self.reading, |row| {
                                            row.child(
                                                crate::ui::control("toggle-md-replace", IconName::Replace)
                                                    .size(px(22.))
                                                    .when(replace_on, |s| s.bg(active_bg).text_color(active_text))
                                                    .tooltip(|window, cx| {
                                                        Tooltip::new(crate::i18n::markdown::FIND_TOGGLE_REPLACE.get()).build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|pane, _, window, cx| {
                                                        pane.toggle_replace(window, cx);
                                                    })),
                                            )
                                        })
                                        .child(
                                            div()
                                                .w(px(1.))
                                                .h(px(14.))
                                                .mx_0p5()
                                                .bg(gpui_kit::rgb(palette.border)),
                                        )
                                        .child(
                                            crate::ui::control("prev-md-search", IconName::ChevronLeft)
                                                .size(px(20.))
                                                .tooltip(|window, cx| {
                                                    Tooltip::new(crate::i18n::markdown::FIND_PREV.get()).build(window, cx)
                                                })
                                                .on_click(cx.listener(|pane, _, _, cx| {
                                                    let q = pane.search_input.read(cx).value().to_string();
                                                    pane.step_search(&q, -1, cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            crate::ui::control("next-md-search", IconName::ChevronRight)
                                                .size(px(20.))
                                                .tooltip(|window, cx| {
                                                    Tooltip::new(crate::i18n::markdown::FIND_NEXT.get()).build(window, cx)
                                                })
                                                .on_click(cx.listener(|pane, _, _, cx| {
                                                    let q = pane.search_input.read(cx).value().to_string();
                                                    pane.step_search(&q, 1, cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .when_some(count_label, |row, label| {
                                            row.child(
                                                div()
                                                    .px_1()
                                                    .text_size(px(11.))
                                                    .text_color(muted_text)
                                                    .flex_shrink_0()
                                                    .child(label),
                                            )
                                        })
                                        .child(
                                            crate::ui::control("close-md-search", IconName::Close)
                                                .size(px(20.))
                                                .tooltip(|window, cx| {
                                                    Tooltip::new(crate::i18n::markdown::FIND_CLOSE.get()).build(window, cx)
                                                })
                                                .on_click(cx.listener(|pane, _, window, cx| {
                                                    pane.close_search(window, cx);
                                                })),
                                        ),
                                )
                                .when(self.search_replace_open && !self.reading, |col| {
                                    col.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .w(px(230.))
                                                    .h(px(26.))
                                                    .px_1p5()
                                                    .rounded(px(6.))
                                                    .bg(gpui_kit::rgb(palette.background))
                                                    .border_1()
                                                    .border_color(gpui_kit::rgb(palette.border))
                                                    .flex()
                                                    .items_center()
                                                    .child(
                                                        Input::new(&self.replace_input)
                                                            .appearance(false)
                                                            .bordered(false)
                                                            .xsmall()
                                                            .h(px(22.))
                                                            .text_size(px(12.))
                                                            .flex_1()
                                                            .min_w_0(),
                                                    ),
                                            )
                                            .child(
                                                crate::ui::control("replace-md-one", IconName::Replace)
                                                    .size(px(22.))
                                                    .tooltip(|window, cx| {
                                                        Tooltip::new(crate::i18n::markdown::REPLACE_ONE.get()).build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.replace_current(cx);
                                                    })),
                                            )
                                            .child(
                                                crate::ui::control("replace-md-all", IconName::ReplaceAll)
                                                    .size(px(22.))
                                                    .tooltip(|window, cx| {
                                                        Tooltip::new(crate::i18n::markdown::REPLACE_ALL.get()).build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.replace_all(cx);
                                                    })),
                                            ),
                                    )
                                }),
                        )
                    })
                    .child({
                        let dark = crate::theme::palette().is_dark();
                        let thumb_bg = if dark { 0x45474b } else { 0xd9d9d7 };
                        let thumb_hover = if dark { 0x5c5f66 } else { 0xbcbcb9 };
                        let thumb_active = if dark { 0x72767f } else { 0xa8a8a5 };
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll)
                                .mode(ScrollbarMode::Always)
                                .styles(move |styles| {
                                    styles
                                        .thumb(move |style| {
                                            style
                                                .bg(gpui_kit::rgb(thumb_bg))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                        .thumb_hover(move |style| {
                                            style
                                                .bg(gpui_kit::rgb(thumb_hover))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                        .thumb_active(move |style| {
                                            style
                                                .bg(gpui_kit::rgb(thumb_active))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                }),
                        )
                    }),
            );

            // Observe outer pane width (for wide auto-open / narrow overlay switching)
            // and scroll offset changes (including scrollbar drag) after BlockEditor prepaints.
            {
                let weak_pane = cx.weak_entity();
                let pane_outer_width = self.pane_outer_width.clone();
                let last_scroll_y = self.last_scroll_y.clone();
                let scroll = self.scroll.clone();
                let visual_editor = self.visual_editor.clone();
                let current_active = self.active_heading;
                let initialized = self.outline_initialized;
                content = content.child(
                    canvas(
                        move |bounds, window, cx| {
                            let width = f32::from(bounds.size.width);
                            if width > 0.0 {
                                let prev = pane_outer_width.get();
                                let crossed_overlay = (prev > 0.0 && prev < outline::OVERLAY_WIDTH)
                                    != (width < outline::OVERLAY_WIDTH);
                                let crossed_auto = (prev < outline::AUTO_OPEN_WIDTH)
                                    != (width >= outline::AUTO_OPEN_WIDTH);
                                if !initialized || crossed_overlay || crossed_auto {
                                    let weak = weak_pane.clone();
                                    window.defer(cx, move |_, cx| {
                                        if let Some(pane) = weak.upgrade() {
                                            pane.update(cx, |pane, cx| {
                                                pane.handle_pane_width(width, cx)
                                            });
                                        }
                                    });
                                } else {
                                    pane_outer_width.set(width);
                                }
                            }
                            let scroll_y = scroll.offset().y;
                            if (f32::from(scroll_y) - f32::from(last_scroll_y.get())).abs() > 0.5 {
                                last_scroll_y.set(scroll_y);
                                let next = visual_editor.read(cx).active_heading_index(px(96.));
                                if next != current_active {
                                    let weak = weak_pane.clone();
                                    window.defer(cx, move |_, cx| {
                                        if let Some(pane) = weak.upgrade() {
                                            pane.update(cx, |pane, cx| {
                                                pane.sync_active_heading(cx)
                                            });
                                        }
                                    });
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );
            }

            if self.is_outline_rendered() && outline_overlay {
                content = content.child(outline::render_overlay(self, cx));
            }

            if self.show_annotations_drawer && !self.annotations.is_empty() {
                let quote_font_size = 12.0;
                let comment_font_size = 12.5;
                let title_font_size = 13.0;
                let drawer_width = 300.0;

                let cards = self.annotations.iter().map(|item| {
                    let id = item.id;
                    let quote = item.quote.clone();
                    let occ = item.occurrence;
                    let comment = item.comment.clone();
                    let color = item.color.clone();
                    let is_editing = self.editing_drawer_annotation_id == Some(id);
                    let badge_rgb = HIGHLIGHT_COLORS
                        .iter()
                        .find(|(n, _, _)| *n == item.color)
                        .map(|(_, _, hex)| *hex)
                        .unwrap_or(0xfacc15);
                    let drawer_swatches =
                        HIGHLIGHT_COLORS
                            .iter()
                            .enumerate()
                            .map(|(idx, (cname, _label, hex))| {
                                let active = color == *cname;
                                let next_color = (*cname).to_owned();
                                div()
                                    .id(("md-drawer-ann-color", id as usize * 10 + idx))
                                    .w(px(14.))
                                    .h(px(14.))
                                    .rounded(px(7.))
                                    .bg(rgb(*hex))
                                    .border_1()
                                    .border_color(if active {
                                        rgb(0x1f2937)
                                    } else {
                                        rgb(0xd1d5db)
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        if let Some(a) =
                                            pane.annotations.iter_mut().find(|a| a.id == id)
                                        {
                                            a.color = next_color.clone();
                                        }
                                        let _ = storage::save_annotations(
                                            &pane.path,
                                            &pane.annotations,
                                        );
                                        cx.notify();
                                    }))
                            });
                    let card_bg = Hsla::from(rgb(badge_rgb)).alpha(
                        if self.selected_annotation_id == Some(id) || is_editing {
                            0.14
                        } else {
                            0.08
                        },
                    );
                    let card_border = Hsla::from(rgb(badge_rgb)).alpha(
                        if self.selected_annotation_id == Some(id) || is_editing {
                            0.65
                        } else {
                            0.32
                        },
                    );
                    div()
                        .id(SharedString::from(format!("md-ann-card-{id}")))
                        .w_full()
                        .p_2p5()
                        .rounded(px(8.))
                        .bg(card_bg)
                        .border_1()
                        .border_color(card_border)
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .min_w_0()
                        .overflow_hidden()
                        .gap_1p5()
                        .cursor_pointer()
                        .hover(|s| s.border_color(Hsla::from(rgb(badge_rgb)).alpha(0.65)))
                        .active(|s| s.border_color(Hsla::from(rgb(badge_rgb)).alpha(0.85)))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .w(px(8.))
                                                .h(px(8.))
                                                .rounded(px(4.))
                                                .flex_shrink_0()
                                                .bg(rgb(badge_rgb)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_size(px(quote_font_size))
                                                .text_color(rgb(0x64748b))
                                                .text_ellipsis()
                                                .child(quote.clone()),
                                        ),
                                )
                                .child(
                                    crate::ui::control(
                                        SharedString::from(format!("del-ann-{id}")),
                                        IconName::Trash,
                                    )
                                    .flex_shrink_0()
                                    .on_click(cx.listener(
                                        move |pane, _, _, cx| {
                                            cx.stop_propagation();
                                            pane.annotations.retain(|a| a.id != id);
                                            if pane.selected_annotation_id == Some(id) {
                                                pane.selected_annotation_id = None;
                                            }
                                            if pane.active_annotation_id == Some(id) {
                                                pane.active_annotation_id = None;
                                            }
                                            if pane.editing_drawer_annotation_id == Some(id) {
                                                pane.editing_drawer_annotation_id = None;
                                            }
                                            let _ = storage::save_annotations(
                                                &pane.path,
                                                &pane.annotations,
                                            );
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                        .when(is_editing, |card| {
                            card.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_1p5()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_key_down(cx.listener(
                                        |pane, ev: &KeyDownEvent, window, cx| {
                                            if ev.keystroke.key == "escape" {
                                                pane.editing_drawer_annotation_id = None;
                                                pane.annotation_list_focus.focus(window, cx);
                                                cx.stop_propagation();
                                                cx.notify();
                                            }
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .children(drawer_swatches),
                                    )
                                    .child(
                                        Textarea::new(&self.drawer_note_input)
                                            .w_full()
                                            .text_size(px(comment_font_size)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .gap_1p5()
                                            .child(
                                                crate::ui::button(
                                                    "cancel-md-drawer-ann",
                                                    crate::i18n::common::CANCEL.get(),
                                                )
                                                .on_click(cx.listener(|pane, _, window, cx| {
                                                    pane.editing_drawer_annotation_id = None;
                                                    pane.annotation_list_focus.focus(window, cx);
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                crate::ui::button_primary(
                                                    "save-md-drawer-ann",
                                                    crate::i18n::common::SAVE.get(),
                                                )
                                                .on_click(cx.listener(
                                                    move |pane, _, window, cx| {
                                                        let next_comment = pane
                                                            .drawer_note_input
                                                            .read(cx)
                                                            .value()
                                                            .trim()
                                                            .to_string();
                                                        if let Some(a) = pane
                                                            .annotations
                                                            .iter_mut()
                                                            .find(|a| a.id == id)
                                                        {
                                                            a.comment = next_comment;
                                                        }
                                                        let _ = storage::save_annotations(
                                                            &pane.path,
                                                            &pane.annotations,
                                                        );
                                                        pane.editing_drawer_annotation_id = None;
                                                        pane.annotation_list_focus
                                                            .focus(window, cx);
                                                        cx.notify();
                                                    },
                                                )),
                                            ),
                                    ),
                            )
                        })
                        .when(!is_editing && !comment.is_empty(), |card| {
                            card.child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_size(px(comment_font_size))
                                    .text_color(rgb(0x334155))
                                    .child(comment.clone()),
                            )
                        })
                        .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
                            let visual_editor = pane.visual_editor.clone();
                            let drawer_note_input = pane.drawer_note_input.clone();
                            let quote_cl = quote.clone();
                            let comment_cl = comment.clone();
                            let double_click = event.click_count() >= 2;
                            pane.active_annotation_id = None;
                            pane.selected_annotation_id = Some(id);
                            if double_click {
                                pane.editing_drawer_annotation_id = Some(id);
                            } else if pane.editing_drawer_annotation_id != Some(id) {
                                pane.annotation_list_focus.focus(window, cx);
                            }
                            window.defer(cx, move |window, cx| {
                                visual_editor.update(cx, |editor, cx| {
                                    editor.select_quote_occurrence(&quote_cl, occ, cx);
                                });
                                if double_click {
                                    drawer_note_input.update(cx, |input, cx| {
                                        input.set_value(comment_cl, window, cx);
                                        input.focus(window, cx);
                                    });
                                }
                            });
                            cx.notify();
                        }))
                });
                content = div().flex_1().min_w_0().min_h_0().child(
                    h_resizable("markdown-annotations-layout")
                        .child(
                            resizable_panel()
                                .size_range(px(200.)..Pixels::MAX)
                                .child(content),
                        )
                        // The drawer scales with zoom and remains user-resizable.
                        .child(
                            resizable_panel()
                                .size(px(drawer_width))
                                .size_range(px(200.)..px(800.))
                                .fixed()
                                .child(
                                    div()
                                        .id("md-annotations-drawer")
                                        .track_focus(&self.annotation_list_focus)
                                        .on_key_down(cx.listener(
                                            |pane, event: &KeyDownEvent, window, cx| {
                                                // Deletion keys in the comment editor edit text, not the card.
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "delete" | "backspace"
                                                ) && pane.editing_drawer_annotation_id.is_none()
                                                    && pane.annotation_list_focus.is_focused(window)
                                                {
                                                    if let Some(id) =
                                                        pane.selected_annotation_id.take()
                                                    {
                                                        pane.annotations
                                                            .retain(|item| item.id != id);
                                                        if pane.active_annotation_id == Some(id) {
                                                            pane.active_annotation_id = None;
                                                        }
                                                        if pane.editing_drawer_annotation_id
                                                            == Some(id)
                                                        {
                                                            pane.editing_drawer_annotation_id =
                                                                None;
                                                        }
                                                        storage::save_annotations(
                                                            &pane.path,
                                                            &pane.annotations,
                                                        )
                                                        .expect(
                                                            "failed to save annotation deletion",
                                                        );
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    }
                                                }
                                            },
                                        ))
                                        .w_full()
                                        .min_w_0()
                                        .flex_shrink_0()
                                        .h_full()
                                        .min_h_0()
                                        .bg(rgb(0xfafaf9))
                                        .border_l_1()
                                        .border_color(rgb(0xe8e7e3))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .px_3()
                                                .pt_3()
                                                .pb_2()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(px(title_font_size))
                                                        .font_semibold()
                                                        .text_color(rgb(0x37352f))
                                                        .child(
                                                            crate::i18n::markdown::DOC_ANNOTATIONS
                                                                .get(),
                                                        ),
                                                )
                                                .child(
                                                    crate::ui::control(
                                                        "close-ann-drawer",
                                                        IconName::Close,
                                                    )
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.show_annotations_drawer = false;
                                                        cx.notify();
                                                    })),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .relative()
                                                .flex_1()
                                                .min_h_0()
                                                .child(
                                                    div()
                                                        .id("md-annotations-list")
                                                        .size_full()
                                                        .overflow_y_scroll()
                                                        .track_scroll(&self.annotation_list_scroll)
                                                        .px_3()
                                                        .pb_3()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .children(cards),
                                                )
                                                .child(div().absolute().inset_0().child(
                                                    Scrollbar::vertical(
                                                        &self.annotation_list_scroll,
                                                    ),
                                                )),
                                        ),
                                ),
                        ),
                );
            }
        }

        div()
            .relative()
            .size_full()
            .font_family(crate::ui::sans_font(&self.settings.interface_font))
            .text_size(px(13.))
            .when(!self.source, |pane| pane.key_context("FolioMarkdownVisual"))
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            // The workspace container supplies the surface so all text panes can share one backdrop.
            .when(self.reading, |pane| pane.child(toolbar))
            .child(crate::ui::ctrl_scroll_capture(cx.listener(
                |pane, event: &ScrollEvent, _, cx| {
                    if event.modifiers.control {
                        let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                            1
                        } else {
                            -1
                        };
                        pane.settings.markdown_font_size =
                            (pane.settings.markdown_font_size as i16 + delta).clamp(11, 26) as u8;
                        pane.visual_editor.update(cx, |editor, cx| {
                            let body_size = px(pane.settings.markdown_font_size as f32);
                            editor.set_code_styles(
                                crate::ui::inline_code_style(&pane.settings, body_size),
                                crate::ui::block_code_style(&pane.settings, body_size),
                                cx,
                            );
                        });
                        pane.zoom_hud
                            .show(pane.settings.markdown_font_size, cx, |pane| {
                                &mut pane.zoom_hud
                            });
                        cx.stop_propagation();
                        cx.notify();
                        cx.emit(MarkdownEvent::Zoom(pane.settings.markdown_font_size));
                    }
                },
            )))
            .child(content)
            .when_some(self.zoom_hud.render("md-zoom-hud"), |pane, hud| {
                pane.child(hud)
            })
            .on_key_down(cx.listener(|pane, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    if let Some(prefix) = pane.visual_editor.read(cx).caret_prefix()
                        && quick_insert_items(prefix).is_some()
                    {
                        pane.dismissed_slash_prefix = Some(prefix.to_owned());
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if !pane.source && pane.is_outline_open() && pane.is_outline_overlay() {
                        pane.close_outline(cx);
                        cx.stop_propagation();
                        return;
                    }
                    if pane.show_turn_into
                        || pane.show_color_picker
                        || pane.link_editing
                        || pane.block_menu_for.is_some()
                        || pane.active_annotation_id.is_some()
                        || pane.search_open
                        || pane.translate.is_open()
                    {
                        let was_search = pane.search_open;
                        pane.show_turn_into = false;
                        pane.show_color_picker = false;
                        pane.link_editing = false;
                        pane.block_menu_for = None;
                        pane.active_annotation_id = None;
                        pane.search_open = false;
                        pane.search_replace_open = false;
                        pane.translate.dismiss();
                        if was_search {
                            pane.defer_focus_visual(window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, _, _, cx| {
                    pane.show_turn_into = false;
                    pane.show_color_picker = false;
                    pane.block_menu_for = None;
                    if pane.translate.is_open() {
                        pane.translate.dismiss();
                        cx.notify();
                    }
                    cx.emit(MarkdownEvent::Focus);
                }),
            )
            .on_action(cx.listener(|pane, _: &FindInDocument, window, cx| {
                pane.open_search(window, cx);
            }))
            .on_action(cx.listener(|pane, _: &ReplaceInDocument, window, cx| {
                pane.open_replace(window, cx);
            }))
            .on_action(
                cx.listener(|pane, _: &gpui_kit::component::input::Search, window, cx| {
                    if !pane.source {
                        pane.open_search(window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|pane, _: &ToggleSource, window, cx| pane.toggle_source(window, cx)),
            )
            .on_action(cx.listener(|pane, _: &Bold, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Bold), cx)
            }))
            .on_action(cx.listener(|pane, _: &Italic, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Italic), cx)
            }))
            .on_action(cx.listener(|pane, _: &Underline, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Underline), cx)
            }))
            .on_action(cx.listener(|pane, _: &Strikethrough, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Strikethrough), cx)
            }))
            .on_action(cx.listener(|pane, _: &InlineCode, _, cx| {
                pane.run_command(EditorCommand::ToggleMark(MarkKind::Code), cx)
            }))
            .on_action(cx.listener(|pane, _: &Highlight, _, cx| {
                let c = pane.active_highlight_color.clone();
                pane.apply_highlight_color(&c, cx);
            }))
            .on_action(cx.listener(|pane, _: &LinkAction, _, cx| {
                pane.visual_editor
                    .update(cx, |editor, cx| editor.request_link_edit(cx));
            }))
            .on_action(cx.listener(|pane, _: &HeadingAction, _, cx| {
                pane.run_command(
                    EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
                    cx,
                )
            }))
            .on_action(cx.listener(|pane, _: &BulletList, _, cx| {
                pane.run_command(EditorCommand::ToggleList { ordered: false }, cx)
            }))
            .on_action(cx.listener(|pane, _: &TaskList, _, cx| {
                pane.run_command(EditorCommand::ToggleTask, cx)
            }))
            .on_action(
                cx.listener(|pane, _: &InsertImage, window, cx| pane.insert_image(window, cx)),
            )
            .on_action(cx.listener(|pane, _: &InsertTable, _, cx| {
                pane.run_command(EditorCommand::InsertTable { rows: 3, cols: 3 }, cx)
            }))
            .context_menu(|menu, _, _| {
                menu.menu(crate::i18n::markdown::CTX_BOLD.get(), Box::new(Bold))
                    .menu(crate::i18n::markdown::CTX_ITALIC.get(), Box::new(Italic))
                    .menu(
                        crate::i18n::markdown::CTX_UNDERLINE.get(),
                        Box::new(Underline),
                    )
                    .menu(
                        crate::i18n::markdown::CTX_STRIKETHROUGH.get(),
                        Box::new(Strikethrough),
                    )
                    .menu(
                        crate::i18n::markdown::CTX_HIGHLIGHT.get(),
                        Box::new(Highlight),
                    )
                    .separator()
                    .menu(
                        crate::i18n::markdown::CTX_HEADING.get(),
                        Box::new(HeadingAction),
                    )
                    .menu(
                        crate::i18n::markdown::CTX_BULLET_LIST.get(),
                        Box::new(BulletList),
                    )
                    .menu(
                        crate::i18n::markdown::CTX_TASK_LIST.get(),
                        Box::new(TaskList),
                    )
                    .separator()
                    .menu(
                        crate::i18n::markdown::CTX_INSERT_IMAGE.get(),
                        Box::new(InsertImage),
                    )
                    .menu(
                        crate::i18n::markdown::CTX_INSERT_TABLE.get(),
                        Box::new(InsertTable),
                    )
            })
            .when_some(self.formula_editor.clone(), |pane, (editor, display)| {
                // Block the underlying editor's mouse handler while the formula input is open.
                pane.child(crate::ui::modal_overlay(
                    crate::i18n::markdown::EDIT_FORMULA_TITLE.get(),
                    crate::i18n::markdown::EDIT_FORMULA_HINT.get().to_owned(),
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div().h(px(180.)).w_full().child(
                                Editor::new(&editor)
                                    .appearance(false)
                                    .bordered(true)
                                    .size_full()
                                    .font_family("Consolas")
                                    .text_size(px(18.)),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    crate::ui::button_secondary(
                                        "cancel-formula",
                                        crate::i18n::common::CANCEL.get(),
                                    )
                                    .on_click(cx.listener(
                                        |pane, _, window, cx| {
                                            pane.formula_editor = None;
                                            pane.defer_focus_visual(window, cx);
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    crate::ui::button_primary(
                                        "apply-formula",
                                        crate::i18n::common::APPLY.get(),
                                    )
                                    .on_click(cx.listener(
                                        move |pane, _, window, cx| {
                                            let latex = editor.read(cx).value().to_string();
                                            let source = if display {
                                                format!("$$\n{latex}\n$$")
                                            } else {
                                                format!("${latex}$")
                                            };
                                            pane.visual_editor.update(cx, |editor, cx| {
                                                editor.replace_selected_markdown(&source, cx);
                                            });
                                            pane.defer_focus_visual(window, cx);
                                            pane.formula_editor = None;
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                ))
            })
            .when_some(self.pending_image.clone(), |pane, (_, destination)| {
                pane.child(crate::ui::modal_overlay(
                    crate::i18n::markdown::OVERWRITE_IMAGE_TITLE.get(),
                    destination.to_string_lossy().into_owned(),
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::ui::button_secondary(
                                "cancel-image-overwrite",
                                crate::i18n::common::CANCEL.get(),
                            )
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.pending_image = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            crate::ui::button_danger(
                                "confirm-image-overwrite",
                                crate::i18n::common::REPLACE.get(),
                            )
                            .on_click(cx.listener(|pane, _, _, cx| pane.confirm_image_import(cx))),
                        ),
                ))
            })
            .when_some(self.image_error.clone(), |pane, error| {
                pane.child(crate::ui::modal_overlay(
                    crate::i18n::markdown::IMAGE_IMPORT_FAILED.get(),
                    error,
                    div().flex().justify_end().child(
                        crate::ui::button_primary(
                            "dismiss-image-error",
                            crate::i18n::common::CONFIRM.get(),
                        )
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.image_error = None;
                            cx.notify();
                        })),
                    ),
                ))
            })
    }
}
