//! Chat transcript, tool activity and composer rendering.

use super::*;

pub(super) fn inline_quote_token(
    context: &gpui_kit::component::input::InlineTokenContext,
    quote: SelectionQuote,
    palette: Palette,
    font_size: Pixels,
) -> AnyElement {
    let tip = quote.tooltip();
    div()
        .id(("ai-inline-quote", context.range().start))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(
            InputToken::new(context)
                .px(px(2.))
                .h(context.line_height() - px(2.))
                .text_size(font_size - px(1.))
                .border_color(gpui::transparent_black())
                .when(!context.is_selected(), |token| token.bg(gpui::transparent_black()))
                .text_color(gpui_kit::rgb(palette.accent)),
        )
        .into_any_element()
}

const QUOTE_LINK: &str = "folio-inline-quote:";

#[derive(Clone)]
pub(super) struct UserMessageBody {
    pub(super) input: InputContent,
    markdown: SharedString,
    quote_tooltips: Arc<[String]>,
}

impl UserMessageBody {
    pub(super) fn new(input: InputContent) -> Self {
        let (markdown, quote_tooltips) = if input.tokens().is_empty() {
            (input.text().clone(), Arc::default())
        } else {
            quoted_markdown(&input)
        };
        Self { input, markdown, quote_tooltips }
    }

    pub(super) fn render(&self, message_index: usize) -> TextView {
        let id = SharedString::from(format!("ai-user-md-{message_index}"));
        let view = TextView::markdown(id, self.markdown.clone());
        if self.quote_tooltips.is_empty() {
            return view;
        }
        let tips = self.quote_tooltips.clone();
        view.link_tooltip(move |url, window, cx| {
            let tip = url.strip_prefix(QUOTE_LINK)
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| tips.get(index))
                .cloned()
                .unwrap_or_else(|| url.to_string());
            Tooltip::new(tip).build(window, cx)
        })
        .on_link_click(|url, _, _, cx| {
            if !url.starts_with(QUOTE_LINK) {
                cx.open_url(url);
            }
        })
    }
}

fn quoted_markdown(body: &InputContent) -> (SharedString, Arc<[String]>) {
    // Markdown still owns wrapping and formatting of the text between quote tokens.
    let mut markdown = String::new();
    let mut tips = Vec::new();
    let mut excerpts = Vec::new();
    let mut cursor = 0;
    for span in body.tokens() {
        let range = span.range();
        let quote = SelectionQuote::from_token(span.token()).expect("sent quote token");
        markdown.push_str(&body.text()[cursor..range.start]);
        let start = markdown.len();
        markdown.push('[');
        for ch in span.token().label().chars() {
            if ch.is_ascii_punctuation() {
                markdown.push('\\');
            }
            markdown.push(ch);
        }
        use std::fmt::Write as _;
        write!(markdown, "]({QUOTE_LINK}{})", tips.len()).expect("writing to a string");
        excerpts.push((start..markdown.len(), span.token().label()));
        tips.push(quote.tooltip());
        cursor = range.end;
    }
    markdown.push_str(&body.text()[cursor..]);

    // A quote inside literal Markdown, such as a code span, remains a visible excerpt.
    // Do not show the generated Markdown link syntax in a context that cannot parse it.
    let mut recognized = std::collections::BTreeSet::new();
    if let Ok(root) = ::markdown::to_mdast(&markdown, &::markdown::ParseOptions::gfm()) {
        let mut nodes = vec![&root];
        while let Some(node) = nodes.pop() {
            if let ::markdown::mdast::Node::Link(link) = node
                && link.url.starts_with(QUOTE_LINK)
                && let Some(position) = &link.position
            {
                recognized.insert(position.start.offset);
            }
            nodes.extend(node.children().into_iter().flatten());
        }
    }
    for (range, label) in excerpts.into_iter().rev() {
        if !recognized.contains(&range.start) {
            markdown.replace_range(range, label);
        }
    }
    (markdown.into(), tips.into())
}

