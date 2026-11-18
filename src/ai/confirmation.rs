//! Inline approval receipts and the shared surface used by AI confirmations.
use super::*;
use crate::tags::agent::{FileTagDelta, TagConfirmation};
use std::time::Instant;

#[derive(Clone, Debug)]
pub(super) enum ConfirmationContent {
    Message(String),
    Command(String),
    Tags {
        call_id: Option<String>,
        preview: TagConfirmation,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApprovalState {
    Pending,
    Applying,
    Applied,
    Declined,
    Expired,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
pub(super) struct TagApproval {
    pub(super) preview: TagConfirmation,
    pub(super) state: ApprovalState,
    pub(super) expanded: bool,
    pub(super) settled_at: Option<Instant>,
}

impl TagApproval {
    pub(super) fn pending(preview: TagConfirmation) -> Self {
        Self {
            preview,
            state: ApprovalState::Pending,
            expanded: false,
            settled_at: None,
        }
    }

    pub(super) fn from_result(details: &Value) -> Option<Self> {
        let preview = serde_json::from_value(details.get("tagApproval")?.clone()).ok()?;
        let state = match details["status"].as_str()? {
            "ok" => ApprovalState::Applied,
            "declined" => ApprovalState::Declined,
            "cancelled" => ApprovalState::Cancelled,
            "error"
                if matches!(
                    details["error"]["code"].as_str(),
                    Some("stale_preview" | "project_not_active" | "directory_workspace_required")
                ) =>
            {
                ApprovalState::Expired
            }
            _ => ApprovalState::Failed,
        };
        Some(Self {
            preview,
            state,
            expanded: false,
            settled_at: None,
        })
    }
}

pub(super) fn conversation_surface(palette: Palette) -> Div {
    div()
        .rounded(px(16.))
        .border_1()
        .border_color(subtle_border(palette.border))
        .bg(gpui_kit::rgb(palette.surface))
        .shadow(card_shadow(palette.is_dark()))
}

impl ChatPane {
    pub(super) fn pending_tag_call(&self) -> Option<&str> {
        match &self.pending_ui.as_ref()?.kind {
            PendingUiKind::Confirm(ConfirmationContent::Tags { call_id, .. }) => call_id.as_deref(),
            _ => None,
        }
    }

    pub(super) fn respond_to_tag_confirmation(
        &mut self,
        confirmed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = &self.pending_ui else {
            return false;
        };
        if !matches!(
            pending.kind,
            PendingUiKind::Confirm(ConfirmationContent::Tags { .. })
        ) {
            return false;
        }
        let id = pending.id.clone();
        self.respond_confirm(&id, confirmed, false, cx);
        true
    }

    pub(super) fn restore_pending_tag_approval(&mut self) {
        let Some(PendingExtensionUi {
            kind:
                PendingUiKind::Confirm(ConfirmationContent::Tags {
                    call_id: Some(id),
                    preview,
                }),
            ..
        }) = self.pending_ui.as_ref()
        else {
            return;
        };
        let (id, preview) = (id.clone(), preview.clone());
        if let Some(block) = self.find_tool_mut(&id) {
            if block.approval.is_none() {
                block.done = false;
                block.approval = Some(TagApproval::pending(preview));
            } else {
                self.pending_ui = None;
            }
        } else if let Some(PendingExtensionUi {
            kind: PendingUiKind::Confirm(ConfirmationContent::Tags { call_id, .. }),
            ..
        }) = self.pending_ui.as_mut()
        {
            // A snapshot may arrive before the tool call. Keep the active
            // confirmation reachable until the host publishes its outcome.
            *call_id = None;
        }
    }

    pub(super) fn expire_tag_approvals(&mut self) {
        for message in self
            .messages
            .iter_mut()
            .rev()
            .take_while(|message| matches!(message, ChatMessage::Assistant { .. }))
        {
            if let ChatMessage::Assistant { parts, .. } = message {
                for part in parts {
                    if let AssistantPart::Tool(block) = part
                        && let Some(approval) = &mut block.approval
                        && matches!(
                            approval.state,
                            ApprovalState::Pending | ApprovalState::Applying
                        )
                    {
                        approval.state = ApprovalState::Expired;
                        approval.expanded = false;
                    }
                }
            }
        }
    }

    pub(super) fn render_confirmation(
        &self,
        pending: &PendingExtensionUi,
        content: &ConfirmationContent,
        palette: Palette,
        typo: AiTypography,
        cx: &Context<Self>,
    ) -> AnyElement {
        if let ConfirmationContent::Tags { preview, .. } = content {
            // A host extension can invoke the tool without a visible SDK tool block.
            let mut approval = TagApproval::pending(preview.clone());
            approval.expanded = true;
            return div()
                .mx_3()
                .mb_2()
                .child(self.render_tag_review(
                    &pending.id,
                    Some(&pending.id),
                    &approval,
                    palette,
                    typo,
                    cx,
                ))
                .into_any_element();
        }
        let (icon, body, shell, action) = match content {
            ConfirmationContent::Message(message) => (
                IconName::Shield,
                div().text_size(typo.sm).child(message.clone()),
                false,
                "允许",
            ),
            ConfirmationContent::Command(command) => {
                let (font, size) = tool_typography(&self.settings);
                (
                    IconName::Terminal,
                    div()
                        .p_2()
                        .rounded(px(6.))
                        .bg(gpui_kit::rgb(palette.code_background))
                        .font_family(font)
                        .text_size(size)
                        .child(command.clone()),
                    true,
                    "运行命令",
                )
            }
            ConfirmationContent::Tags { .. } => unreachable!("tag previews use the receipt layout"),
        };
        div()
            .mx_3()
            .mb_2()
            .child(
                conversation_surface(palette)
                    .id(format!("confirmation-{}", pending.id))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(review_heading(
                        icon,
                        &pending.title,
                        None,
                        ApprovalState::Pending,
                        palette,
                        typo,
                    ))
                    .child(
                        div()
                            .id("confirmation-body")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .child(body),
                    )
                    .child(self.confirm_buttons(&pending.id, shell, action, palette, cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_tag_approval(
        &self,
        block: &ToolBlock,
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let approval = block
            .approval
            .as_ref()
            .expect("approval timeline node has a receipt");
        let pending_id = self
            .pending_tag_call()
            .filter(|call| *call == block.id)
            .and(self.pending_ui.as_ref().map(|pending| pending.id.as_str()));
        div()
            .font_family(ui_font.clone())
            .child(self.render_tag_review(&block.id, pending_id, approval, palette, typo, cx))
            .into_any_element()
    }

    fn render_tag_review(
        &self,
        id: &str,
        pending_id: Option<&str>,
        approval: &TagApproval,
        palette: Palette,
        typo: AiTypography,
        cx: &Context<Self>,
    ) -> AnyElement {
        let state = approval.state;
        let live = matches!(state, ApprovalState::Pending | ApprovalState::Applying);
        let collapse_duration = Duration::from_millis(220);
        let collapsing = approval
            .settled_at
            .is_some_and(|settled| settled.elapsed() < collapse_duration)
            && !approval.expanded;
        let muted = gpui_kit::rgb(palette.muted);
        let summary = review_summary(&approval.preview, state);
        let icon = if state == ApprovalState::Applied {
            IconName::Check
        } else {
            IconName::Tag
        };
        let toggle = id.to_owned();
        let inline = pending_id != Some(id);
        let details = if live || approval.expanded || collapsing {
            let body = tag_preview(&approval.preview, approval.expanded, palette, typo)
                .when(!inline, |body| body.max_h(px(240.)).overflow_y_scroll());
            if collapsing {
                Some(
                    body.overflow_hidden()
                        .with_animation(
                            "approval-collapse",
                            Animation::new(collapse_duration).with_easing(ease_out_quint()),
                            |body, t| body.max_h(px(210. * (1. - t))).opacity(1. - t),
                        )
                        .into_any_element(),
                )
            } else {
                Some(body.into_any_element())
            }
        } else {
            None
        };
        let card = conversation_surface(palette)
            .id(format!("tag-review-{id}"))
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(review_heading(
                        icon,
                        &summary,
                        if live || approval.expanded {
                            uniform_tag_chips(&approval.preview, palette, typo)
                        } else {
                            None
                        },
                        state,
                        palette,
                        typo,
                    )))
                    .when(!live, |row| {
                        row.child(
                            Button::new("receipt-details")
                                .ghost()
                                .compact()
                                .label(if approval.expanded {
                                    "收起"
                                } else {
                                    "展开"
                                })
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    if let Some(receipt) = pane
                                        .find_tool_mut(&toggle)
                                        .and_then(|block| block.approval.as_mut())
                                    {
                                        receipt.expanded = !receipt.expanded;
                                        cx.notify();
                                    }
                                })),
                        )
                    }),
            )
            .children(details)
            .when(state == ApprovalState::Pending && inline, |card| {
                if let TagConfirmation::Files { files, .. } = &approval.preview
                    && files.len() > 5
                {
                    let expand = id.to_owned();
                    card.child(
                        div().pl_1().child(
                            Button::new("show-all")
                                .ghost()
                                .compact()
                                .label(if approval.expanded {
                                    "收起文件".into()
                                } else {
                                    format!("展开全部 {} 个", files.len())
                                })
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    if let Some(receipt) = pane
                                        .find_tool_mut(&expand)
                                        .and_then(|block| block.approval.as_mut())
                                    {
                                        receipt.expanded = !receipt.expanded;
                                        cx.notify();
                                    }
                                })),
                        ),
                    )
                } else {
                    card
                }
            })
            .when_some(
                pending_id.filter(|_| state == ApprovalState::Pending),
                |card, pending_id| {
                    card.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .flex_wrap()
                            .gap_1()
                            .pt_1()
                            .border_t_1()
                            .border_color(subtle_border(palette.border))
                            .child(
                                div()
                                    .pl_1()
                                    .text_size(typo.xs)
                                    .text_color(muted)
                                    .child("Enter 应用 · Esc 拒绝"),
                            )
                            .child(self.confirm_buttons(
                                pending_id,
                                false,
                                "应用变更",
                                palette,
                                cx,
                            )),
                    )
                },
            )
            .when(state == ApprovalState::Expired, |card| {
                card.child(
                    div()
                        .pl_5()
                        .text_size(typo.xs)
                        .text_color(muted)
                        .child("请重新发起这批变更。"),
                )
            });
        if state == ApprovalState::Pending {
            card.relative()
                .with_animation(
                    format!("tag-review-in-{id}"),
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                    |card, t| card.top(px(6. * (1. - t))).opacity(t),
                )
                .into_any_element()
        } else {
            card.into_any_element()
        }
    }

    fn confirm_buttons(
        &self,
        id: &str,
        shell: bool,
        action: &'static str,
        palette: Palette,
        cx: &Context<Self>,
    ) -> Div {
        let deny = id.to_owned();
        let allow = id.to_owned();
        let session = id.to_owned();
        let primary = ButtonCustomVariant::new(cx)
            .color(gpui_kit::rgb(palette.accent).into())
            .foreground(gpui_kit::rgb(palette.on_accent).into())
            .hover(mix_color(palette.accent, palette.text, 0.11).into())
            .active(mix_color(palette.accent, palette.text, 0.20).into());
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap_1p5()
            .child(
                Button::new("deny")
                    .ghost()
                    .compact()
                    .h(px(26.))
                    .rounded(px(7.))
                    .label("拒绝")
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.respond_confirm(&deny, false, false, cx)
                    })),
            )
            .when(shell, |row| {
                row.child(
                    Button::new("allow-session")
                        .ghost()
                        .compact()
                        .h(px(26.))
                        .rounded(px(7.))
                        .label("此会话始终允许")
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            pane.respond_confirm(&session, true, true, cx)
                        })),
                )
            })
            .child(
                Button::new("allow")
                    .custom(primary)
                    .compact()
                    .h(px(26.))
                    .rounded(px(7.))
                    .label(action)
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.respond_confirm(&allow, true, false, cx)
                    })),
            )
    }
}

