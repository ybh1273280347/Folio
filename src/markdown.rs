//! Native Markdown reading and editing. Source is retained verbatim between edits;
//! visual blocks edit their own source range so unrelated formatting stays intact.
use crate::math::{MathReady, MathRenderer};
use crate::model::AppSettings;
use crate::richtext::RichText;
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use gpui_kit::{
    base::StyledExt,
    component::{
        input::{Editor, EditorState, InputEvent, Position, TextDecorationCollection},
        text::{TaskToggleEvent, TextView},
    },
    prelude::FluentBuilder,
    *,
};
use std::{ops::Range, path::PathBuf};

pub enum MarkdownEvent {
    Change(String),
    Focus,
    Zoom(u8),
}
actions!(folio_markdown, [ToggleSource]);

/// Normalize math delimiters for the renderer without changing byte offsets used
/// by task markers and block ranges. Code spans and fenced code stay literal.
pub fn normalize_math(text: &str) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    let mut ticks = 0;
    while cursor < text.len() {
        let tail = &text[cursor..];
        if tail.starts_with('`') {
            let count = tail.bytes().take_while(|byte| *byte == b'`').count();
            if ticks == 0 {
                ticks = count;
            } else if ticks == count {
                ticks = 0;
            }
            result.push_str(&tail[..count]);
            cursor += count;
            continue;
        }
        if ticks == 0 {
            let replacement = if tail.starts_with("\\(") {
                Some("$ ")
            } else if tail.starts_with("\\)") {
                Some(" $")
            } else if tail.starts_with("\\[") || tail.starts_with("\\]") {
                Some("$$")
            } else {
                None
            };
            if let Some(replacement) = replacement {
                result.push_str(replacement);
                cursor += 2;
                continue;
            }
        }
        let character = tail.chars().next().expect("cursor is inside source");
        result.push(character);
        cursor += character.len_utf8();
    }
    result
}
struct Block {
    range: Range<usize>,
    heading: Option<(u8, String)>,
    table: Vec<Vec<Range<usize>>>,
}
pub struct MarkdownPane {
    text: String,
    path: PathBuf,
    blocks: Vec<Block>,
    editor: Entity<EditorState>,
    source: bool,
    editing: Option<usize>,
    editing_range: Option<Range<usize>>,
    editing_cell: Option<(usize, usize)>,
    rich: Option<RichText>,
    decorations: Option<TextDecorationCollection>,
    outline: bool,
    scroll: ScrollHandle,
    settings: AppSettings,
    math: Entity<MathRenderer>,
    _subscription: Subscription,
}
impl EventEmitter<MarkdownEvent> for MarkdownPane {}

