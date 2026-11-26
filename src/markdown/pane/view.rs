//! Main Markdown pane presentation and input surface.

use super::*;

impl Render for MarkdownPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_bounds = self.scroll.bounds();
        let pane_origin = pane_bounds.origin;
        let pane_size = pane_bounds.size;
        // Keep room below the last line without letting the canvas scroll indefinitely.
        let trailing_space = (pane_size.height * 0.4).clamp(px(160.), px(480.));

        let mut content = div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden();
        let outline_overlay = self.is_outline_overlay();
        if self.is_outline_rendered() && !outline_overlay {
            content = content.child(outline::render_docked(self, window, cx));
        }
        if self.source && self.is_outline_open() {
            content = content.child(
                div()
                    .id("markdown-outline-resize")
                    .w(px(5.))
                    .flex_shrink_0()
                    .h_full()
                    .border_l_1()
                    .border_color(crate::theme::color(0xe8e7e3))
                    .cursor_col_resize()
                    .hover(|s| s.bg(crate::theme::color(0xe8e7e3)))
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
            let loading = self.source_editor.read(cx).is_loading_wrapped_value();
            if !loading && self.pending_source_jump.is_some() {
                let pane = cx.entity().downgrade();
                window.on_next_frame(move |window, cx| {
                    let _ = pane.update(cx, |pane, cx| {
                        if pane.source
                            && !pane.source_editor.read(cx).is_loading_wrapped_value()
                            && let Some(jump) = pane.pending_source_jump.take()
                        {
                            match jump {
                                PendingSourceJump::Heading(index) => pane.navigate_heading(index, window, cx),
                                PendingSourceJump::KbHit(hit) => pane.jump_to_kb_hit(&hit, window, cx),
                            }
                        }
                    });
                });
            }
            let pane = cx.entity();
            let editor = Editor::new(&self.source_editor)
                .appearance(false)
                .bordered(false)
                .size_full()
                .font_family(if self.settings.editor_font.is_empty() {
                    SharedString::from("Consolas")
                } else {
                    self.settings.editor_font.clone().into()
                })
                .text_size(px(self.settings.editor_font_size as f32))
                .on_paste(move |item, window, cx| {
                    pane.update(cx, |pane, cx| pane.handle_source_paste(item, window, cx))
                });
            content = content.child(
                div().relative().flex_1().min_w_0().min_h_0().p_2().flex().justify_center()
                    // Mount the empty editor during preparation so it can measure the
                    // actual text area without shaping the entire source in prepaint.
                    .child(div().size_full().min_w_0()
                        .when(self.settings.line_width_limited, |editor| editor.max_w(px(850.)))
                        .when(loading, |editor| editor.invisible())
                        .child(editor))
                    .when(loading, |source| {
                        source.child(div().absolute().inset_2().text_sm().text_color(rgb(0x787774))
                            .child(crate::i18n::markdown::LOADING_MORE_CONTENT.get()))
                    }),
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

            let show_templates = self.show_empty_templates();
            let template_pills = show_templates.then(|| {
                div()
                    .id("md-empty-templates")
                    .w_full()
                    .pb_4()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("md-template-todo")
                            .h(px(30.))
                            .px_3()
                            .rounded_full()
                            .bg(rgb(0xf4f4f2))
                            .border_1()
                            .border_color(rgb(0xe8e7e3))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xeae9e5)))
                            .active(|s| s.bg(rgb(0xe2e1dc)))
                            .on_click(cx.listener(|pane, _, window, cx| {
                                pane.apply_empty_template("todo", window, cx);
                            }))
                            .child(
                                crate::ui::icon(IconName::ListTodo)
                                    .size(px(14.))
                                    .text_color(rgb(0x16a34a)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(0x37352f))
                                    .child(crate::i18n::markdown::TEMPLATE_TODO_LABEL.get()),
                            ),
                    )
            });
            let template_ghost_preview = show_templates.then(|| {
                let date_label = crate::i18n::markdown::today_date_or_fallback();
                let ghost_items = [
                    crate::i18n::markdown::TEMPLATE_TODO_CH1.get(),
                    crate::i18n::markdown::TEMPLATE_TODO_CH2.get(),
                    crate::i18n::markdown::TEMPLATE_TODO_CH3.get(),
                ];
                let item_size = px(self.settings.markdown_font_size as f32);
                div()
                    .id("md-template-ghost-preview")
                    .w_full()
                    .pt_2()
                    .opacity(0.48)
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|pane, _, window, cx| {
                            pane.defer_focus_visual(window, cx);
                        }),
                    )
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .child(
                        div()
                            .text_size(px(24.))
                            .line_height(relative(1.3))
                            .font_semibold()
                            .text_color(rgb(0x787774))
                            .child(crate::i18n::markdown::TEMPLATE_TODO_TITLE.get()),
                    )
                    .child(
                        div()
                            .pt_1()
                            .text_size(px(18.))
                            .line_height(relative(1.35))
                            .font_semibold()
                            .text_color(rgb(0x787774))
                            .child(date_label),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(ghost_items.into_iter().map(|label| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2p5()
                                    .child(
                                        div()
                                            .size(px(15.))
                                            .flex_shrink_0()
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(rgb(0xa8a7a2)),
                                    )
                                    .child(
                                        div()
                                            .text_size(item_size)
                                            .text_color(rgb(0x787774))
                                            .child(label),
                                    )
                            })),
                    )
            });

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
                            .px(px(36.))
                            .pt_6()
                            .pb(trailing_space)
                            .child(document_title)
                            .children(template_pills)
                            .child(BlockEditor::new(&self.visual_editor).when(self.backdrop_visible, |editor| {
                                editor.with_content_background_opacity(0.35)
                            }))
                            .children(template_ghost_preview)
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
                    .when(!self.zen_mode, |area| {
                        area.when_some(self.hovered_link.as_ref(), |area, url| {
                            let label = crate::i18n::markdown::link_tooltip_click(
                                cfg!(target_os = "macos"),
                                url.starts_with('#'),
                            );
                            area.child(
                                div()
                                    .absolute()
                                    .bottom_2()
                                    .left_3()
                                    .child(crate::ui::popover_enter(
                                        div()
                                            .id("md-link-hover-pill")
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
                                        "md-link-hover-enter",
                                        crate::ui::PopoverDirection::Fade,
                                    )),
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
                        let right_offset = px(16.);
                        let active_bg = gpui_kit::rgb(palette.accent).opacity(if palette.is_dark() {
                            0.22
                        } else {
                            0.14
                        });
                        let active_text = gpui_kit::rgb(palette.accent);
                        let muted_text = gpui_kit::rgb(palette.muted);
                        let case_on = self.search_case_sensitive;
                        let word_on = self.search_whole_word;
                        let replace_on = self.search_replace_open;
                        let replace_open = self.search_replace_open && !self.reading;
                        let replace_progress = crate::ui::sample_disclosure(
                            ("md-search-replace-fold", cx.entity_id().as_u64() as usize),
                            replace_open,
                            window,
                            cx,
                        );
                        let replace_rendered = replace_open || replace_progress > 0.001;

                        let search_card = div()
                            .id("md-search-bar")
                            .px_2()
                            .py_1p5()
                            .rounded(px(10.))
                            .bg(palette.elevated_surface())
                            .border_1()
                            .border_color(palette.card_border())
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
                            .when(replace_rendered, |col| {
                                let replace_row = div()
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
                                    );
                                col.child(crate::ui::disclosure_body(
                                    ("md-search-replace-body", cx.entity_id().as_u64() as usize),
                                    replace_open,
                                    replace_progress,
                                    replace_row,
                                    window,
                                    cx,
                                ))
                            });

                        area.child(
                            div()
                                .absolute()
                                .top_2p5()
                                .right(right_offset)
                                .child(crate::ui::popover_enter(
                                    search_card,
                                    "md-search-bar-enter",
                                    crate::ui::PopoverDirection::Down,
                                )),
                        )
                    })
                    .child({
                        let dark = crate::theme::palette().is_dark();
                        let thumb_bg = if dark { 0x45474b } else { 0xd9d9d7 };
                        let thumb_hover = if dark { 0x5c5f66 } else { 0xbcbcb9 };
                        let thumb_active = if dark { 0x72767f } else { 0xa8a8a5 };
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll)
                                .mode(ScrollbarMode::Hover)
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
                content = content.child(outline::render_overlay(self, window, cx));
            }

            let ann_open = self.show_annotations_drawer && !self.annotations.is_empty();
            let ann_progress = crate::ui::sample_drawer(
                ("md-annotations-drawer-motion", cx.entity_id().as_u64() as usize),
                ann_open,
                200,
                window,
                cx,
            );
            let ann_rendered = ann_open || ann_progress > 0.001;
            let drawer_panel = if ann_rendered {
                let quote_font_size = 12.0;
                let comment_font_size = 12.5;
                let title_font_size = 13.0;
                let drawer_width = self.annotations_drawer_width.clamp(200., 800.);

                let cards = self.annotations.iter().map(|item| {
                    let id = item.id;
                    let quote = item.quote.clone();
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
                                        let _ = annotations::save_annotations(
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
                                        move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.emit(MarkdownEvent::DeleteAnnotation(id));
                                        },
                                    )),
                                ),
                        )
                        .when(is_editing, |card| {
                            card.child(crate::ui::popover_enter(
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
                                                        let _ = annotations::save_annotations(
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
                                ("md-drawer-ann-edit-enter", id as usize),
                                crate::ui::PopoverDirection::Down,
                            ))
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
                            let pane_view = cx.entity().downgrade();
                            let drawer_note_input = pane.drawer_note_input.clone();
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
                                let _ = pane_view.update(cx, |pane, cx| {
                                    pane.reveal_annotation(id, cx);
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

                let drawer_body = div()
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
                                && let Some(id) = pane.selected_annotation_id
                            {
                                cx.stop_propagation();
                                cx.emit(MarkdownEvent::DeleteAnnotation(id));
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
                                )
                                .mode(ScrollbarMode::Hover),
                            )),
                    );

                let drag_pane = cx.entity();
                Some(
                    crate::ui::horizontal_drawer(
                        crate::ui::DrawerSide::Right,
                        ann_open,
                        ann_progress,
                        drawer_width,
                        drawer_body,
                    )
                    .when(ann_open && ann_progress >= 0.999, |drawer| {
                        drawer.child(
                            resize_handle("md-annotations-resize", Axis::Horizontal)
                                .inside(HandleEdge::Leading)
                                .on_drag(
                                    AnnotationsDrawerResizeDrag,
                                    move |_, _, window, cx| {
                                        let start_x = f32::from(window.mouse_position().x);
                                        drag_pane.update(cx, |pane, _| {
                                            pane.annotations_drawer_drag = Some((
                                                start_x,
                                                pane.annotations_drawer_width.clamp(200., 800.),
                                            ));
                                        });
                                        cx.new(|_| AnnotationsDrawerResizeDrag)
                                    },
                                ),
                        )
                    }),
                )
            } else {
                None
            };

            content = div().relative().flex().flex_1().min_w_0().min_h_0().child(
                div()
                    .id("markdown-annotations-layout")
                    .relative()
                    .flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .on_drag_move(cx.listener(
                        |pane, event: &DragMoveEvent<AnnotationsDrawerResizeDrag>, _, cx| {
                            let Some((start_x, start_w)) = pane.annotations_drawer_drag else {
                                return;
                            };
                            let delta = f32::from(event.event.position.x) - start_x;
                            let next_w = (start_w - delta).clamp(200., 800.).round();
                            if (pane.annotations_drawer_width - next_w).abs() >= 1.0 {
                                pane.annotations_drawer_width = next_w;
                                cx.notify();
                            }
                        },
                    ))
                    .child(div().relative().flex().flex_1().min_w_0().min_h_0().h_full().child(content))
                    .children(drawer_panel),
            );
        }

        // Pane chrome: the badge stays put while the document scrolls and is
        // shared by both view modes, which report the same source text.
        if self.char_count > 0 && !self.show_annotations_drawer {
            content = content.child(crate::ui::char_count_badge(self.char_count));
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
            // The workspace container supplies the surface and unified header bar.
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
                        pane.close_color_picker();
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
                    pane.close_color_picker();
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
            .folio_context_menu(|menu, _, _| {
                menu.min_w(px(176.))
                    .action_item(crate::i18n::markdown::CTX_BOLD.get(), Some(IconName::Bold), Box::new(Bold))
                    .action_item(crate::i18n::markdown::CTX_ITALIC.get(), Some(IconName::Italic), Box::new(Italic))
                    .action_item(
                        crate::i18n::markdown::CTX_UNDERLINE.get(),
                        Some(IconName::Underline),
                        Box::new(Underline),
                    )
                    .action_item(
                        crate::i18n::markdown::CTX_STRIKETHROUGH.get(),
                        Some(IconName::Strikethrough),
                        Box::new(Strikethrough),
                    )
                    .action_item(
                        crate::i18n::markdown::CTX_HIGHLIGHT.get(),
                        Some(IconName::Highlighter),
                        Box::new(Highlight),
                    )
                    .separator()
                    .action_item(
                        crate::i18n::markdown::CTX_HEADING.get(),
                        Some(IconName::Heading2),
                        Box::new(HeadingAction),
                    )
                    .action_item(
                        crate::i18n::markdown::CTX_BULLET_LIST.get(),
                        Some(IconName::List),
                        Box::new(BulletList),
                    )
                    .action_item(
                        crate::i18n::markdown::CTX_TASK_LIST.get(),
                        Some(IconName::ListTodo),
                        Box::new(TaskList),
                    )
                    .separator()
                    .action_item(
                        crate::i18n::markdown::CTX_INSERT_IMAGE.get(),
                        Some(IconName::Image),
                        Box::new(InsertImage),
                    )
                    .action_item(
                        crate::i18n::markdown::CTX_INSERT_TABLE.get(),
                        Some(IconName::Table2),
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