fn review_heading(
    icon: IconName,
    title: &str,
    tags: Option<Div>,
    state: ApprovalState,
    palette: Palette,
    typo: AiTypography,
) -> Div {
    let muted = gpui_kit::rgb(palette.muted);
    let success = if palette.is_dark() {
        0x8ccaa6
    } else {
        0x32664b
    };
    let (label, tone) = match state {
        ApprovalState::Pending => (
            "待确认",
            if palette.is_dark() {
                0xf3c680
            } else {
                0x956016
            },
        ),
        ApprovalState::Applying => ("应用中", palette.accent),
        ApprovalState::Applied => ("已应用", success),
        ApprovalState::Declined => ("已拒绝", palette.muted),
        ApprovalState::Expired => ("已过期", palette.muted),
        ApprovalState::Cancelled => ("已取消", palette.muted),
        ApprovalState::Failed => ("未应用", palette.danger),
    };
    let glyph = crate::ui::icon(icon)
        .size(px(13.))
        .text_color(gpui_kit::rgb(if state == ApprovalState::Applied {
            success
        } else {
            palette.accent
        }));
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap_1p5()
        .child(if state == ApprovalState::Applied {
            glyph
                .with_animation(
                    "receipt-check",
                    Animation::new(Duration::from_millis(190)),
                    |icon, t| icon.opacity(t),
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .max_w_full()
                        .min_w_0()
                        .text_ellipsis()
                        .text_size(typo.sm)
                        .font_medium()
                        .text_color(
                            if matches!(
                                state,
                                ApprovalState::Declined
                                    | ApprovalState::Expired
                                    | ApprovalState::Cancelled
                            ) {
                                muted
                            } else {
                                gpui_kit::rgb(palette.text)
                            },
                        )
                        .child(title.to_owned()),
                )
                .children(tags),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .px_1p5()
                .rounded_full()
                .bg(mix_color(palette.surface, tone, 0.10))
                .text_size(typo.xs)
                .text_color(gpui_kit::rgb(tone))
                .child(if state == ApprovalState::Applying {
                    crate::ui::icon(IconName::RefreshCw)
                        .size(px(9.))
                        .with_animation(
                            "approval-spin",
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |icon, t| icon.rotate(gpui::radians(t * std::f32::consts::TAU)),
                        )
                        .into_any_element()
                } else {
                    div()
                        .size(px(4.))
                        .rounded_full()
                        .bg(gpui_kit::rgb(tone))
                        .into_any_element()
                })
                .child(label),
        )
}

/// A single delta shared by every file is presented once, above the file list.
fn common_delta(files: &[FileTagDelta]) -> Option<&FileTagDelta> {
    files.first().filter(|first| {
        files
            .iter()
            .all(|file| file.add == first.add && file.remove == first.remove)
    })
}

fn review_summary(preview: &TagConfirmation, state: ApprovalState) -> String {
    let applied = state == ApprovalState::Applied;
    match preview {
        TagConfirmation::Files {
            files,
            color_changes,
            ..
        } => {
            if files.is_empty() {
                return format!(
                    "{} {} 个标签的颜色",
                    if applied { "已更改" } else { "更改" },
                    color_changes.len()
                );
            }
            let count = files.len();
            match common_delta(files) {
                Some(delta) if delta.remove.is_empty() => {
                    if applied {
                        format!("已为 {count} 个文件添加「{}」", delta.add.join("、"))
                    } else {
                        format!("为 {count} 个文件添加标签")
                    }
                }
                Some(delta) if delta.add.is_empty() => {
                    if applied {
                        format!("已从 {count} 个文件移除「{}」", delta.remove.join("、"))
                    } else {
                        format!("从 {count} 个文件移除标签")
                    }
                }
                _ => format!(
                    "{} {count} 个文件的标签",
                    if applied { "已更新" } else { "调整" }
                ),
            }
        }
        TagConfirmation::Rename { from, to, .. } if applied => {
            format!("已将「{from}」重命名为「{to}」")
        }
        TagConfirmation::Color { tag, .. } if applied => format!("已更改「{tag}」的颜色"),
        TagConfirmation::Rename { .. } => "重命名标签".into(),
        TagConfirmation::Color { .. } => "更改标签颜色".into(),
    }
}

fn uniform_tag_chips(
    preview: &TagConfirmation,
    palette: Palette,
    typo: AiTypography,
) -> Option<Div> {
    let TagConfirmation::Files {
        files,
        created_tags,
        label_colors,
        ..
    } = preview
    else {
        return None;
    };
    let delta = common_delta(files)?;
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(delta.add.iter().map(|tag| {
                let label = if created_tags.contains(tag) {
                    format!("{tag}（新建）")
                } else {
                    tag.clone()
                };
                tag_swatch(*label_colors.get(tag).unwrap_or(&0), &label, typo)
            }))
            .children(delta.remove.iter().map(|tag| {
                tag_chip(
                    format!("− {tag}"),
                    gpui_kit::rgb(palette.danger),
                    mix_color(palette.surface, palette.danger, 0.09),
                    typo,
                )
            })),
    )
}