impl MarkdownPane {
    pub fn new(
        text: String,
        path: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&math, |_, _, _: &MathReady, cx| cx.notify())
            .detach();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .soft_wrap(true)
                .searchable(true)
        });
        let subscription = cx.subscribe(
            &editor,
            |pane: &mut Self, state, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    let mut value = state.read(cx).value().to_string();
                    if pane.source {
                        pane.text = value;
                    } else if let Some(index) = pane.editing {
                        if let Some(rich) = &mut pane.rich {
                            rich.change(value);
                            value = rich.markdown();
                            if let Some(styles) = &pane.decorations {
                                styles.set(rich.decorations(), cx);
                            }
                        }
                        let range = pane.editing_range.clone().expect("active visual range");
                        let new_end = range.start + value.len();
                        let change = value.len() as isize - range.len() as isize;
                        pane.text.replace_range(range.clone(), &value);
                        pane.editing_range = Some(range.start..new_end);
                        pane.blocks[index].range.end = pane.blocks[index]
                            .range
                            .end
                            .checked_add_signed(change)
                            .unwrap();
                        for block in pane.blocks.iter_mut().skip(index + 1) {
                            block.range.start = block
                                .range
                                .start
                                .checked_add_signed(change)
                                .expect("source edit preserves block offsets");
                            block.range.end = block
                                .range
                                .end
                                .checked_add_signed(change)
                                .expect("source edit preserves block offsets");
                        }
                        for row in &mut pane.blocks[index].table {
                            for cell in row {
                                if cell.start >= range.end {
                                    cell.start = cell.start.checked_add_signed(change).unwrap();
                                    cell.end = cell.end.checked_add_signed(change).unwrap();
                                } else if cell.start == range.start {
                                    cell.end = new_end;
                                }
                            }
                        }
                    }
                    cx.emit(MarkdownEvent::Change(pane.text.clone()));
                    cx.notify();
                }
                InputEvent::Focus => cx.emit(MarkdownEvent::Focus),
                _ => {}
            },
        );
        let mut pane = Self {
            text,
            path,
            blocks: Vec::new(),
            editor,
            source: false,
            editing: None,
            editing_range: None,
            editing_cell: None,
            rich: None,
            decorations: None,
            outline: false,
            scroll: ScrollHandle::new(),
            settings,
            math,
            _subscription: subscription,
        };
        pane.parse();
        pane.prepare_math(cx);
        pane
    }
    fn prepare_math(&self, cx: &mut Context<Self>) {
        let arena = Arena::new();
        let mut options = Options::default();
        options.extension.math_dollars = true;
        let normalized = normalize_math(&self.text);
        let root = parse_document(&arena, &normalized, &options);
        for node in root.descendants() {
            if let NodeValue::Math(math) = &node.data.borrow().value {
                self.math.update(cx, |renderer, _| {
                    renderer.prepare(&math.literal, math.display_math)
                });
            }
        }
    }
    fn parse(&mut self) {
        let arena = Arena::new();
        let mut options = Options::default();
        options.extension.table = true;
        options.extension.tasklist = true;
        options.extension.math_dollars = true;
        let root = parse_document(&arena, &self.text, &options);
        let mut lines = vec![0];
        for (offset, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                lines.push(offset + 1);
            }
        }
        self.blocks = root
            .children()
            .map(|node| {
                let data = node.data.borrow();
                let position = data.sourcepos;
                let start = lines[position.start.line - 1] + position.start.column - 1;
                let end = (lines[position.end.line - 1] + position.end.column).min(self.text.len());
                let heading = match &data.value {
                    NodeValue::Heading(heading) => Some((
                        heading.level,
                        node.descendants()
                            .filter_map(|child| match &child.data.borrow().value {
                                NodeValue::Text(text) => Some(text.to_string()),
                                NodeValue::Code(code) => Some(code.literal.clone()),
                                _ => None,
                            })
                            .collect(),
                    )),
                    _ => None,
                };
                let table = if matches!(data.value, NodeValue::Table(_)) {
                    node.children()
                        .map(|row| {
                            row.children()
                                .map(|cell| {
                                    let p = cell.data.borrow().sourcepos;
                                    let start = lines[p.start.line - 1] + p.start.column - 1;
                                    let end =
                                        (lines[p.end.line - 1] + p.end.column).min(self.text.len());
                                    start..end
                                })
                                .collect()
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Block {
                    range: start..end,
                    heading,
                    table,
                }
            })
            .collect();
        if self.blocks.is_empty() {
            self.blocks.push(Block {
                range: 0..self.text.len(),
                heading: None,
                table: Vec::new(),
            });
        }
    }
    pub fn sync(
        &mut self,
        text: &str,
        settings: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings != *settings {
            cx.notify();
        }
        self.settings = settings.clone();
        if self.text == text {
            return;
        }
        self.text = text.to_owned();
        self.editing = None;
        self.parse();
        self.prepare_math(cx);
        if self.source {
            self.editor.update(cx, |editor, cx| {
                editor.set_value(text.to_owned(), window, cx)
            });
        }
        cx.notify();
    }
    fn edit_block(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.blocks[index].range.start;
        self.parse();
        let index = self
            .blocks
            .iter()
            .position(|block| block.range.start == start)
            .expect("clicked block retains its source start");
        self.editing = Some(index);
        self.editing_cell = None;
        self.prepare_math(cx);
        self.begin_edit(self.blocks[index].range.clone(), window, cx);
    }
    fn begin_edit(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_range = Some(range.clone());
        let text = self.text[range].to_owned();
        let rich = RichText::new(&text);
        self.editor.update(cx, |editor, cx| {
            editor.set_value(rich.text.clone(), window, cx);
            editor.set_highlighter("plaintext", cx);
            editor.focus(window, cx);
        });
        if let Some(styles) = &self.decorations {
            styles.set(rich.decorations(), cx);
        } else {
            self.decorations = Some(self.editor.update(cx, |editor, cx| {
                editor.create_decorations_collection(rich.decorations(), cx)
            }));
        }
        self.rich = Some(rich);
        cx.emit(MarkdownEvent::Focus);
        cx.notify();
    }
    fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.source = !self.source;
        self.editing = None;
        self.parse();
        self.prepare_math(cx);
        self.rich = None;
        if let Some(styles) = &self.decorations {
            styles.set(Vec::new(), cx);
        }
        if self.source {
            self.editor.update(cx, |editor, cx| {
                editor.set_value(self.text.clone(), window, cx);
                editor.set_highlighter("markdown", cx);
                editor.focus(window, cx);
            });
        }
        cx.notify();
    }
    fn format(&mut self, prefix: &str, suffix: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.source && self.editing.is_none() {
            self.edit_block(0, window, cx);
        }
        let editor = self.editor.clone();
        let range = editor.read(cx).selected_range();
        let text = editor.read(cx).value();
        if !self.source && self.editing_cell.is_none() && matches!(prefix, "## " | "- " | "- [ ] ")
        {
            let target = self.editing_range.clone().expect("visual block is active");
            let block = self.text[target.clone()]
                .trim_start_matches('#')
                .trim_start();
            let replacement = format!("{prefix}{block}");
            self.text.replace_range(target, &replacement);
            self.editing = None;
            self.parse();
            cx.emit(MarkdownEvent::Change(self.text.clone()));
            cx.notify();
            return;
        }
        if !self.source && !suffix.is_empty() {
            let rich = self.rich.as_mut().expect("visual block has projection");
            rich.format(range, prefix, suffix);
            self.text.replace_range(
                self.editing_range.clone().expect("active visual range"),
                &rich.markdown(),
            );
            self.parse();
            self.editing = None;
            self.rich = None;
            self.prepare_math(cx);
            cx.emit(MarkdownEvent::Change(self.text.clone()));
            cx.notify();
            return;
        }
        let selected = text[range].to_owned();
        let replacement = format!("{prefix}{selected}{suffix}");
        editor.update(cx, |state, cx| state.replace(replacement, window, cx));
        // Programmatic replacement is silent; route it through the same document event.
        let mut value = editor.read(cx).value().to_string();
        if !self.source {
            if let Some(rich) = &mut self.rich {
                rich.change(value);
                value = rich.markdown();
            }
        }
        if self.source {
            self.text = value;
        } else if self.editing.is_some() {
            self.text.replace_range(
                self.editing_range.clone().expect("active visual range"),
                &value,
            );
            self.parse();
            self.editing = None;
        }
        self.prepare_math(cx);
        cx.emit(MarkdownEvent::Change(self.text.clone()));
        cx.notify();
    }
    fn task(
        &mut self,
        offset: usize,
        event: &TaskToggleEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = event.marker_range();
        self.text.replace_range(
            offset + range.start..offset + range.end,
            if event.checked() { "[x]" } else { "[ ]" },
        );
        self.parse();
        self.editing = None;
        if self.source {
            self.editor.update(cx, |state, cx| {
                state.set_value(self.text.clone(), window, cx)
            });
        }
        cx.emit(MarkdownEvent::Change(self.text.clone()));
        cx.notify();
    }
    fn insert_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title("插入本地图片")
            .add_filter("图片", &["png", "jpg", "jpeg", "webp", "gif", "svg"]);
        let overwrite = rfd::AsyncMessageDialog::new()
            .set_parent(window)
            .set_title("替换同名图片？")
            .set_buttons(rfd::MessageButtons::OkCancel);
        let failure = rfd::AsyncMessageDialog::new()
            .set_parent(window)
            .set_title("图片导入失败");
        let directory = self
            .path
            .parent()
            .expect("Markdown file has parent")
            .to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let Some(handle) = picker.pick_file().await else {
                return;
            };
            let path = handle.path();
            let filename = path
                .file_name()
                .expect("selected image has name")
                .to_owned();
            let destination = directory.join(&filename);
            if path != destination {
                if destination.exists()
                    && overwrite
                        .set_description(destination.to_string_lossy())
                        .show()
                        .await
                        != rfd::MessageDialogResult::Ok
                {
                    return;
                }
                if let Err(error) = std::fs::copy(path, &destination) {
                    failure.set_description(error.to_string()).show().await;
                    return;
                }
            }
            let insertion = format!("![图片](<{}>)", filename.to_string_lossy());
            let _ = this.update_in(cx, |pane, window, cx| {
                pane.format(&insertion, "", window, cx)
            });
        })
        .detach();
    }
}

