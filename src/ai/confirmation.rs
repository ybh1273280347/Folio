//! Inline receipts for executed tag tools and the shared surface used by AI confirmations.
use super::*;
use crate::tags::agent::{FileTagDelta, TagConfirmation};

#[derive(Clone, Debug)]
pub(super) enum ConfirmationContent {
    Message(String),
    Command(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApprovalState {
    Pending,
    Applied,
    Undone,
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
}

impl TagApproval {
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

    pub(super) fn render_confirmation(
        &self,
        pending: &PendingExtensionUi,
        content: &ConfirmationContent,
        palette: Palette,
        typo: AiTypography,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (icon, body, shell, action) = match content {
            ConfirmationContent::Message(message) => (
                IconName::Shield,
                div().text_size(typo.sm).child(message.clone()),
                false,
                "允许",
            ),
            ConfirmationContent::Command(command) => {
                let (font, size) = crate::theme::tool_typography(&self.settings);
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
        div()
            .font_family(ui_font.clone())
            .child(self.render_tag_review(&block.id, approval, palette, typo, cx))
            .into_any_element()
    }

    pub(super) fn render_applied_approval_group(
        &self,
        blocks: &[&ToolBlock],
        palette: Palette,
        typo: AiTypography,
        ui_font: &SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let group_key = blocks[0].id.clone();
        let expanded = self.expanded_approval_groups.contains(&group_key);
        let all_undone = blocks.iter().all(|b| {
            b.approval
                .as_ref()
                .is_some_and(|a| a.state == ApprovalState::Undone)
        });
        let mut total_files = 0usize;
        let mut applied_ids: Vec<String> = Vec::new();
        for block in blocks {
            if let Some(approval) = &block.approval {
                if approval.state == ApprovalState::Applied {
                    applied_ids.push(block.id.clone());
                }
                if let TagConfirmation::Files { files, .. } = &approval.preview {
                    total_files += files.len();
                }
            }
        }
        let summary = if all_undone {
            if total_files > 0 {
                format!("已撤销 {total_files} 个文件的标签")
            } else {
                format!("已撤销 {} 项标签变更", blocks.len())
            }
        } else if total_files > 0 {
            format!("已更新 {total_files} 个文件的标签")
        } else {
            format!("已完成 {} 项标签变更", blocks.len())
        };
        let success = if palette.is_dark() { 0x8ccaa6 } else { 0x32664b };
        let muted = gpui_kit::rgb(palette.muted);
        let text_hover = gpui_kit::rgb(palette.text);
        let hover_bg = gpui_kit::hsla(0., 0., if palette.is_dark() { 1. } else { 0. }, 0.04);
        let toggle_key = group_key.clone();
        let row_id = SharedString::from(format!("tag-group-{group_key}"));

        let chevron = div()
            .flex_shrink_0()
            .opacity(0.5)
            .group_hover(row_id.clone(), |s| s.opacity(0.85))
            .child(
                crate::ui::icon(IconName::ChevronRight)
                    .size(px(12.))
                    .text_color(muted)
                    .with_animation(
                        (row_id.clone(), expanded as usize),
                        Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
                        move |ic, t| {
                            let angle = if expanded {
                                t * std::f32::consts::FRAC_PI_2
                            } else {
                                (1.0 - t) * std::f32::consts::FRAC_PI_2
                            };
                            ic.rotate(gpui::radians(angle))
                        },
                    ),
            );

        let header_size = px(f32::from(typo.sm).clamp(12.0, 13.5));

        let right_action = if all_undone {
            div()
                .text_size(header_size)
                .text_color(muted)
                .flex_shrink_0()
                .child("已撤销")
                .into_any_element()
        } else {
            let undo_ids = applied_ids;
            div()
                .id(SharedString::from(format!("tag-group-undo-{group_key}")))
                .px_1()
                .text_size(header_size)
                .text_color(muted)
                .flex_shrink_0()
                .cursor_pointer()
                .hover(move |s| s.text_color(text_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |pane, _, _, cx| {
                    cx.stop_propagation();
                    let mut previews = Vec::new();
                    for id in undo_ids.iter().rev() {
                        if let Some(approval) =
                            pane.find_tool_mut(id).and_then(|b| b.approval.as_mut())
                            && approval.state == ApprovalState::Applied
                        {
                            approval.state = ApprovalState::Undone;
                            previews.push(approval.preview.clone());
                        }
                    }
                    if !previews.is_empty() {
                        cx.emit(AiPaneEvent::UndoTags(previews));
                        cx.notify();
                    }
                }))
                .child("撤销")
                .into_any_element()
        };

        div()
            .font_family(ui_font.clone())
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .id(row_id.clone())
                    .group(row_id)
                    .w_full()
                    .min_h(px(24.))
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .when(all_undone, |d| d.opacity(0.65))
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if !pane.expanded_approval_groups.remove(&toggle_key) {
                            pane.expanded_approval_groups.insert(toggle_key.clone());
                        }
                        cx.notify();
                    }))
                    .child(
                        crate::ui::icon(IconName::Check)
                            .size(px(12.5))
                            .flex_shrink_0()
                            .text_color(if all_undone {
                                muted
                            } else {
                                gpui_kit::rgb(success)
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_size(header_size)
                            .text_color(muted)
                            .child(summary),
                    )
                    .child(right_action)
                    .child(chevron),
            )
            .when(expanded, |col| {
                col.child(
                    div()
                        .ml(px(12.))
                        .pl_3()
                        .border_l_1()
                        .border_color(subtle_border(palette.border))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(
                            blocks
                                .iter()
                                .map(|block| self.render_tag_approval(block, palette, typo, ui_font, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_tag_review(
        &self,
        id: &str,
        approval: &TagApproval,
        palette: Palette,
        typo: AiTypography,
        cx: &Context<Self>,
    ) -> AnyElement {
        let state = approval.state;
        let muted = gpui_kit::rgb(palette.muted);
        let text_hover = gpui_kit::rgb(palette.text);
        let summary = review_summary(&approval.preview, state);
        let icon = if matches!(state, ApprovalState::Applied | ApprovalState::Undone) {
            IconName::Check
        } else {
            IconName::Tag
        };
        let toggle = id.to_owned();
        let expanded = approval.expanded;
        let details = expanded.then(|| tag_preview(&approval.preview, palette, typo).into_any_element());
        let row_id = SharedString::from(format!("tag-review-{id}"));
        let hover_bg = gpui_kit::hsla(0., 0., if palette.is_dark() { 1. } else { 0. }, 0.04);
        let chevron = div()
            .flex_shrink_0()
            .opacity(0.5)
            .group_hover(row_id.clone(), |s| s.opacity(0.85))
            .child(
                crate::ui::icon(IconName::ChevronRight)
                    .size(px(12.))
                    .text_color(muted)
                    .with_animation(
                        (row_id.clone(), expanded as usize),
                        Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
                        move |ic, t| {
                            let angle = if expanded {
                                t * std::f32::consts::FRAC_PI_2
                            } else {
                                (1.0 - t) * std::f32::consts::FRAC_PI_2
                            };
                            ic.rotate(gpui::radians(angle))
                        },
                    ),
            );

        let header_size = px(f32::from(typo.sm).clamp(12.0, 13.5));

        let right_action = match state {
            ApprovalState::Applied => {
                let undo_id = id.to_owned();
                Some(
                    div()
                        .id(SharedString::from(format!("tag-undo-{id}")))
                        .px_1()
                        .text_size(header_size)
                        .text_color(muted)
                        .flex_shrink_0()
                        .cursor_pointer()
                        .hover(move |s| s.text_color(text_hover))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(receipt) = pane
                                .find_tool_mut(&undo_id)
                                .and_then(|block| block.approval.as_mut())
                                && receipt.state == ApprovalState::Applied
                            {
                                receipt.state = ApprovalState::Undone;
                                cx.emit(AiPaneEvent::UndoTags(vec![receipt.preview.clone()]));
                                cx.notify();
                            }
                        }))
                        .child("撤销")
                        .into_any_element(),
                )
            }
            ApprovalState::Undone => Some(
                div()
                    .text_size(header_size)
                    .text_color(muted)
                    .flex_shrink_0()
                    .child("已撤销")
                    .into_any_element(),
            ),
            _ => None,
        };

        let inline_chips = if state == ApprovalState::Applied {
            uniform_tag_chips(&approval.preview, palette, typo)
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .id(row_id.clone())
                    .group(row_id)
                    .w_full()
                    .min_h(px(24.))
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap_1p5()
                    .cursor_pointer()
                    .when(state == ApprovalState::Undone, |d| d.opacity(0.65))
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if let Some(receipt) = pane
                            .find_tool_mut(&toggle)
                            .and_then(|block| block.approval.as_mut())
                        {
                            receipt.expanded = !receipt.expanded;
                            cx.notify();
                        }
                    }))
                    .child(div().flex_1().min_w_0().child(review_heading(
                        icon,
                        &summary,
                        inline_chips,
                        state,
                        palette,
                        typo,
                    )))
                    .children(right_action)
                    .child(chevron),
            )
            .when_some(details, |col, body| {
                col.child(
                    div()
                        .ml(px(12.))
                        .pl_3()
                        .pb_1()
                        .border_l_1()
                        .border_color(subtle_border(palette.border))
                        .child(body),
                )
            })
            .into_any_element()
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
        ApprovalState::Applied | ApprovalState::Undone => ("", success),
        ApprovalState::Declined => ("已拒绝", palette.muted),
        ApprovalState::Expired => ("已过期", palette.muted),
        ApprovalState::Cancelled => ("已取消", palette.muted),
        ApprovalState::Failed => ("未应用", palette.danger),
    };
    let glyph = crate::ui::icon(icon)
        .size(px(12.5))
        .text_color(gpui_kit::rgb(match state {
            ApprovalState::Applied => success,
            ApprovalState::Failed => palette.danger,
            ApprovalState::Pending => palette.accent,
            ApprovalState::Undone
            | ApprovalState::Declined
            | ApprovalState::Expired
            | ApprovalState::Cancelled => palette.muted,
        }));
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap_1p5()
        .child(glyph.into_any_element())
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
                        .text_size(px(f32::from(typo.sm).clamp(12.0, 13.5)))
                        .when(state == ApprovalState::Pending, |d| d.font_medium())
                        .text_color(match state {
                            ApprovalState::Pending => gpui_kit::rgb(palette.text),
                            ApprovalState::Failed => gpui_kit::rgb(palette.danger),
                            ApprovalState::Applied
                            | ApprovalState::Undone
                            | ApprovalState::Declined
                            | ApprovalState::Expired
                            | ApprovalState::Cancelled => muted,
                        })
                        .child(title.to_owned()),
                )
                .children(tags),
        )
        .when(
            !matches!(state, ApprovalState::Applied | ApprovalState::Undone),
            |row| {
                row.child(
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
                        .child(
                            div()
                                .size(px(4.))
                                .rounded_full()
                                .bg(gpui_kit::rgb(tone)),
                        )
                        .child(label),
                )
            },
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
    let undone = state == ApprovalState::Undone;
    match preview {
        TagConfirmation::Files {
            files,
            color_changes,
            ..
        } => {
            if files.is_empty() {
                return format!(
                    "{} {} 个标签的颜色",
                    if undone {
                        "已撤销更改"
                    } else if applied {
                        "已更改"
                    } else {
                        "更改"
                    },
                    color_changes.len()
                );
            }
            let count = files.len();
            match common_delta(files) {
                Some(delta) if delta.remove.is_empty() => {
                    if undone {
                        format!("已撤销「{}」的 {count} 个文件标签", delta.add.join("、"))
                    } else if applied {
                        format!("已为 {count} 个文件添加")
                    } else {
                        format!("为 {count} 个文件添加标签")
                    }
                }
                Some(delta) if delta.add.is_empty() => {
                    if undone {
                        format!("已撤销「{}」的 {count} 个文件标签", delta.remove.join("、"))
                    } else if applied {
                        format!("已从 {count} 个文件移除")
                    } else {
                        format!("从 {count} 个文件移除标签")
                    }
                }
                _ => format!(
                    "{} {count} 个文件的标签",
                    if undone {
                        "已撤销"
                    } else if applied {
                        "已更新"
                    } else {
                        "调整"
                    }
                ),
            }
        }
        TagConfirmation::Rename { from, to, .. } if undone => {
            format!("已撤销将「{from}」重命名为「{to}」")
        }
        TagConfirmation::Rename { from, to, .. } if applied => {
            format!("已将「{from}」重命名为「{to}」")
        }
        TagConfirmation::Color { tag, .. } if undone => format!("已撤销更改「{tag}」的颜色"),
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
    let muted = gpui_kit::rgb(palette.muted);
    Some(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .children(delta.add.iter().flat_map(|tag| {
                let swatch = tag_swatch(*label_colors.get(tag).unwrap_or(&0), tag, typo)
                    .into_any_element();
                if created_tags.contains(tag) {
                    vec![
                        swatch,
                        div()
                            .text_size(typo.xs)
                            .text_color(muted)
                            .child("· 新建")
                            .into_any_element(),
                    ]
                } else {
                    vec![swatch]
                }
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

fn common_dir_prefix(files: &[FileTagDelta]) -> String {
    let dirs: Vec<&str> = files
        .iter()
        .map(|f| f.path.rsplit_once('/').map_or("", |(d, _)| d))
        .collect();
    let Some(first) = dirs.first().copied() else {
        return String::new();
    };
    if first.is_empty() {
        return String::new();
    }
    let mut prefix = first;
    for dir in &dirs[1..] {
        while !prefix.is_empty() && *dir != prefix && !dir.starts_with(&format!("{prefix}/")) {
            prefix = prefix.rsplit_once('/').map_or("", |(p, _)| p);
        }
    }
    prefix.to_owned()
}

fn tag_preview(
    preview: &TagConfirmation,
    palette: Palette,
    typo: AiTypography,
) -> Stateful<Div> {
    let muted = gpui_kit::rgb(palette.muted);
    let mut body = div()
        .id("receipt-preview")
        .min_w_0()
        .flex()
        .flex_col()
        .gap_0p5()
        .pt_0p5();
    match preview {
        TagConfirmation::Files {
            files,
            created_tags,
            color_changes,
            label_colors,
        } => {
            let uniform = common_delta(files).is_some();
            let common_dir = common_dir_prefix(files);
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
            if !common_dir.is_empty() {
                body = body.child(
                    div()
                        .px_1()
                        .pb_0p5()
                        .text_size(typo.xs)
                        .text_color(muted)
                        .child(format!("{common_dir} · {} 个文件", files.len())),
                );
            }
            let list = div()
                .id("receipt-file-list")
                .flex()
                .flex_col()
                .gap(px(1.))
                .when(files.len() > 6, |l| l.max_h(px(200.)).overflow_y_scroll())
                .children(files.iter().map(|file| {
                    let name = file
                        .path
                        .rsplit_once('/')
                        .map_or(file.path.as_str(), |(_, n)| n);
                    let display = if common_dir.is_empty() {
                        file.path.as_str()
                    } else {
                        file.path
                            .strip_prefix(&format!("{common_dir}/"))
                            .unwrap_or(name)
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .min_h(px(24.))
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
                                .text_ellipsis()
                                .text_size(typo.xs)
                                .text_color(muted)
                                .child(display.to_owned()),
                        )
                }));
            body.child(list)
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
        .py(px(1.))
        .rounded_full()
        .text_ellipsis()
        .bg(background)
        .text_color(text)
        .text_size(px(f32::from(typo.xs).min(11.)))
        .child(label)
}