fn tag_preview(
    preview: &TagConfirmation,
    expanded: bool,
    palette: Palette,
    typo: AiTypography,
) -> Stateful<Div> {
    let muted = gpui_kit::rgb(palette.muted);
    let border = subtle_border(palette.border);
    let mut body = div()
        .id("receipt-preview")
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .border_t_1()
        .border_color(border)
        .pt_1();
    match preview {
        TagConfirmation::Files {
            files,
            created_tags,
            color_changes,
            label_colors,
        } => {
            let uniform = common_delta(files).is_some();
            let count = if expanded {
                files.len()
            } else {
                files.len().min(5)
            };
            body = body.when(!uniform && !created_tags.is_empty(), |body| {
                body.child(
                    div()
                        .pl_1()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .text_size(typo.xs)
                        .text_color(muted)
                        .child("新建")
                        .children(created_tags.iter().map(|tag| {
                            tag_swatch(*label_colors.get(tag).unwrap_or(&0), tag, typo)
                        })),
                )
            });
            body = body.children(color_changes.iter().filter_map(|change| {
                let from = change.from?;
                Some(
                    div()
                        .pl_1()
                        .flex()
                        .flex_wrap()
                        .gap_1p5()
                        .items_center()
                        .text_size(typo.xs)
                        .text_color(muted)
                        .child(format!(
                            "「{}」换色，影响 {} 个关联文件",
                            change.tag, change.affected
                        ))
                        .child(tag_transition(
                            color_swatch(from, typo),
                            color_swatch(change.to, typo),
                            palette,
                        )),
                )
            }));
            body.children(files.iter().take(count).map(|file| {
                let (folder, name) = file
                    .path
                    .rsplit_once('/')
                    .unwrap_or(("", file.path.as_str()));
                let green = if palette.is_dark() {
                    0x8ccaa6
                } else {
                    0x32664b
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(36.))
                    .py_1()
                    .px_1()
                    .child(div().flex_shrink_0().child(crate::ui::file_icon(
                        &file.path,
                        !crate::files::is_text(Path::new(name)),
                        false,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .line_height(relative(1.15))
                            .child(
                                div()
                                    .text_ellipsis()
                                    .text_size(typo.sm)
                                    .child(name.to_owned()),
                            )
                            .when(!folder.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_ellipsis()
                                        .text_size(px(12.))
                                        .text_color(muted)
                                        .child(folder.to_owned()),
                                )
                            })
                            .when(!uniform, |row| {
                                row.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_1()
                                        .pt_0p5()
                                        .children(file.add.iter().map(|tag| {
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_0p5()
                                                .child(
                                                    div()
                                                        .text_size(typo.xs)
                                                        .text_color(gpui_kit::rgb(green))
                                                        .child("+"),
                                                )
                                                .child(tag_swatch(
                                                    *label_colors.get(tag).unwrap_or(&0),
                                                    tag,
                                                    typo,
                                                ))
                                        }))
                                        .children(file.remove.iter().map(|tag| {
                                            tag_chip(
                                                format!("− {tag}"),
                                                gpui_kit::rgb(palette.danger),
                                                mix_color(palette.surface, palette.danger, 0.09),
                                                typo,
                                            )
                                        })),
                                )
                            }),
                    )
            }))
        }
        TagConfirmation::Rename { from, to, affected } => body
            .child(tag_transition(
                tag_chip(
                    from.clone(),
                    gpui_kit::rgb(palette.text),
                    gpui_kit::rgb(palette.hover),
                    typo,
                ),
                tag_chip(
                    to.clone(),
                    gpui_kit::rgb(palette.text),
                    mix_color(palette.surface, palette.accent, 0.13),
                    typo,
                ),
                palette,
            ))
            .child(
                div()
                    .text_size(typo.xs)
                    .text_color(muted)
                    .child(format!("影响 {affected} 个关联文件")),
            ),
        TagConfirmation::Color {
            tag,
            from,
            to,
            affected,
        } => body
            .child(
                div()
                    .text_size(typo.xs)
                    .text_color(muted)
                    .child(format!("「{tag}」，影响 {affected} 个关联文件")),
            )
            .child(tag_transition(
                color_swatch(*from, typo),
                color_swatch(*to, typo),
                palette,
            )),
    }
}

fn swatch_colors(index: u8) -> (&'static str, u32, u32) {
    *crate::ui::TAG_COLORS
        .get(index as usize)
        .unwrap_or(&crate::ui::TAG_COLORS[0])
}

fn color_swatch(index: u8, typo: AiTypography) -> Div {
    tag_swatch(index, swatch_colors(index).0, typo)
}

fn tag_swatch(index: u8, label: &str, typo: AiTypography) -> Div {
    let (_, bg, ink) = swatch_colors(index);
    tag_chip(
        label.to_owned(),
        gpui_kit::rgb(ink),
        gpui_kit::rgb(bg),
        typo,
    )
}

fn tag_transition(before: Div, after: Div, palette: Palette) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1p5()
        .child(before)
        .child(
            div()
                .text_color(gpui_kit::rgb(palette.muted))
                .child(crate::ui::icon(IconName::ChevronRight).size(px(11.))),
        )
        .child(after)
}

fn tag_chip(label: String, text: Rgba, background: Rgba, typo: AiTypography) -> Div {
    div()
        .max_w_full()
        .min_w_0()
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .text_ellipsis()
        .bg(background)
        .text_color(text)
        .text_size(typo.xs)
        .child(label)
}