impl ChatPane {
    pub(super) fn render_turn_navigation(
        &self,
        turns: &[TurnAnchor],
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let top = self.scroll.top_item();
        let active = turns.partition_point(|turn| turn.item <= top).saturating_sub(1);
        let row_height = (typo.sm * 1.7).max(px(26.));
        let trigger_size = px(26.);
        let gap = px(6.);
        let row = div()
            .id("ai-turn-navigation")
            .relative()
            .flex()
            .items_center()
            .gap(gap)
            .child(
                div()
                    .id("ai-turn-navigation-toggle")
                    .size(trigger_size)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .text_color(gpui_kit::rgb(palette.muted))
                    .hover(|style| style.bg(gpui_kit::rgb(palette.hover)))
                    .when(self.turn_navigation_open, |el| {
                        el.bg(gpui_kit::rgb(palette.hover))
                            .text_color(gpui_kit::rgb(palette.accent))
                    })
                    .child(crate::ui::icon(IconName::List).size(px(14.)))
                    .on_hover(cx.listener(move |pane, hovered, _, cx| {
                        if *hovered && !pane.turn_navigation_open {
                            pane.turn_navigation_open = true;
                            pane.turn_navigation_scroll.scroll_to_item(active);
                            cx.notify();
                        }
                    })),
            )
            .when(self.turn_navigation_open, |row| {
                let pane = cx.entity().downgrade();
                row.w_full()
                    .h_full()
                    .max_w(px(374.))
                    .max_h(row_height * turns.len().min(9) as f32 + typo.sm * 1.5 + px(16.))
                    .child(canvas(|_, _, _| (), move |bounds, (), window, cx| {
                        // Use geometry because the popup's hitbox blocks mouse hits on its parent.
                        // Include the narrow bridge from the trigger to the list.
                        let close = move |cx: &mut App| {
                            let _ = pane.update(cx, |pane, cx| {
                                pane.turn_navigation_open = false;
                                cx.notify();
                            });
                        };
                        let contains = move |point: Point<Pixels>| {
                            bounds.contains(&point)
                                && (point.x >= bounds.left() + trigger_size + gap
                                    || (point.y - bounds.center().y).abs() <= trigger_size / 2.)
                        };
                        // A quick pass over the trigger can finish before the expanded row paints.
                        if !contains(window.mouse_position()) {
                            let close = close.clone();
                            window.defer(cx, move |_, cx| close(cx));
                        }
                        let on_move = close.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase.capture() && !contains(event.position) {
                                on_move(cx);
                            }
                        });
                        window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                            if phase.capture() {
                                close(cx);
                            }
                        });
                    }).absolute().inset_0())
                    .child(
                        div()
                            .relative()
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .p_1()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(subtle_border(palette.border))
                            .bg(gpui_kit::rgb(palette.surface))
                            .shadow(popover_shadow(palette.is_dark()))
                            .font_family(ui_font.clone())
                            .text_size(typo.sm)
                            .occlude()
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px_2()
                                    .py_0p5()
                                    .line_height(relative(1.5))
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child(format!(
                                        "{} ({})",
                                        crate::i18n::ai::TURN_NAVIGATION.get(),
                                        turns.len(),
                                    )),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div()
                                            .id("ai-turn-navigation-list")
                                            .size_full()
                                            .flex()
                                            .flex_col()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.turn_navigation_scroll)
                                            .pr_2()
                                            .children(turns.iter().enumerate().map(|(index, turn)| {
                                                let item = turn.item;
                                                let timestamp = turn.timestamp_ms
                                                    .and_then(chrono::DateTime::from_timestamp_millis)
                                                    .map(|time| {
                                                        time.with_timezone(&chrono::Local)
                                                            .format("%m-%d %H:%M")
                                                            .to_string()
                                                    })
                                                    .unwrap_or_else(|| "—".into());
                                                div()
                                                    .id(("ai-turn-anchor", item))
                                                    .h(row_height)
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .px_2()
                                                    .rounded(px(5.))
                                                    .cursor_pointer()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .when(index == active, |el| {
                                                        el.bg(fg_alpha(palette.accent, 0.12))
                                                    })
                                                    .hover(|style| style.bg(gpui_kit::rgb(palette.hover)))
                                                    .child(
                                                        div()
                                                            .flex_shrink_0()
                                                            .w(typo.sm * 6.8)
                                                            .text_color(gpui_kit::rgb(palette.muted))
                                                            .child(timestamp),
                                                    )
                                                    .child(div().min_w_0().flex_1().text_ellipsis()
                                                        .child(turn.preview.clone()))
                                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                                        pane.turn_navigation_open = false;
                                                        pane.stick_to_bottom = false;
                                                        pane.scroll.scroll_to_top_of_item(item);
                                                        cx.notify();
                                                    }))
                                            })),
                                    )
                                    .child(
                                        div().absolute().inset_0().child(
                                            Scrollbar::vertical(&self.turn_navigation_scroll)
                                                .mode(ScrollbarMode::Hover)
                                                .styles(move |styles| {
                                                    styles.thumb(move |style| {
                                                        style
                                                            .width(px(4.))
                                                            .radius(px(2.))
                                                            .bg(fg_alpha(palette.text, 0.20))
                                                    })
                                                }),
                                        ),
                                    ),
                            ),
                    )
            });

        div()
            .absolute()
            .left_1()
            .right_3()
            .top_2()
            .bottom_2()
            .flex()
            .items_center()
            .child(row)
            .into_any_element()
    }

    pub(super) fn md_style(&self, body_size: Pixels, cx: &App) -> TextViewStyle {
        let base = TextViewStyle::from_theme(&gpui_kit::base::Theme::global(cx));
        let soft_selection = base.selection().alpha(0.3);
        base.with_inline_code_style(crate::ui::inline_code_style(&self.settings, body_size))
            .with_code_block(crate::ui::block_code_style(&self.settings, body_size))
            .with_table(crate::ui::markdown_table_style())
            .with_selection(soft_selection)
    }

    pub(super) fn render_diff_card(
        &self,
        id: SharedString,
        diff: &FileDiff,
        code_font: &SharedString,
        palette: Palette,
    ) -> AnyElement {
        if let Some(patch) = &diff.patch {
            return DiffPatchView {
                id,
                patch: patch.clone(),
            }
            .into_any_element();
        }
        let add_tint = mix_color(palette.surface, 0x22c55e, 0.14);
        let add_fg = theme_color(0x16a34a);
        let del_tint = mix_color(palette.surface, palette.danger, 0.14);
        let del_fg = gpui_kit::rgb(palette.danger);
        let muted = gpui_kit::rgb(palette.muted);
        let typo = AiTypography::from_settings(&self.settings);

        div()
            .min_w_0()
            .rounded(px(6.))
            .border_1()
            .border_color(gpui_kit::rgb(palette.border))
            .overflow_hidden()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id(id)
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .font_family(code_font.clone())
                    .text_size(typo.sm)
                    .line_height(relative(1.4))
                    .children(diff.lines.iter().take(120).map(|line| {
                        let (number, text, marker, fg, bg) = match line {
                            DiffLine::Context(n, s) => (n, s, "", muted, None),
                            DiffLine::Removed(n, s) => (n, s, "−", del_fg, Some(del_tint)),
                            DiffLine::Added(n, s) => (n, s, "+", add_fg, Some(add_tint)),
                            DiffLine::Ellipsis => {
                                return div()
                                    .h(px(3.))
                                    .bg(gpui_kit::rgb(palette.border))
                                    .into_any_element();
                            }
                        };
                        div()
                            .flex()
                            .min_h(typo.sm * 1.4)
                            .pr_2()
                            .text_color(fg)
                            .when_some(bg, |el, bg| el.bg(bg))
                            .when_some(*number, |el, n| {
                                el.child(
                                    div()
                                        .w(typo.sm * 4.6)
                                        .pr_2()
                                        .flex_shrink_0()
                                        .text_right()
                                        .child(n.to_string()),
                                )
                            })
                            .child(div().w_5().flex_shrink_0().text_center().child(marker))
                            .child(div().min_w_0().flex_1().child(text.clone()))
                            .into_any_element()
                    }))
                    .when(diff.lines.len() > 120, |el| {
                        el.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_color(muted)
                                .child(crate::i18n::ai::PREVIEW_FIRST_120_LINES.get()),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_markdown_block(
        &self,
        id: SharedString,
        text: &str,
        msg_idx: usize,
        part_idx: usize,
        md_style: &TextViewStyle,
        ui_font: &SharedString,
        palette: Palette,
        typo: AiTypography,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let normalized = markdown_editor::normalize_math(text);
        let chat = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!(
                "ai-quote-source-{msg_idx}-{part_idx}"
            )))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |pane, event: &MouseDownEvent, _, _| {
                    // Streaming must not scroll the paragraph away while a selection is being made.
                    pane.stick_to_bottom = false;
                    pane.quote_drag = Some((msg_idx, part_idx, event.position));
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |pane, event: &MouseUpEvent, window, cx| {
                    let Some((start_msg, start_part, anchor)) = pane.quote_drag.take() else {
                        return;
                    };
                    if (start_msg, start_part) != (msg_idx, part_idx) {
                        return;
                    }
                    let position = point(
                        anchor.x.min(event.position.x),
                        anchor.y.min(event.position.y),
                    );
                    let view = cx.entity().downgrade();
                    // Read after the window's selection layer has completed the mouse-up gesture.
                    window.defer(cx, move |window, cx| {
                        let text = TextSelection::selected_text(window, cx);
                        let text = text.trim();
                        if text.is_empty() {
                            return;
                        }
                        let _ = view.update(cx, |pane, cx| {
                            pane.stick_to_bottom = false;
                            pane.quote_popover = Some(AnswerQuotePopover {
                                quote: SelectionQuote {
                                    source: crate::i18n::ai::AI_ANSWER_SOURCE.get().into(),
                                    text: text.to_owned(),
                                },
                                position,
                                scroll_offset: pane.scroll.offset(),
                            });
                            cx.notify();
                        });
                    });
                }),
            )
            .font_family(ui_font.clone())
            .text_size(typo.body)
            .line_height(relative(1.6))
            .text_color(gpui_kit::rgb(palette.text))
            .child(
                TextView::markdown(id, normalized)
                    .selectable(true)
                    .markdown_extensions(self.md_extensions.clone())
                    .on_link_click(move |url, event, _, cx| {
                        // Match TextView's activation gestures when overriding its link handler.
                        let activate = match event {
                            ClickEvent::Mouse(click) => {
                                matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
                            }
                            ClickEvent::Keyboard(_) => true,
                            ClickEvent::Touch(click) => !click.long_press,
                        };
                        if !activate {
                            return;
                        }
                        let relative = url.starts_with("./");
                        if !relative && !url.starts_with("file://") {
                            cx.open_url(url);
                            return;
                        }
                        let _ = chat.update(cx, |pane, cx| {
                            // Relative citations belong to the conversation's workspace.
                            let path = if relative {
                                let path = url.split(['#', '?']).next().expect("link has a path");
                                Some(
                                    pane.session
                                        .borrow()
                                        .cwd
                                        .join(crate::files::percent_decode(path)),
                                )
                            } else {
                                url::Url::parse(url)
                                    .ok()
                                    .and_then(|uri| uri.to_file_path().ok())
                            };
                            if let Some(path) = path {
                                cx.emit(AiPaneEvent::OpenFile(path));
                            } else {
                                pane.show_toast(
                                    crate::i18n::ai::ERR_RESOLVE_FILE_REF.get().into(),
                                    true,
                                    cx,
                                );
                            }
                        });
                    })
                    .style(md_style.clone())
                    .font_family(ui_font.clone())
                    .text_size(typo.body)
                    .code_block_actions(move |block, _, _| {
                        let code = block.code().to_string();
                        let lang = block.lang().filter(|l| !l.is_empty());
                        let span_start = block.span.as_ref().map(|s| s.start).unwrap_or(0);
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when_some(lang, |row, lang| {
                                row.child(
                                    div()
                                        .pl_1p5()
                                        .text_size(typo.xs)
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child(lang),
                                )
                            })
                            .child(ai_copy_button(
                                SharedString::from(format!(
                                    "ai-copy-code-{msg_idx}-{part_idx}-{span_start}"
                                )),
                                crate::i18n::ai::COPY_CODE.get(),
                                crate::i18n::common::COPIED.get(),
                                code,
                            ))
                    })
                    .table_actions(move |table, _, _| {
                        let md = table.markdown.clone();
                        let span_start = table.span.as_ref().map(|s| s.start).unwrap_or(0);
                        div().w_full().flex().justify_end().child(ai_copy_button(
                            SharedString::from(format!(
                                "ai-copy-table-{msg_idx}-{part_idx}-{span_start}"
                            )),
                            crate::i18n::ai::COPY_TABLE.get(),
                            crate::i18n::common::COPIED.get(),
                            md,
                        ))
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_assistant_run(
        &self,
        run_start: usize,
        run_end: usize,
        total_msgs: usize,
        md_style: &TextViewStyle,
        ui_font: &SharedString,
        palette: Palette,
        typo: AiTypography,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ps = ProcessStyle::from_theme(palette, typo);
        let (code_font, _) = crate::theme::tool_typography(&self.settings);

        let session = self.session.borrow();
        let mut all_parts: Vec<(usize, usize, &AssistantPart)> = Vec::new();
        let mut run_streaming = false;
        let mut errors: Vec<String> = Vec::new();
        let mut recorded_duration_ms: Option<u64> = None;

        for (m_idx, msg) in session.messages[run_start..run_end].iter().enumerate() {
            let abs_msg_idx = run_start + m_idx;
            if let ChatMessage::Assistant {
                parts,
                error,
                streaming,
                duration_ms,
            } = msg
            {
                if *streaming {
                    run_streaming = true;
                }
                if let Some(err) = error.as_ref().filter(|e| !e.is_empty()) {
                    errors.push(err.clone());
                }
                if let Some(ms) = *duration_ms {
                    recorded_duration_ms =
                        Some(recorded_duration_ms.unwrap_or(0).saturating_add(ms));
                }
                for (p_idx, part) in parts.iter().enumerate() {
                    all_parts.push((abs_msg_idx, p_idx, part));
                }
            }
        }

        let is_last_run = run_end == total_msgs;
        let any_tool_running = all_parts
            .iter()
            .any(|(_, _, p)| matches!(p, AssistantPart::Tool(b) if !b.done));
        let any_thinking_running = all_parts
            .iter()
            .any(|(_, _, p)| matches!(p, AssistantPart::Thinking { streaming, .. } if *streaming));
        let is_task_running = run_streaming
            || any_tool_running
            || any_thinking_running
            || (is_last_run && self.session.borrow().is_streaming);

        let run_has_error = !errors.is_empty()
            || all_parts
                .iter()
                .any(|(_, _, p)| matches!(p, AssistantPart::Tool(b) if b.is_error));

        // Only a trailing text block is an answer; tools/thinking after earlier prose must remain in the timeline.
        let final_text_pos = all_parts
            .last()
            .filter(|(_, _, p)| matches!(p, AssistantPart::Text(t) if !t.trim().is_empty()))
            .map(|_| all_parts.len() - 1);

        enum TimelineNode<'a> {
            Group(Vec<ProcessStep<'a>>),
            Approval(&'a ToolBlock),
            Prose {
                msg_idx: usize,
                part_idx: usize,
                text: &'a str,
            },
        }

        let split_at = final_text_pos.unwrap_or(all_parts.len());
        let process_slice = &all_parts[..split_at];
        let mut timeline: Vec<TimelineNode<'_>> = Vec::new();
        let mut pending_steps: Vec<ProcessStep<'_>> = Vec::new();
        let mut process_step_count = 0usize;
        let mut sum_tool_ms = 0u64;
        let mut sum_think_ms = 0u64;
        let mut tool_step_count = 0usize;
        let mut thinking_step_count = 0usize;

        for &(m_idx, p_idx, part) in process_slice {
            match part {
                AssistantPart::Thinking {
                    text,
                    duration_ms,
                    streaming,
                } => {
                    if text.trim().is_empty() && !*streaming {
                        continue;
                    }
                    process_step_count += 1;
                    thinking_step_count += 1;
                    if let Some(ms) = *duration_ms {
                        sum_think_ms = sum_think_ms.saturating_add(ms);
                    }
                    pending_steps.push(ProcessStep::Thinking {
                        msg_idx: m_idx,
                        part_idx: p_idx,
                        text,
                        duration_ms: *duration_ms,
                        streaming: *streaming,
                    });
                }
                AssistantPart::Tool(block) if block.approval.is_some() => {
                    if !pending_steps.is_empty() {
                        timeline.push(TimelineNode::Group(std::mem::take(&mut pending_steps)));
                    }
                    timeline.push(TimelineNode::Approval(block));
                }
                AssistantPart::Tool(block) => {
                    process_step_count += 1;
                    tool_step_count += 1;
                    if let Some(ms) = block.duration_ms {
                        sum_tool_ms = sum_tool_ms.saturating_add(ms);
                    }
                    pending_steps.push(ProcessStep::Tool {
                        msg_idx: m_idx,
                        part_idx: p_idx,
                        block,
                    });
                }
                AssistantPart::Text(text) => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    if !pending_steps.is_empty() {
                        timeline.push(TimelineNode::Group(std::mem::take(&mut pending_steps)));
                    }
                    timeline.push(TimelineNode::Prose {
                        msg_idx: m_idx,
                        part_idx: p_idx,
                        text,
                    });
                }
            }
        }
        if !pending_steps.is_empty() {
            timeline.push(TimelineNode::Group(pending_steps));
        }

        // If task finished cleanly and has intermediate steps before final text (or pure process),
        // allow collapsing the entire process area into one sleek single line ("用时 1m 58s").
        let has_process_area = timeline
            .iter()
            .any(|node| !matches!(node, TimelineNode::Approval(_)));
        let has_timeline = !timeline.is_empty();
        let show_tool_details =
            self.settings.tool_execution_view == ToolExecutionView::Detailed && tool_step_count > 0;
        let collapse_eligible = !is_task_running && !run_has_error && has_process_area;
        // A manual toggle reverses the current mode's expansion default.
        let task_expanded = !collapse_eligible
            || (show_tool_details != self.toggled_task_processes.contains(&run_start));

        let total_elapsed_ms = recorded_duration_ms.or_else(|| {
            if is_last_run && self.session.borrow().is_streaming {
                session
                    .active_run_start
                    .map(|t| t.elapsed().as_millis() as u64)
            } else {
                let summed = sum_tool_ms.saturating_add(sum_think_ms);
                (summed > 0).then_some(summed)
            }
        });

        let task_summary_label =
            if thinking_step_count == 1 && tool_step_count == 0 && timeline.len() == 1 {
                match total_elapsed_ms {
                    Some(ms) => crate::i18n::ai::worked_for(&format_thinking_duration(ms)),
                    None => crate::i18n::ai::WORK_PROCESS.get().to_owned(),
                }
            } else if let Some(ms) = total_elapsed_ms {
                crate::i18n::ai::elapsed_for(&format_duration_compact(ms))
            } else {
                crate::i18n::ai::completed_steps(process_step_count)
            };

        let mut rendered_process: Vec<AnyElement> = Vec::new();
        let mut applied_run: Vec<&ToolBlock> = Vec::new();
        let flush_applied = |this: &Self,
                             run: &mut Vec<&ToolBlock>,
                             out: &mut Vec<AnyElement>,
                             cx: &Context<Self>| {
            if run.is_empty() {
                return;
            }
            if run.len() == 1 {
                out.push(this.render_tag_approval(run[0], palette, typo, ui_font, cx));
            } else {
                out.push(this.render_applied_approval_group(run, palette, typo, ui_font, cx));
            }
            run.clear();
        };
        for node in timeline
            .into_iter()
            .filter(|node| task_expanded || matches!(node, TimelineNode::Approval(_)))
        {
            match node {
                TimelineNode::Approval(block)
                    if block.approval.as_ref().is_some_and(|a| {
                        matches!(a.state, ApprovalState::Applied | ApprovalState::Undone)
                    }) =>
                {
                    applied_run.push(block);
                }
                TimelineNode::Approval(block) => {
                    flush_applied(self, &mut applied_run, &mut rendered_process, cx);
                    rendered_process
                        .push(self.render_tag_approval(block, palette, typo, ui_font, cx));
                }
                TimelineNode::Group(steps) => {
                    flush_applied(self, &mut applied_run, &mut rendered_process, cx);
                    rendered_process.push(
                        self.render_process_group(&steps, ui_font, &code_font, palette, ps, cx),
                    );
                }
                TimelineNode::Prose {
                    msg_idx,
                    part_idx,
                    text,
                } => {
                    flush_applied(self, &mut applied_run, &mut rendered_process, cx);
                    rendered_process.push(
                        div()
                            .pt(px(2.))
                            .pb(px(4.))
                            .child(self.render_markdown_block(
                                SharedString::from(format!(
                                    "ai-assistant-mid-{msg_idx}-{part_idx}"
                                )),
                                text,
                                msg_idx,
                                part_idx,
                                md_style,
                                ui_font,
                                palette,
                                typo,
                                cx,
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
        flush_applied(self, &mut applied_run, &mut rendered_process, cx);

        let final_answer_el = final_text_pos.map(|pos| {
            let (m_idx, p_idx, part) = all_parts[pos];
            let text = match part {
                AssistantPart::Text(t) => t.as_str(),
                _ => "",
            };
            self.render_markdown_block(
                SharedString::from(format!("ai-assistant-md-{m_idx}-{p_idx}")),
                text,
                m_idx,
                p_idx,
                md_style,
                ui_font,
                palette,
                typo,
                cx,
            )
        });

        // The copy/export actions are hidden during streaming, so defer joining the full answer until settlement.
        let full_answer = (!is_task_running)
            .then(|| {
                all_parts
                    .iter()
                    .filter_map(|(_, _, p)| match p {
                        AssistantPart::Text(t) if !t.trim().is_empty() => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .when(has_timeline, |root| {
                root.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(ps.row_gap)
                        .when(collapse_eligible, |col| {
                            col.child(
                                render_process_header(
                                    SharedString::from(format!("ai-task-toggle-{run_start}")),
                                    None,
                                    task_summary_label,
                                    false,
                                    false,
                                    task_expanded,
                                    ps,
                                )
                                .on_click(cx.listener(
                                    move |pane, _, _, cx| {
                                        if !pane.toggled_task_processes.remove(&run_start) {
                                            pane.toggled_task_processes.insert(run_start);
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                        })
                        .children(rendered_process),
                )
            })
            .when_some(final_answer_el, |root, answer| {
                root.child(
                    div()
                        .when(has_timeline, |el| el.mt(ps.section_gap))
                        .child(answer),
                )
            })
            .when(!is_task_running && !full_answer.is_empty(), |root| {
                root.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_start()
                        .pt_1()
                        .child(ai_copy_button(
                            SharedString::from(format!("ai-copy-msg-{run_start}")),
                            crate::i18n::ai::COPY_ANSWER.get(),
                            crate::i18n::common::COPIED.get(),
                            full_answer.clone(),
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("ai-export-note-{run_start}")))
                                .size(AI_COPY_BUTTON_SIZE)
                                .rounded(AI_COPY_BUTTON_RADIUS)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(fg_alpha(palette.text, 0.55))
                                .hover(move |style| {
                                    style
                                        .bg(fg_alpha(palette.text, 0.06))
                                        .text_color(fg_alpha(palette.text, 0.85))
                                })
                                .child(
                                    crate::ui::icon(IconName::NotebookPen)
                                        .w(AI_COPY_ICON_SIZE)
                                        .h(AI_COPY_ICON_SIZE),
                                )
                                .tooltip(|window, cx| {
                                    Tooltip::new(crate::i18n::ai::EXPORT_AS_NOTE.get())
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    pane.export_answer(&full_answer, cx)
                                })),
                        ),
                )
            })
            .when(is_task_running && all_parts.is_empty(), |root| {
                root.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .py_1()
                        .min_h(px(24.))
                        .child(
                            crate::ui::icon(IconName::RefreshCw)
                                .w(ps.icon_size)
                                .h(ps.icon_size)
                                .text_color(ps.fg_secondary)
                                .with_animation(
                                    ("ai-stream-spin", run_start),
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    |icon, t| icon.rotate(gpui::radians(t * std::f32::consts::TAU)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(ps.header_size)
                                .text_color(ps.fg_secondary)
                                .with_animation(
                                    ("ai-stream-breathe", run_start),
                                    Animation::new(Duration::from_millis(1200)).repeat(),
                                    |el, t| {
                                        let a = 0.55
                                            + 0.45
                                                * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos());
                                        el.opacity(a)
                                    },
                                )
                                .child(crate::i18n::ai::THINKING_ELLIPSIS.get()),
                        ),
                )
            })
            .children(errors.into_iter().map(|err| {
                div()
                    .mt_2()
                    .px_2p5()
                    .py_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ps.error_fg)
                    .text_size(ps.detail_size)
                    .text_color(ps.error_fg)
                    .child(err)
            }))
            .into_any_element()
    }

    pub(super) fn render_process_group(
        &self,
        steps: &[ProcessStep<'_>],
        ui_font: &SharedString,
        code_font: &SharedString,
        palette: Palette,
        ps: ProcessStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(first) = steps.first() else {
            return div().into_any_element();
        };
        let (first_msg, first_part) = first.ids();
        let group_key = (first_msg, first_part);

        // Single standalone thinking step renders cleanly as one line without double nesting.
        if steps.len() == 1
            && let ProcessStep::Thinking {
                msg_idx,
                part_idx,
                text,
                duration_ms,
                streaming,
            } = first
        {
            let title = if *streaming {
                crate::i18n::ai::STILL_THINKING.get().to_owned()
            } else if let Some(ms) = *duration_ms {
                crate::i18n::ai::thought_for(&format_thinking_duration(ms))
            } else {
                crate::i18n::ai::THINKING_PROCESS.get().to_owned()
            };
            let msg_idx = *msg_idx;
            let part_idx = *part_idx;
            let running = *streaming;
            let expanded = running || self.expanded_thinking.contains(&(msg_idx, part_idx));
            return div()
                .flex()
                .flex_col()
                .gap(ps.row_gap)
                .child(
                    render_process_header(
                        SharedString::from(format!("ai-think-hdr-{msg_idx}-{part_idx}")),
                        Some(IconName::Brain),
                        title,
                        *streaming,
                        false,
                        expanded,
                        ps,
                    )
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if !running && !pane.expanded_thinking.remove(&(msg_idx, part_idx)) {
                            pane.expanded_thinking.insert((msg_idx, part_idx));
                        }
                        cx.notify();
                    })),
                )
                .when(expanded && !text.trim().is_empty(), |col| {
                    col.child(
                        div()
                            .ml(px(12.))
                            .pl_3()
                            .border_l_1()
                            .border_color(ps.guide_line)
                            .child(render_thinking_detail(
                                SharedString::from(format!("ai-think-body-{msg_idx}-{part_idx}")),
                                (*text).to_owned(),
                                *streaming,
                                ui_font,
                                ps,
                            )),
                    )
                })
                .into_any_element();
        }

        let any_running = steps.iter().any(|s| match s {
            ProcessStep::Thinking { streaming, .. } => *streaming,
            ProcessStep::Tool { block, .. } => !block.done,
        });
        let any_error = steps.iter().any(|s| match s {
            ProcessStep::Thinking { .. } => false,
            ProcessStep::Tool { block, .. } => block.is_error,
        });
        let group_expanded = !self.collapsed_tool_groups.contains(&group_key);
        let title = process_group_title(steps, any_running);
        let header_icon = process_group_icon(steps);

        div()
            .flex()
            .flex_col()
            .gap(ps.row_gap)
            .child(
                render_process_header(
                    SharedString::from(format!("ai-proc-grp-{first_msg}-{first_part}")),
                    Some(header_icon),
                    title,
                    any_running,
                    any_error,
                    group_expanded,
                    ps,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    if !pane.collapsed_tool_groups.remove(&group_key) {
                        pane.collapsed_tool_groups.insert(group_key);
                    }
                    cx.notify();
                })),
            )
            .when(group_expanded, |col| {
                col.child(
                    div()
                        .ml(px(12.))
                        .pl_3()
                        .border_l_1()
                        .border_color(ps.guide_line)
                        .flex()
                        .flex_col()
                        .gap(ps.row_gap)
                        .children(steps.iter().map(|step| match step {
                            ProcessStep::Thinking {
                                msg_idx,
                                part_idx,
                                text,
                                duration_ms,
                                streaming,
                            } => {
                                let msg_idx = *msg_idx;
                                let part_idx = *part_idx;
                                let running = *streaming;
                                let expanded = running
                                    || self.expanded_thinking.contains(&(msg_idx, part_idx));
                                let label = if *streaming {
                                    crate::i18n::ai::STILL_THINKING.get().to_owned()
                                } else if let Some(ms) = *duration_ms {
                                    crate::i18n::ai::thought_for(&format_thinking_duration(ms))
                                } else {
                                    crate::i18n::ai::THINKING_PROCESS.get().to_owned()
                                };
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(ps.row_gap)
                                    .child(
                                        render_process_header(
                                            SharedString::from(format!(
                                                "ai-think-sub-{msg_idx}-{part_idx}"
                                            )),
                                            Some(IconName::Brain),
                                            label,
                                            *streaming,
                                            false,
                                            expanded,
                                            ps,
                                        )
                                        .on_click(
                                            cx.listener(move |pane, _, _, cx| {
                                                if !running
                                                    && !pane
                                                        .expanded_thinking
                                                        .remove(&(msg_idx, part_idx))
                                                {
                                                    pane.expanded_thinking
                                                        .insert((msg_idx, part_idx));
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .when(expanded && !text.trim().is_empty(), |box_| {
                                        box_.child(render_thinking_detail(
                                            SharedString::from(format!(
                                                "ai-think-subbody-{msg_idx}-{part_idx}"
                                            )),
                                            (*text).to_owned(),
                                            *streaming,
                                            ui_font,
                                            ps,
                                        ))
                                    })
                                    .into_any_element()
                            }
                            ProcessStep::Tool {
                                msg_idx,
                                part_idx,
                                block,
                            } => self.render_tool_step_detail(
                                *msg_idx, *part_idx, block, ui_font, code_font, palette, ps, cx,
                            ),
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_tool_step_detail(
        &self,
        msg_idx: usize,
        part_idx: usize,
        block: &ToolBlock,
        ui_font: &SharedString,
        code_font: &SharedString,
        palette: Palette,
        ps: ProcessStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_file_edit = block.name == "edit" || block.name == "write";
        let is_shell = matches!(block.name.as_str(), "bash" | "powershell");
        let is_cmd = is_shell || block.name == "run_python";
        let mode = self.settings.tool_execution_view;
        let icon = if is_cmd {
            IconName::Terminal
        } else if is_file_edit {
            IconName::PencilLine
        } else if block.name == "read" {
            IconName::FileText
        } else {
            IconName::Wrench
        };
        let row_id = SharedString::from(format!("ai-cmd-row-{msg_idx}-{part_idx}"));
        if mode == ToolExecutionView::Minimal {
            let text = crate::i18n::ai::tool_status_label(&block.name, block.done, block.is_error);
            return render_command_detail_row(
                row_id,
                icon,
                text,
                block.done,
                block.is_error,
                None,
                ui_font,
                code_font,
                ps,
            )
            .into_any_element();
        }

        // The two payloads belong to the tool call, even when its summary is a file diff.
        let details = (mode == ToolExecutionView::Detailed
            && (!block.args.is_null() || !block.output.is_empty()))
        .then(|| {
            div().flex().flex_col().gap(ps.row_gap).children(
                [
                    (
                        "input",
                        crate::i18n::ai::INPUT_ARGS_LABEL.get(),
                        (!block.args.is_null()).then(|| {
                            serde_json::to_string_pretty(&block.args)
                                .expect("tool arguments are JSON")
                        }),
                        ps.fg_secondary,
                    ),
                    (
                        "output",
                        crate::i18n::ai::OUTPUT_LABEL.get(),
                        (!block.output.is_empty()).then(|| block.output.clone()),
                        if block.is_error {
                            ps.error_fg
                        } else {
                            ps.fg_secondary
                        },
                    ),
                ]
                .into_iter()
                .filter_map(|(kind, title, text, color)| {
                    let text = text?;
                    Some(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(ps.meta_size)
                                    .text_color(ps.fg_tertiary)
                                    .child(title),
                            )
                            .child(
                                div().on_scroll(|_, _, cx| cx.stop_propagation()).child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "tool-{kind}-{msg_idx}-{part_idx}"
                                        )))
                                        .p_2()
                                        .max_h(px(180.))
                                        .overflow_y_scroll()
                                        .rounded(px(6.))
                                        .bg(gpui_kit::rgb(palette.code_background))
                                        .font_family(code_font.clone())
                                        .text_size(ps.detail_size)
                                        .text_color(color)
                                        .child(text),
                                ),
                            ),
                    )
                }),
            )
        });

        if is_file_edit && !block.diffs.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap(ps.row_gap)
                .children(block.diffs.iter().enumerate().map(|(diff_idx, diff)| {
                    let target = self.session.borrow().resolve_tool_path(&diff.path);
                    let diff_key = format!("{msg_idx}:{part_idx}:{diff_idx}:{}", diff.path);
                    let unviewed = !self.viewed_diff_files.contains(&diff_key);
                    let is_created = block.name == "write" && diff.removed == 0 && diff.added > 0;
                    let row = render_file_edit_row(
                        SharedString::from(format!("ai-file-row-{msg_idx}-{part_idx}-{diff_idx}")),
                        diff,
                        is_created,
                        block.is_error,
                        unviewed,
                        code_font,
                        palette,
                        ps,
                        cx.listener({
                            let diff_key = diff_key.clone();
                            let target = target.clone();
                            move |pane, _, _, cx| {
                                pane.viewed_diff_files.insert(diff_key.clone());
                                cx.emit(AiPaneEvent::OpenFile(target.clone()));
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                        cx.listener({
                            let diff_key = diff_key.clone();
                            move |pane, _, _, cx| {
                                pane.viewed_diff_files.insert(diff_key.clone());
                                if !pane.expanded_diffs.remove(&(msg_idx, part_idx)) {
                                    pane.expanded_diffs.insert((msg_idx, part_idx));
                                }
                                cx.notify();
                            }
                        }),
                    );
                    div().min_w_0().flex().flex_col().child(row).when(
                        self.expanded_diffs.contains(&(msg_idx, part_idx))
                            && (!diff.lines.is_empty()
                                || diff
                                    .patch
                                    .as_ref()
                                    .is_some_and(|patch| !patch.line_numbers.is_empty())),
                        |el| {
                            el.child(
                                self.render_diff_card(
                                    format!("ai-diff-{msg_idx}-{part_idx}-{diff_idx}-{}", block.id)
                                        .into(),
                                    diff,
                                    code_font,
                                    palette,
                                ),
                            )
                        },
                    )
                }))
                .children(details)
                .into_any_element();
        }

        let text = if is_shell {
            let raw = block
                .args
                .get("command")
                .and_then(|v| v.as_str())
                .map(|c| c.lines().next().unwrap_or(c).trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| block.summary.clone());
            if block.is_error {
                format!("Command failed ({raw})")
            } else {
                raw
            }
        } else {
            block.summary.clone()
        };

        let detail_font = if is_shell { code_font } else { ui_font };
        let row = render_command_detail_row(
            row_id.clone(),
            icon,
            text,
            block.done,
            block.is_error,
            block.duration_ms,
            detail_font,
            code_font,
            ps,
        )
        .with_animation(
            (row_id, 2usize),
            Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).top(px(4.0 * (1.0 - t))),
        );

        div()
            .flex()
            .flex_col()
            .gap(ps.row_gap)
            .child(row)
            .children(details)
            .into_any_element()
    }

    pub(super) fn render_pending_ui(
        &self,
        pending: &PendingExtensionUi,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        let warn_bg = mix_color(palette.surface, palette.accent, 0.08);
        let card = || {
            div()
                .mx_3()
                .mb_2()
                .p_2p5()
                .rounded(px(8.))
                .border_1()
                .border_color(gpui_kit::rgb(palette.accent))
                .bg(warn_bg)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(typo.sm)
                        .font_semibold()
                        .text_color(gpui_kit::rgb(palette.text))
                        .child(crate::ui::icon(IconName::ShieldAlert).w(px(13.)).h(px(13.)))
                        .child(pending.title.clone()),
                )
        };

        let card = match &pending.kind {
            PendingUiKind::Confirm(content) => {
                return self.render_confirmation(pending, content, palette, typo, cx);
            }
            PendingUiKind::Select { options } => card().child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(options.iter().enumerate().map(|(idx, opt)| {
                        let choice = opt.clone();
                        Button::new(("ext-opt", idx))
                            .ghost()
                            .compact()
                            .label(opt.clone())
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                pane.respond_select(choice.clone(), cx);
                            }))
                    }))
                    .child(
                        Button::new("ext-select-cancel")
                            .ghost()
                            .compact()
                            .label(crate::i18n::common::CANCEL.get())
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.cancel_pending_ext(cx);
                            })),
                    ),
            ),
            PendingUiKind::TextInput {
                placeholder,
                multiline,
            } => {
                let is_multi = *multiline;
                card()
                    .when(!placeholder.is_empty() && is_multi, |c| {
                        c.child(
                            div()
                                .text_size(typo.sm)
                                .text_color(gpui_kit::rgb(palette.muted))
                                .child(placeholder.clone()),
                        )
                    })
                    .child(if is_multi {
                        Textarea::new(&self.ext_textarea)
                            .h(px(80.))
                            .into_any_element()
                    } else {
                        Input::new(&self.ext_input).into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1p5()
                            .child(
                                Button::new("ext-input-cancel")
                                    .ghost()
                                    .compact()
                                    .label(crate::i18n::common::CANCEL.get())
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.cancel_pending_ext(cx);
                                    })),
                            )
                            .child(
                                Button::new("ext-input-submit")
                                    .primary()
                                    .compact()
                                    .label(crate::i18n::common::SUBMIT.get())
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        pane.submit_pending_ext_text(is_multi, cx);
                                    })),
                            ),
                    )
            }
        };
        crate::ui::popover_enter(card, "ai-pending-ui-pop", crate::ui::PopoverDirection::Up)
            .into_any_element()
    }

    pub(super) fn render_context_ring(&self, palette: Palette) -> AnyElement {
        let pct = self
            .session
            .borrow()
            .context_usage
            .as_ref()
            .and_then(|cu| cu.percent)
            .unwrap_or(0)
            .min(100);
        let tip = match &self.session.borrow().context_usage {
            Some(cu) => {
                let used = cu.tokens.unwrap_or(0) / 1000;
                let total = cu.context_window / 1000;
                let rem = 100u64.saturating_sub(pct);
                crate::i18n::ai::context_window_stats(pct, rem, used as usize, total as usize)
            }
            None => crate::i18n::ai::CONTEXT_WINDOW_EMPTY.get().to_owned(),
        };

        let track_color: Rgba = gpui_kit::rgb(palette.border);
        let arc_color: Rgba = if pct >= 85 {
            gpui_kit::rgb(palette.danger)
        } else {
            gpui_kit::rgb(palette.accent)
        };
        let fraction = match &self.session.borrow().context_usage {
            Some(cu) if cu.context_window > 0 => cu
                .tokens
                .map(|t| (t as f32 / cu.context_window as f32).clamp(0.0, 1.0))
                .unwrap_or_else(|| (pct as f32 / 100.0).clamp(0.0, 1.0)),
            _ => (pct as f32 / 100.0).clamp(0.0, 1.0),
        };

        div()
            .id("ai-context-ring")
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        let cx_pos = bounds.center().x;
                        let cy_pos = bounds.center().y;
                        let r = (bounds.size.width.min(bounds.size.height) * 0.5) - px(2.5);
                        if r <= px(1.) {
                            return;
                        }
                        let segments = 28usize;
                        let mut track = gpui_kit::PathBuilder::stroke(px(1.8));
                        for i in 0..=segments {
                            let angle = -std::f32::consts::FRAC_PI_2
                                + (i as f32 / segments as f32) * std::f32::consts::TAU;
                            let pt = point(cx_pos + r * angle.cos(), cy_pos + r * angle.sin());
                            if i == 0 {
                                track.move_to(pt);
                            } else {
                                track.line_to(pt);
                            }
                        }
                        if let Ok(path) = track.build() {
                            window.paint_path(path, track_color);
                        }

                        if fraction > 0.0 {
                            let active_steps = ((segments as f32) * fraction).ceil() as usize;
                            let mut arc = gpui_kit::PathBuilder::stroke(px(1.8));
                            for i in 0..=active_steps.max(1) {
                                let t = (i as f32 / segments as f32).min(fraction);
                                let angle =
                                    -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
                                let pt = point(cx_pos + r * angle.cos(), cy_pos + r * angle.sin());
                                if i == 0 {
                                    arc.move_to(pt);
                                } else {
                                    arc.line_to(pt);
                                }
                            }
                            if let Ok(path) = arc.build() {
                                window.paint_path(path, arc_color);
                            }
                        }
                    },
                )
                .size(px(16.)),
            )
            .into_any_element()
    }

    pub(super) fn render_slash_popover(
        &self,
        items: Vec<SlashItem>,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = items.len();
        let active_idx = self.slash_index.min(count.saturating_sub(1));
        let typo = AiTypography::from_settings(&self.settings);
        let sel_bg = mix_color(
            palette.hover,
            palette.accent,
            if palette.is_dark() { 0.14 } else { 0.08 },
        );
        div()
            .id("ai-slash-popover")
            .track_scroll(&self.slash_scroll)
            .w_full()
            .max_h(px(268.))
            .overflow_y_scroll()
            .rounded(px(12.))
            .border_1()
            .border_color(subtle_border(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font)
            .p_1p5()
            .flex()
            .flex_col()
            .gap_0p5()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                let selected = idx == active_idx;
                let item_click = item.clone();
                let icon_box_bg = if selected {
                    mix_color(
                        palette.surface,
                        palette.accent,
                        if palette.is_dark() { 0.22 } else { 0.12 },
                    )
                } else {
                    gpui_kit::rgb(palette.background)
                };
                div()
                    .id(("ai-slash-item", idx))
                    .px_2()
                    .py_1p5()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .when(selected, |row| row.bg(sel_bg))
                    .when(!selected, |row| {
                        row.hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                    })
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.execute_slash_item(item_click.clone(), window, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .size(px(22.))
                                    .rounded(px(6.))
                                    .flex_shrink_0()
                                    .bg(icon_box_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        crate::ui::icon(item.icon)
                                            .w(px(12.))
                                            .h(px(12.))
                                            .text_color(gpui_kit::rgb(if selected {
                                                palette.accent
                                            } else {
                                                palette.muted
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(typo.sm)
                                    .font_medium()
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .flex_shrink_0()
                                    .child(item.title),
                            )
                            .child(
                                div()
                                    .text_size(typo.sm)
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .text_ellipsis()
                                    .child(item.subtitle),
                            ),
                    )
            }))
            .with_animation(
                ("ai-slash-pop", self.slash_anim_gen),
                Animation::new(Duration::from_millis(145)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mb(px((1. - t) * -6.)),
            )
            .into_any_element()
    }

    pub(super) fn render_model_popover(
        &self,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        let search_q = self
            .model_search_input
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        let session = self.session.borrow();
        let current_id = session.current_model.as_ref().map(|m| m.id.as_str());
        let supports_thinking = session.current_model.as_ref().is_none_or(|m| m.reasoning);

        let mut models: Vec<(bool, ModelInfo)> = self
            .session
            .borrow()
            .available_models
            .iter()
            .filter(|m| {
                search_q.is_empty()
                    || m.name.to_ascii_lowercase().contains(&search_q)
                    || m.id.to_ascii_lowercase().contains(&search_q)
                    || m.provider.to_ascii_lowercase().contains(&search_q)
            })
            .cloned()
            .map(|m| {
                let key = format!("{}/{}", m.provider, m.id);
                let fav = self.favorite_models.contains(&key);
                (fav, m)
            })
            .collect();
        models.sort_by_key(|(fav, _)| !*fav);

        let sel_bg = mix_color(
            palette.hover,
            palette.accent,
            if palette.is_dark() { 0.16 } else { 0.09 },
        );

        div()
            .id("ai-model-popover")
            .w_full()
            .h(px(292.))
            .rounded(px(12.))
            .border_1()
            .border_color(subtle_border(palette.border))
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font.clone())
            .overflow_hidden()
            .flex()
            .flex_col()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .border_b_1()
                    .border_color(subtle_border(palette.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .border_r_1()
                            .border_color(subtle_border(palette.border))
                            .px_2p5()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                crate::ui::icon(IconName::Search)
                                    .w(px(12.))
                                    .h(px(12.))
                                    .text_color(gpui_kit::rgb(palette.muted)),
                            )
                            .child(
                                div().flex_1().min_w_0().text_size(typo.sm).child(
                                    Input::new(&self.model_search_input)
                                        .xsmall()
                                        .h(px(20.))
                                        .appearance(false)
                                        .bordered(false)
                                        .font_family(ui_font.clone())
                                        .text_size(typo.sm),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(136.))
                            .flex_shrink_0()
                            .px_2p5()
                            .py_1p5()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .text_size(typo.xs)
                                    .font_semibold()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child("Effort"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .border_r_1()
                            .border_color(subtle_border(palette.border))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id("ai-model-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p_1p5()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .when(models.is_empty(), |list| {
                                        list.child(
                                            div()
                                                .px_2p5()
                                                .py_4()
                                                .text_size(typo.sm)
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .child(
                                                    if self
                                                        .session
                                                        .borrow()
                                                        .available_models
                                                        .is_empty()
                                                    {
                                                        crate::i18n::ai::NO_MODELS_AVAILABLE.get()
                                                    } else {
                                                        crate::i18n::ai::NO_MATCHING_MODELS.get()
                                                    },
                                                ),
                                        )
                                    })
                                    .children(models.into_iter().enumerate().map(
                                        |(idx, (fav, m))| {
                                            let selected = current_id == Some(m.id.as_str());
                                            let provider = m.provider.clone();
                                            let model_id = m.id.clone();
                                            let fav_key = format!("{}/{}", m.provider, m.id);
                                            div()
                                                .id(("ai-model-row", idx))
                                                .px_2()
                                                .py_1p5()
                                                .rounded(px(7.))
                                                .cursor_pointer()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap_1p5()
                                                .when(selected, |r| r.bg(sel_bg))
                                                .when(!selected, |r| {
                                                    r.hover(move |s| {
                                                        s.bg(gpui_kit::rgb(palette.hover))
                                                    })
                                                })
                                                .on_click(cx.listener(move |pane, _, _, cx| {
                                                    if let Some(client) =
                                                        &pane.session.borrow().client
                                                    {
                                                        let _ =
                                                            client.set_model(&provider, &model_id);
                                                    }
                                                    cx.notify();
                                                }))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .child(render_model_icon(
                                                            Some(&m),
                                                            px(13.),
                                                            if selected {
                                                                palette.accent
                                                            } else {
                                                                palette.muted
                                                            },
                                                        ))
                                                        .child(
                                                            div()
                                                                .text_size(typo.sm)
                                                                .font_medium()
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.text,
                                                                ))
                                                                .text_ellipsis()
                                                                .child(m.name),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .flex_shrink_0()
                                                        .when(selected, |r| {
                                                            r.child(
                                                                crate::ui::icon(IconName::Check)
                                                                    .w(px(12.))
                                                                    .h(px(12.))
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.accent,
                                                                    )),
                                                            )
                                                        })
                                                        .child(
                                                            div()
                                                                .id(("ai-model-fav", idx))
                                                                .size(px(20.))
                                                                .rounded(px(5.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_color(gpui_kit::rgb(if fav {
                                                                    palette.accent
                                                                } else {
                                                                    palette.muted
                                                                }))
                                                                .opacity(if fav {
                                                                    1.0
                                                                } else {
                                                                    0.45
                                                                })
                                                                .hover(move |s| {
                                                                    s.opacity(1.0).bg(
                                                                        gpui_kit::rgb(
                                                                            palette.pressed,
                                                                        ),
                                                                    )
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    |_, _, cx| {
                                                                        cx.stop_propagation();
                                                                    },
                                                                )
                                                                .on_click(cx.listener(
                                                                    move |pane, _, _, cx| {
                                                                        pane.toggle_favorite_model(
                                                                            &fav_key, cx,
                                                                        );
                                                                        cx.stop_propagation();
                                                                    },
                                                                ))
                                                                .child(
                                                                    crate::ui::icon(IconName::Star)
                                                                        .w(px(11.))
                                                                        .h(px(11.)),
                                                                ),
                                                        ),
                                                )
                                        },
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .w(px(136.))
                            .flex_shrink_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id("ai-think-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p_1p5()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .when(!supports_thinking, |col| {
                                        col.child(
                                            div()
                                                .px_2()
                                                .py_2()
                                                .text_size(typo.xs)
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .child(
                                                    crate::i18n::ai::MODEL_NO_THINKING_LEVEL.get(),
                                                ),
                                        )
                                    })
                                    .when(supports_thinking, |col| {
                                        col.children(
                                            self.session
                                                .borrow()
                                                .thinking_levels
                                                .iter()
                                                .enumerate()
                                                .map(|(idx, lvl)| {
                                                    let checked =
                                                        self.session.borrow().current_thinking
                                                            == *lvl;
                                                    let lvl_set = lvl.clone();
                                                    div()
                                                        .id(("ai-think-row", idx))
                                                        .px_2()
                                                        .py_1p5()
                                                        .rounded(px(7.))
                                                        .cursor_pointer()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap_1p5()
                                                        .when(checked, |r| r.bg(sel_bg))
                                                        .when(!checked, |r| {
                                                            r.hover(move |s| {
                                                                s.bg(gpui_kit::rgb(palette.hover))
                                                            })
                                                        })
                                                        .on_click(cx.listener(
                                                            move |pane, _, _, cx| {
                                                                pane.session
                                                                    .borrow_mut()
                                                                    .set_thinking(&lvl_set);
                                                                pane.model_popover_open = false;
                                                                cx.notify();
                                                            },
                                                        ))
                                                        .child(
                                                            div()
                                                                .text_size(typo.sm)
                                                                .font_medium()
                                                                .text_color(gpui_kit::rgb(
                                                                    palette.text,
                                                                ))
                                                                .child(thinking_short_label(lvl)),
                                                        )
                                                        .when(checked, |r| {
                                                            r.child(
                                                                crate::ui::icon(IconName::Check)
                                                                    .w(px(12.))
                                                                    .h(px(12.))
                                                                    .text_color(gpui_kit::rgb(
                                                                        palette.accent,
                                                                    )),
                                                            )
                                                        })
                                                }),
                                        )
                                    }),
                            ),
                    ),
            )
            .with_animation(
                ("ai-model-pop", self.model_popover_anim_gen),
                Animation::new(Duration::from_millis(145)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mb(px((1. - t) * -6.)),
            )
            .into_any_element()
    }

    pub(super) fn render_toast_popover(
        &self,
        msg: String,
        is_err: bool,
        palette: Palette,
        ui_font: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typo = AiTypography::from_settings(&self.settings);
        div()
            .id("ai-toast-popover")
            .w_full()
            .px_3()
            .py_2()
            .rounded(px(10.))
            .border_1()
            .border_color(if is_err {
                gpui_kit::rgb(palette.danger)
            } else {
                subtle_border(palette.border)
            })
            .bg(gpui_kit::rgb(palette.surface))
            .shadow(popover_shadow(palette.is_dark()))
            .font_family(ui_font)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .text_size(typo.sm)
            .text_color(gpui_kit::rgb(if is_err {
                palette.danger
            } else {
                palette.text
            }))
            .child(div().min_w_0().text_ellipsis().child(msg))
            .child(
                div()
                    .id("ai-toast-close")
                    .size(px(18.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(gpui_kit::rgb(palette.muted))
                    .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.toast = None;
                        cx.notify();
                    }))
                    .child(crate::ui::icon(IconName::Close).w(px(11.)).h(px(11.))),
            )
            .with_animation(
                ("ai-toast-pop", self.toast_anim_gen),
                Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mb(px((1. - t) * -6.)),
            )
            .into_any_element()
    }

    pub(super) fn render_welcome(
        &self,
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title_el = if let Some(name) = self
            .project_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            div()
                .mt(AI_WELCOME_ICON_TITLE_GAP)
                .max_w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(22.))
                .font_medium()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(fg_alpha(palette.text, 0.72))
                        .child(crate::i18n::ai::HERO_PREFIX.get()),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .font_semibold()
                        .text_color(gpui_kit::rgb(palette.text))
                        .child(name.to_owned()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(fg_alpha(palette.text, 0.72))
                        .child(crate::i18n::ai::HERO_SUFFIX.get()),
                )
                .into_any_element()
        } else {
            div()
                .mt(AI_WELCOME_ICON_TITLE_GAP)
                .max_w_full()
                .text_center()
                .text_size(px(22.))
                .font_medium()
                .text_color(gpui_kit::rgb(palette.text))
                .child(crate::i18n::ai::HERO_EMPTY.get())
                .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px_3()
            .flex()
            .flex_col()
            .items_center()
            .child(div().flex_grow(2.0).flex_basis(px(0.)))
            .child(
                div()
                    .w_full()
                    .max_w(AI_COMPOSER_MAX_WIDTH)
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        img("brand/ai-mascot.png")
                            .size(AI_WELCOME_ICON_SIZE)
                            .object_fit(ObjectFit::Contain)
                            .flex_shrink_0(),
                    )
                    .child(title_el)
                    .child(
                        div()
                            .w_full()
                            .mt(px(22.))
                            .child(self.render_composer(palette, typo, ui_font, window, cx)),
                    ),
            )
            .child(div().flex_grow(3.0).flex_basis(px(0.)))
            .into_any_element()
    }

    pub(super) fn render_composer(
        &self,
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = self.session.borrow();
        let model_label = session
            .current_model
            .as_ref()
            .map(|m| short_model_name(&m.name))
            .unwrap_or_else(|| crate::i18n::ai::MODEL_FALLBACK.get().to_owned());
        let selected_model = session.current_model.as_ref();
        let supports_thinking = self
            .session
            .borrow()
            .current_model
            .as_ref()
            .is_none_or(|m| m.reasoning);
        let attached_chip = self.effective_attached_file().cloned();
        let has_chips = attached_chip.is_some() || !self.pasted_images.is_empty();
        let slash_items = self.current_slash_items(cx);
        let has_slash_items = !slash_items.is_empty();
        let input_empty = self.input.read(cx).value().is_empty();
        let can_send = !self.session.borrow().is_rewinding
            && (!self.input.read(cx).value().trim().is_empty()
                || !self.pasted_images.is_empty());
        let pane_paste = cx.entity();

        let subtle_btn = |cx: &App| {
            ButtonCustomVariant::new(cx)
                .foreground(gpui_kit::rgb(palette.muted).into())
                .hover(gpui_kit::rgb(palette.hover).into())
                .active(gpui_kit::rgb(palette.pressed).into())
        };
        let skill_token_bg = mix_color(
            palette.surface,
            palette.accent,
            if palette.is_dark() { 0.22 } else { 0.11 },
        );
        let skill_token_border = mix_color(
            palette.border,
            palette.accent,
            if palette.is_dark() { 0.45 } else { 0.35 },
        );
        div()
            .relative()
            .w_full()
            .capture_action::<MoveUp>(cx.listener(|pane, _: &MoveUp, _, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let count = items.len();
                    pane.slash_index = (pane.slash_index.min(count - 1) + count - 1) % count;
                    pane.slash_scroll.scroll_to_item(pane.slash_index);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action::<MoveDown>(cx.listener(|pane, _: &MoveDown, _, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let count = items.len();
                    pane.slash_index = (pane.slash_index.min(count - 1) + 1) % count;
                    pane.slash_scroll.scroll_to_item(pane.slash_index);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action::<IndentInline>(cx.listener(|pane, _: &IndentInline, window, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let chosen = items[pane.slash_index.min(items.len() - 1)].clone();
                    cx.stop_propagation();
                    pane.execute_slash_item(chosen, window, cx);
                }
            }))
            .capture_action::<Enter>(cx.listener(|pane, ev: &Enter, window, cx| {
                if ev.shift || ev.secondary {
                    return;
                }
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    let chosen = items[pane.slash_index.min(items.len() - 1)].clone();
                    cx.stop_propagation();
                    pane.execute_slash_item(chosen, window, cx);
                }
            }))
            .capture_action::<Escape>(cx.listener(|pane, _: &Escape, _, cx| {
                let items = pane.current_slash_items(cx);
                if !items.is_empty() {
                    pane.dismissed_slash_text = Some(pane.input.read(cx).value().to_string());
                    pane.slash_was_open = false;
                    cx.stop_propagation();
                    cx.notify();
                } else if pane.model_popover_open {
                    pane.model_popover_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .when(
                has_slash_items || self.model_popover_open || self.toast.is_some(),
                |wrap| {
                    let toast_item = self.toast.clone();
                    wrap.child(
                        deferred(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_full()
                                .mb_2()
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .when_some(toast_item, |col, (msg, is_err)| {
                                    col.child(self.render_toast_popover(
                                        msg,
                                        is_err,
                                        palette,
                                        ui_font.clone(),
                                        cx,
                                    ))
                                })
                                .when(self.model_popover_open && !has_slash_items, |col| {
                                    col.child(self.render_model_popover(
                                        palette,
                                        ui_font.clone(),
                                        cx,
                                    ))
                                })
                                .when(has_slash_items, |col| {
                                    col.child(self.render_slash_popover(
                                        slash_items,
                                        palette,
                                        ui_font.clone(),
                                        cx,
                                    ))
                                }),
                        )
                        .with_priority(3),
                    )
                },
            )
            .child(
                confirmation::conversation_surface(palette)
                    .px_3()
                    .pt_2()
                    .pb_1p5()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(has_chips, |card| {
                        card.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_1()
                                .when_some(attached_chip, |row, file| {
                                    let open_path = file.path.clone();
                                    row.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .px_2()
                                            .py(px(2.))
                                            .rounded(px(6.))
                                            .bg(gpui_kit::rgb(palette.background))
                                            .border_1()
                                            .border_color(subtle_border(palette.border))
                                            .text_size(typo.sm)
                                            .child(
                                                div()
                                                    .id("ai-attached-file")
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |_, _, _, cx| {
                                                        cx.emit(AiPaneEvent::OpenFile(
                                                            open_path.clone(),
                                                        ));
                                                    }))
                                                    .child(
                                                        crate::ui::icon(IconName::FileText)
                                                            .w(px(11.))
                                                            .h(px(11.))
                                                            .text_color(gpui_kit::rgb(
                                                                palette.accent,
                                                            )),
                                                    )
                                                    .child(format!("@{}", file.display)),
                                            )
                                            .child(
                                                div()
                                                    .id("ai-detach-file")
                                                    .cursor_pointer()
                                                    .text_color(gpui_kit::rgb(palette.muted))
                                                    .on_click(cx.listener(|pane, _, _, cx| {
                                                        pane.attach_active_file = false;
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        crate::ui::icon(IconName::Close)
                                                            .w(px(11.))
                                                            .h(px(11.)),
                                                    ),
                                            ),
                                    )
                                })
                                .children(self.pasted_images.iter().map(|p| {
                                    let rm_id = p.id;
                                    let preview = p.image.clone();
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .pl_1()
                                        .pr_1p5()
                                        .py(px(2.))
                                        .rounded(px(7.))
                                        .bg(gpui_kit::rgb(palette.background))
                                        .border_1()
                                        .border_color(subtle_border(palette.border))
                                        .text_size(typo.xs)
                                        .child(
                                            div()
                                                .id(("ai-preview-draft-img", rm_id as usize))
                                                .flex()
                                                .items_center()
                                                .gap_1p5()
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |pane, _, window, cx| {
                                                    pane.open_overlay(preview.clone(), window, cx);
                                                }))
                                                .child(
                                                    img(p.image.clone())
                                                        .size(px(24.))
                                                        .rounded(px(5.))
                                                        .object_fit(ObjectFit::Cover),
                                                )
                                                .child(
                                                    div()
                                                        .max_w(px(110.))
                                                        .text_ellipsis()
                                                        .text_color(gpui_kit::rgb(palette.muted))
                                                        .child(p.label.clone()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(("ai-rm-img", rm_id as usize))
                                                .cursor_pointer()
                                                .text_color(gpui_kit::rgb(palette.muted))
                                                .on_click(cx.listener(move |pane, _, _, cx| {
                                                    pane.pasted_images.retain(|im| im.id != rm_id);
                                                    cx.notify();
                                                }))
                                                .child(
                                                    crate::ui::icon(IconName::Close)
                                                        .w(px(11.))
                                                        .h(px(11.)),
                                                ),
                                        )
                                })),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .child(
                                Textarea::new(&self.input)
                                    .appearance(false)
                                    .bordered(false)
                                    .aria_label(crate::i18n::ai::INPUT_PLACEHOLDER.get())
                                    .font_family(ui_font.clone())
                                    .text_size(typo.body)
                                    .on_paste(move |item, window, cx| {
                                        pane_paste.update(cx, |pane, cx| {
                                            pane.handle_paste(item, window, cx)
                                        })
                                    })
                                    .token(move |ctx, _, _| {
                                        let id = ctx.token().id();
                                        if let Some(quote) = SelectionQuote::from_token(ctx.token()) {
                                            inline_quote_token(ctx, quote, palette, typo.body)
                                        } else if id.starts_with("skill:") {
                                            InputToken::new(ctx)
                                                .icon(IconName::Sparkles)
                                                .bg(skill_token_bg)
                                                .border_color(skill_token_border)
                                                .text_color(gpui_kit::rgb(palette.accent))
                                                .into_any_element()
                                        } else if id.starts_with("dir:") {
                                            InputToken::new(ctx)
                                                .icon(IconName::FolderOpen)
                                                .bg(gpui_kit::rgb(palette.background))
                                                .border_color(subtle_border(palette.border))
                                                .text_color(gpui_kit::rgb(palette.text))
                                                .into_any_element()
                                        } else {
                                            InputToken::new(ctx)
                                                .icon(IconName::FileText)
                                                .bg(gpui_kit::rgb(palette.background))
                                                .border_color(subtle_border(palette.border))
                                                .text_color(gpui_kit::rgb(palette.text))
                                                .into_any_element()
                                        }
                                    })
                                    .on_token_click(cx.listener(
                                        |pane,
                                         ev: &gpui_kit::component::input::InlineTokenClickEvent,
                                         _,
                                         cx| {
                                            if let Some(rel) = ev.token().id().strip_prefix("file:")
                                            {
                                                let target =
                                                    pane.session.borrow().resolve_tool_path(rel);
                                                cx.emit(AiPaneEvent::OpenFile(target));
                                            }
                                        },
                                    )),
                            )
                            .when(input_empty, |box_wrap| {
                                box_wrap.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .px(px(10.))
                                        .py(px(8.))
                                        .font_family(ui_font.clone())
                                        .text_size(typo.body)
                                        .line_height(rems(1.25))
                                        .text_color(fg_alpha(palette.text, 0.40))
                                        .child(crate::i18n::ai::INPUT_PLACEHOLDER.get()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Button::new("ai-pick-files")
                                            .custom(subtle_btn(cx))
                                            .compact()
                                            .w(px(26.))
                                            .h(px(26.))
                                            .icon(crate::ui::icon(IconName::Plus))
                                            .tooltip(crate::i18n::ai::TOOLTIP_ATTACH_FILE.get())
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.pick_files(window, cx);
                                            })),
                                    )
                                    .child({
                                        let has_file = self.active_file.is_some();
                                        let active_ctx = has_file && self.attach_active_file;
                                        let ctx_tip = match &self.active_file {
                                            Some(f) if active_ctx => {
                                                crate::i18n::ai::attached_file_tooltip(&f.display)
                                            }
                                            Some(f) => {
                                                crate::i18n::ai::attach_current_file_tooltip(
                                                    &f.display,
                                                )
                                            }
                                            None => crate::i18n::ai::NO_OPEN_FILE.get().to_owned(),
                                        };
                                        let ctx_bg = if active_ctx {
                                            mix_color(
                                                palette.surface,
                                                palette.accent,
                                                if palette.is_dark() { 0.22 } else { 0.12 },
                                            )
                                        } else {
                                            gpui_kit::rgb(palette.surface).into()
                                        };
                                        div()
                                            .id("ai-toggle-active-context")
                                            .size(px(26.))
                                            .rounded(px(7.))
                                            .bg(ctx_bg)
                                            .when(active_ctx, |el| {
                                                el.border_1().border_color(mix_color(
                                                    palette.border,
                                                    palette.accent,
                                                    0.45,
                                                ))
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(has_file, |el| {
                                                el.cursor_pointer().hover(move |s| {
                                                    s.bg(gpui_kit::rgb(palette.hover))
                                                })
                                            })
                                            .when(!has_file, |el| el.opacity(0.42))
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(ctx_tip.clone()).build(window, cx)
                                            })
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                if pane.active_file.is_some() {
                                                    pane.attach_active_file =
                                                        !pane.attach_active_file;
                                                    cx.notify();
                                                }
                                            }))
                                            .child(
                                                crate::ui::icon(IconName::Paperclip)
                                                    .w(px(13.))
                                                    .h(px(13.))
                                                    .text_color(gpui_kit::rgb(if active_ctx {
                                                        palette.accent
                                                    } else {
                                                        palette.muted
                                                    })),
                                            )
                                    })
                                    .child(
                                        div()
                                            .id("ai-perm-toggle")
                                            .px_2()
                                            .h(px(24.))
                                            .rounded(px(6.))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(typo.sm)
                                            .text_color(gpui_kit::rgb(
                                                if self.session.borrow().auto_approve_shell {
                                                    palette.danger
                                                } else {
                                                    palette.muted
                                                },
                                            ))
                                            .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                            .tooltip(|window, cx| {
                                                Tooltip::new(
                                                    crate::i18n::ai::TOOLTIP_PERM_MODE.get(),
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                let mut session = pane.session.borrow_mut();
                                                session.auto_approve_shell =
                                                    !session.auto_approve_shell;
                                                cx.notify();
                                            }))
                                            .child(
                                                crate::ui::icon(
                                                    if self.session.borrow().auto_approve_shell {
                                                        IconName::ShieldAlert
                                                    } else {
                                                        IconName::Shield
                                                    },
                                                )
                                                .w(px(13.))
                                                .h(px(13.)),
                                            )
                                            .child(if self.session.borrow().auto_approve_shell {
                                                crate::i18n::ai::PERM_FULL_ACCESS.get()
                                            } else {
                                                crate::i18n::ai::PERM_ASK.get()
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .min_w_0()
                                    .child(self.render_context_ring(palette))
                                    .child({
                                        let open = self.model_popover_open;
                                        let hover_bg = fg_alpha(palette.text, 0.06);
                                        div()
                                            .id("ai-model-effort-pill")
                                            .group("ai-model-effort-pill")
                                            .h(px(26.))
                                            .px_2()
                                            .rounded(px(8.))
                                            .when(open, |el| el.bg(hover_bg))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .cursor_pointer()
                                            .text_size(typo.sm)
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .hover(move |s| s.bg(hover_bg))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.model_popover_open = !pane.model_popover_open;
                                                if pane.model_popover_open {
                                                    pane.model_popover_anim_gen =
                                                        pane.model_popover_anim_gen.wrapping_add(1);
                                                    pane.model_search_input.update(
                                                        cx,
                                                        |input, cx| {
                                                            input.set_value(
                                                                String::new(),
                                                                window,
                                                                cx,
                                                            );
                                                            input.focus(window, cx);
                                                        },
                                                    );
                                                }
                                                cx.notify();
                                            }))
                                            .child(render_model_icon(
                                                selected_model,
                                                px(13.),
                                                palette.muted,
                                            ))
                                            .child(
                                                div()
                                                    .max_w(px(140.))
                                                    .text_ellipsis()
                                                    .font_medium()
                                                    .text_color(gpui_kit::rgb(palette.text))
                                                    .child(model_label),
                                            )
                                            .when(supports_thinking, |btn| {
                                                btn.child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .text_color(fg_alpha(palette.text, 0.40))
                                                        .child(thinking_short_label(
                                                            &self.session.borrow().current_thinking,
                                                        )),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .flex_shrink_0()
                                                    .text_color(fg_alpha(
                                                        palette.text,
                                                        if open { 0.85 } else { 0.38 },
                                                    ))
                                                    .group_hover("ai-model-effort-pill", move |s| {
                                                        s.text_color(fg_alpha(palette.text, 0.85))
                                                    })
                                                    .child(
                                                        crate::ui::icon(if open {
                                                            IconName::ChevronUp
                                                        } else {
                                                            IconName::ChevronDown
                                                        })
                                                        .w(px(12.))
                                                        .h(px(12.)),
                                                    ),
                                            )
                                    })
                                    .child(if self.session.borrow().is_streaming {
                                        div()
                                            .id("ai-abort")
                                            .size(px(28.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(palette.danger))
                                            .text_color(gpui_kit::rgb(palette.on_accent))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor_pointer()
                                            .on_click(cx.listener(|pane, _, _, cx| {
                                                pane.abort(cx);
                                            }))
                                            .child(
                                                crate::ui::icon(IconName::Square)
                                                    .w(px(11.))
                                                    .h(px(11.)),
                                            )
                                            .into_any_element()
                                    } else {
                                        div()
                                            .id("ai-send")
                                            .size(px(28.))
                                            .rounded_full()
                                            .bg(gpui_kit::rgb(if can_send {
                                                palette.accent
                                            } else {
                                                palette.hover
                                            }))
                                            .text_color(gpui_kit::rgb(if can_send {
                                                palette.on_accent
                                            } else {
                                                palette.muted
                                            }))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(can_send, |btn| btn.cursor_pointer())
                                            .on_click(cx.listener(|pane, _, window, cx| {
                                                pane.send_prompt(window, cx);
                                            }))
                                            .child(
                                                crate::ui::icon(IconName::ArrowUp)
                                                    .w(px(14.))
                                                    .h(px(14.)),
                                            )
                                            .into_any_element()
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}