fn action(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xefefed)))
        .child(crate::ui::label(label.into()))
}
impl Render for MarkdownPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(36.))
            .px_3()
            .border_b_1()
            .border_color(rgb(0xe8e7e3))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        action("md-outline", "☷").on_click(cx.listener(|pane, _, _, cx| {
                            pane.outline = !pane.outline;
                            cx.notify();
                        })),
                    )
                    .child(action("md-bold", "B").font_bold().on_click(
                        cx.listener(|pane, _, window, cx| pane.format("**", "**", window, cx)),
                    ))
                    .child(action("md-italic", "I").on_click(
                        cx.listener(|pane, _, window, cx| pane.format("*", "*", window, cx)),
                    ))
                    .child(action("md-heading", "H").on_click(
                        cx.listener(|pane, _, window, cx| pane.format("## ", "", window, cx)),
                    ))
                    .child(action("md-list", "•").on_click(
                        cx.listener(|pane, _, window, cx| pane.format("- ", "", window, cx)),
                    ))
                    .child(action("md-task", "☑").on_click(
                        cx.listener(|pane, _, window, cx| pane.format("- [ ] ", "", window, cx)),
                    ))
                    .child(action("md-math", "∑").on_click(
                        cx.listener(|pane, _, window, cx| pane.format("$", "$", window, cx)),
                    ))
                    .child(
                        action("md-image", "▧").on_click(
                            cx.listener(|pane, _, window, cx| pane.insert_image(window, cx)),
                        ),
                    )
                    .child(
                        action("md-table", "▦").on_click(cx.listener(|pane, _, window, cx| {
                            pane.format(
                                "\n| 标题 | 内容 |\n| --- | --- |\n|  |  |\n",
                                "",
                                window,
                                cx,
                            )
                        })),
                    ),
            )
            .child(
                action(
                    "md-source",
                    if self.source {
                        "所见即所得"
                    } else {
                        "源码"
                    },
                )
                .on_click(cx.listener(|pane, _, window, cx| pane.toggle_source(window, cx))),
            );
        if self.editing.is_some() && !self.source {
            toolbar = toolbar.child(action("md-finish-edit", "✓").on_click(cx.listener(
                |pane, _, _, cx| {
                    pane.editing = None;
                    pane.parse();
                    cx.notify();
                },
            )));
        }
        let mut content = div().flex().flex_1().min_h_0();
        if self.outline {
            let headings = self.blocks.iter().enumerate().filter_map(|(index, block)| {
                block.heading.as_ref().map(|(level, title)| {
                    action(("heading", index), title.clone())
                        .pl(px(*level as f32 * 12.))
                        .text_xs()
                        .on_click(cx.listener(move |pane, _, window, cx| {
                            if pane.source {
                                let line = pane.text[..pane.blocks[index].range.start]
                                    .bytes()
                                    .filter(|byte| *byte == b'\n')
                                    .count();
                                pane.editor.update(cx, |state, cx| {
                                    state.set_cursor_position(
                                        Position::new(line as u32, 0),
                                        window,
                                        cx,
                                    )
                                });
                            } else {
                                pane.scroll.scroll_to_item(index);
                            }
                            cx.notify();
                        }))
                })
            });
            content = content.child(
                div()
                    .id("markdown-outline")
                    .w(px(190.))
                    .flex_shrink_0()
                    .h_full()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(rgb(0xe8e7e3))
                    .p_2()
                    .children(headings),
            );
        }
        if self.source {
            content = content.child(
                div().flex_1().min_w_0().p_2().child(
                    Editor::new(&self.editor)
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
            let blocks = self.blocks.iter().enumerate().map(|(index, block)| {
                let text = self.text[block.range.clone()].to_owned();
                let offset = block.range.start;
                let app = cx.entity();
                let base = self
                    .path
                    .parent()
                    .expect("Markdown has directory")
                    .to_path_buf();
                let body = if !block.table.is_empty() {
                    let rows = block.table.iter().enumerate().map(|(row_index, row)| {
                        div()
                            .flex()
                            .children(row.iter().enumerate().map(|(column, range)| {
                                let target = range.clone();
                                let mut cell = div()
                                    .id(SharedString::from(format!(
                                        "cell-{index}-{row_index}-{column}"
                                    )))
                                    .flex_1()
                                    .min_w(px(70.))
                                    .min_h(px(38.))
                                    .p_2()
                                    .border_1()
                                    .border_color(rgb(0xe8e7e3))
                                    .when(row_index == 0, |cell| {
                                        cell.bg(rgb(0xf7f6f5)).font_semibold()
                                    });
                                if self.editing == Some(index)
                                    && self.editing_cell == Some((row_index, column))
                                {
                                    cell = cell.child(
                                        Editor::new(&self.editor)
                                            .appearance(false)
                                            .bordered(false)
                                            .h(px(60.))
                                            .w_full()
                                            .font_family("Segoe UI")
                                            .text_size(px(self.settings.markdown_font_size as f32)),
                                    );
                                } else {
                                    cell = cell
                                        .cursor_text()
                                        .child(
                                            TextView::markdown(
                                                SharedString::from(format!(
                                                    "cell-view-{index}-{row_index}-{column}"
                                                )),
                                                normalize_math(&self.text[range.clone()]),
                                            )
                                            .plugin(self.math.read(cx).plugin(false)),
                                        )
                                        .on_click(cx.listener(move |pane, _, window, cx| {
                                            pane.editing = Some(index);
                                            pane.editing_cell = Some((row_index, column));
                                            pane.begin_edit(target.clone(), window, cx);
                                        }));
                                }
                                cell
                            }))
                    });
                    div()
                        .flex()
                        .flex_col()
                        .children(rows)
                        .child(action(("table-row", index), "+ 行").on_click(cx.listener(
                            move |pane, _, _, cx| {
                                let block = &pane.blocks[index];
                                let end = block.range.end;
                                let count = block.table[0].len();
                                pane.text
                                    .insert_str(end, &format!("\n|{}", "  |".repeat(count)));
                                pane.editing = None;
                                pane.parse();
                                cx.emit(MarkdownEvent::Change(pane.text.clone()));
                                cx.notify();
                            },
                        )))
                        .into_any_element()
                } else if self.editing == Some(index) {
                    div()
                        .min_h(px(80.))
                        .h(px((text.lines().count().max(3) as f32 * 24.).min(640.)))
                        .child(
                            Editor::new(&self.editor)
                                .appearance(false)
                                .bordered(false)
                                .size_full()
                                .font_family(if self.settings.interface_font.is_empty() {
                                    SharedString::from("Segoe UI")
                                } else {
                                    self.settings.interface_font.clone().into()
                                })
                                .text_size(px(self.settings.markdown_font_size as f32)),
                        )
                        .into_any_element()
                } else {
                    div()
                        .id(("md-block", index))
                        .min_h(px(24.))
                        .cursor_text()
                        .on_click(cx.listener(move |pane, _: &ClickEvent, window, cx| {
                            pane.edit_block(index, window, cx);
                            cx.emit(MarkdownEvent::Focus);
                        }))
                        .child(
                            TextView::markdown(("md-view", index), normalize_math(&text))
                                .selectable(true)
                                .plugin(self.math.read(cx).plugin(false))
                                .plugin(self.math.read(cx).plugin(true))
                                .image_source(move |uri| {
                                    if uri.as_ref().starts_with("http")
                                        || uri.as_ref().starts_with("data:")
                                    {
                                        ImageSource::from(uri.clone())
                                    } else {
                                        ImageSource::from(base.join(uri.as_ref()))
                                    }
                                })
                                .on_task_toggle(move |event, window, cx| {
                                    app.update(cx, |pane, cx| pane.task(offset, event, window, cx))
                                }),
                        )
                        .into_any_element()
                };
                div().mb_3().child(body)
            });
            content = content.child(
                div()
                    .id("markdown-content")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .text_size(px(self.settings.markdown_font_size as f32))
                    .p_6()
                    .children(blocks)
                    .child(action("md-add-paragraph", "+").on_click(cx.listener(
                        |pane, _, window, cx| {
                            pane.text.push_str("\n\n");
                            let end = pane.text.len();
                            pane.blocks.push(Block {
                                range: end..end,
                                heading: None,
                                table: Vec::new(),
                            });
                            pane.editing = Some(pane.blocks.len() - 1);
                            pane.editing_range = Some(end..end);
                            pane.editing_cell = None;
                            pane.rich = Some(RichText::new(""));
                            pane.editor.update(cx, |state, cx| {
                                state.set_value("", window, cx);
                                state.focus(window, cx);
                            });
                            cx.notify();
                        },
                    ))),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .child(toolbar)
            .child(content)
            .on_action(
                cx.listener(|pane, _: &ToggleSource, window, cx| pane.toggle_source(window, cx)),
            )
            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                if event.modifiers.control {
                    let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                        1
                    } else {
                        -1
                    };
                    pane.settings.markdown_font_size =
                        (pane.settings.markdown_font_size as i16 + delta).clamp(11, 26) as u8;
                    cx.stop_propagation();
                    cx.notify();
                    cx.emit(MarkdownEvent::Zoom(pane.settings.markdown_font_size));
                }
            }))
    }
}
