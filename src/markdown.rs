//! Markdown panes share one GPUI block editor for live editing and keep the
//! source editor only for the explicit source-mode toggle.
use crate::math::{MathReady, MathRenderer, formula_id};
use crate::model::AppSettings;
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use gpui_kit::{
    base::{Scrollbar, ScrollbarMode, text::MarkdownExtensions},
    component::{
        input::{Editor, EditorState, InputEvent, Position},
        menu::ContextMenuExt,
    },
    prelude::FluentBuilder,
    *,
};
use markdown_editor::{
    BlockEditor, BlockEditorEvent, BlockEditorState, BlockKind, BlockPosition, BlockSelection,
    EditorCommand, MarkKind,
};
use std::{cell::RefCell, ops::Range, path::PathBuf, rc::Rc};

pub enum MarkdownEvent {
    Change(String),
    Focus,
    Zoom(u8),
}

actions!(
    folio_markdown,
    [
        ToggleSource,
        Bold,
        Italic,
        HeadingAction,
        BulletList,
        TaskList,
        InsertImage,
        InsertTable
    ]
);

struct OutlineHeading {
    range: Range<usize>,
    level: u8,
    title: String,
}

pub struct MarkdownPane {
    export: Rc<RefCell<Option<crate::markdown_pdf::ExportRequest>>>,
    export_width: Pixels,
    text: String,
    path: PathBuf,
    headings: Vec<OutlineHeading>,
    source_editor: Entity<EditorState>,
    visual_editor: Entity<BlockEditorState>,
    source: bool,
    outline: bool,
    reading: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    settings: AppSettings,
    math: Entity<MathRenderer>,
    formula_editor: Option<(Entity<EditorState>, bool)>,
    pending_image: Option<(PathBuf, PathBuf)>,
    image_error: Option<String>,
    _source_subscription: Subscription,
    _visual_subscription: Subscription,
    _math_subscription: Subscription,
}

impl EventEmitter<MarkdownEvent> for MarkdownPane {}

impl MarkdownPane {
    /// The workspace hosts this capture independently of the selected tab.
    pub fn export_element(&self) -> Option<AnyElement> {
        self.export.borrow().as_ref()?;
        let request = self.export.clone();
        let editor = self.visual_editor.clone();
        let math = self.math.clone();
        let width = self.export_width;
        Some(
            div()
                .id("markdown-pdf-export")
                .absolute()
                .top_0()
                .left_0()
                .text_size(px(self.settings.markdown_font_size as f32))
                .font_family(if self.settings.interface_font.is_empty() {
                    SharedString::from("Segoe UI")
                } else {
                    self.settings.interface_font.clone().into()
                })
                .child(
                    canvas(
                        move |_, window, cx| {
                            let content = BlockEditor::document(&editor, window, cx);
                            let mut element =
                                div().w(width).p_6().child(content).into_any_element();
                            let size = element.layout_as_root(
                                size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                                window,
                                cx,
                            );
                            let bounds = Bounds::new(Point::default(), size);
                            window.with_document_mask(bounds, |window| {
                                element.prepaint_at(Point::default(), window, cx);
                            });
                            (element, size)
                        },
                        move |_, (mut element, size), window, cx| {
                            let bounds = Bounds::new(Point::default(), size);
                            let ops = window.with_document_mask(bounds, |window| {
                                window.capture_vectors(|window| element.paint(window, cx))
                            });
                            let cancelled = request
                                .borrow()
                                .as_ref()
                                .expect("one export")
                                .cancelled
                                .load(std::sync::atomic::Ordering::Relaxed);
                            // Wait for offscreen images and math, while allowing cancellation immediately.
                            if !cancelled
                                && (!math.read(cx).is_idle()
                                    || ops.iter().any(|op| matches!(op, VectorPaint::PendingImage)))
                            {
                                return;
                            }
                            let faces = if cancelled {
                                Ok(Default::default())
                            } else {
                                crate::markdown_pdf::fonts(&ops, cx)
                            };
                            let request = request.borrow_mut().take().expect("one export request");
                            std::thread::spawn(move || {
                                crate::markdown_pdf::publish(request, ops, size, faces)
                            });
                        },
                    )
                    .size(px(0.)),
                )
                .into_any_element(),
        )
    }

    pub fn export_pdf(
        &mut self,
        request: crate::markdown_pdf::ExportRequest,
        cx: &mut Context<Self>,
    ) {
        if self.source {
            self.visual_editor.update(cx, |editor, cx| {
                editor
                    .set_source(&self.text, cx)
                    .expect("current Markdown parses")
            });
        }
        *self.export.borrow_mut() = Some(request);
        self.export_width = if self.settings.line_width_limited {
            self.scroll.bounds().size.width.min(px(850.))
        } else {
            self.scroll.bounds().size.width
        };
        cx.notify();
    }

    pub fn new(
        text: String,
        path: PathBuf,
        settings: AppSettings,
        math: Entity<MathRenderer>,
        reading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroll = ScrollHandle::new();
        math.update(cx, |renderer, cx| renderer.set_appearance(&settings, cx));
        let extensions = MarkdownExtensions::default()
            .plugin(math.read(cx).plugin(false, math.clone()))
            .plugin(math.read(cx).plugin(true, math.clone()));
        let visual_editor = cx.new(|cx| {
            let mut editor = BlockEditorState::new(window, cx);
            editor
                .set_markdown_extensions(extensions, cx)
                .expect("current Markdown parses with the configured GPUI extensions");
            editor
                .set_source(&text, cx)
                .expect("current Markdown parses with the configured GPUI extensions");
            editor.set_scroll_handle(scroll.clone());
            editor
        });
        let source_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .soft_wrap(true)
                .searchable(true)
                .default_value(text.clone())
        });
        let source_subscription = cx.subscribe_in(
            &source_editor,
            window,
            |pane: &mut Self, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) && pane.source {
                    pane.text = state.read(cx).value().to_string();
                    pane.parse_outline();
                    cx.emit(MarkdownEvent::Change(pane.text.clone()));
                    cx.notify();
                } else if matches!(event, InputEvent::Focus) {
                    cx.emit(MarkdownEvent::Focus);
                }
            },
        );
        let visual_subscription = cx.subscribe(
            &visual_editor,
            |pane, editor, event: &BlockEditorEvent, cx| match event {
                BlockEditorEvent::Changed { .. } => {
                    pane.text = editor.read(cx).source();
                    pane.parse_outline();
                    cx.emit(MarkdownEvent::Change(pane.text.clone()));
                    cx.notify();
                }
                BlockEditorEvent::Focus => cx.emit(MarkdownEvent::Focus),
                _ => {}
            },
        );
        let math_subscription =
            cx.subscribe_in(&math, window, |pane, _, event: &MathReady, window, cx| {
                match event {
                    MathReady::Ready => pane.visual_editor.update(cx, |_, cx| cx.notify()),
                    MathReady::Edit { id, latex, display } => {
                        let selected = pane.visual_editor.update(cx, |editor, cx| {
                            editor.select_custom_node(|node| formula_id(node) == Some(*id), cx)
                        });
                        if selected {
                            let editor = cx.new(|cx| {
                                EditorState::new(window, cx)
                                    .line_number(false)
                                    .soft_wrap(true)
                                    .default_value(latex.clone())
                            });
                            editor.update(cx, |editor, cx| editor.focus(window, cx));
                            pane.formula_editor = Some((editor, *display));
                        }
                    }
                }
                cx.notify();
            });
        let mut pane = Self {
            export: Rc::new(RefCell::new(None)),
            export_width: px(0.),
            text,
            path,
            headings: Vec::new(),
            source_editor,
            visual_editor,
            source: false,
            outline: false,
            reading,
            focus: cx.focus_handle(),
            scroll,
            settings,
            math,
            formula_editor: None,
            pending_image: None,
            image_error: None,
            _source_subscription: source_subscription,
            _visual_subscription: visual_subscription,
            _math_subscription: math_subscription,
        };
        pane.parse_outline();
        pane
    }

    fn parse_outline(&mut self) {
        let arena = Arena::new();
        let root = parse_document(&arena, &self.text, &Options::default());
        let lines: Vec<_> = std::iter::once(0)
            .chain(self.text.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        self.headings = root
            .descendants()
            .filter_map(|node| {
                let data = node.data.borrow();
                let NodeValue::Heading(heading) = &data.value else {
                    return None;
                };
                let position = data.sourcepos;
                let start = lines[position.start.line - 1] + position.start.column - 1;
                let end = (lines[position.end.line - 1] + position.end.column).min(self.text.len());
                let title = node
                    .descendants()
                    .filter_map(|child| match &child.data.borrow().value {
                        NodeValue::Text(text) => Some(text.to_string()),
                        NodeValue::Code(code) => Some(code.literal.clone()),
                        _ => None,
                    })
                    .collect();
                Some(OutlineHeading {
                    range: start..end,
                    level: heading.level,
                    title,
                })
            })
            .collect();
    }

    pub fn sync(
        &mut self,
        text: &str,
        settings: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings != *settings {
            self.settings = settings.clone();
            self.math
                .update(cx, |renderer, cx| renderer.set_appearance(settings, cx));
            cx.notify();
        }
        if self.text == text {
            return;
        }
        self.text = text.to_owned();
        self.parse_outline();
        self.visual_editor.update(cx, |editor, cx| {
            editor
                .set_source(text, cx)
                .expect("external Markdown parses with the configured GPUI extensions");
        });
        self.source_editor.update(cx, |editor, cx| {
            editor.set_value(text.to_owned(), window, cx)
        });
        cx.notify();
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            self.source_editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        } else {
            self.visual_editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        }
        cx.emit(MarkdownEvent::Focus);
    }

    pub fn toggle_outline(&mut self, cx: &mut Context<Self>) {
        self.outline = !self.outline;
        cx.notify();
    }

    pub fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.source = !self.source;
        if self.source {
            self.source_editor.update(cx, |editor, cx| {
                editor.set_value(self.text.clone(), window, cx);
                editor.set_highlighter("markdown", cx);
                editor.focus(window, cx);
            });
        } else {
            self.visual_editor.update(cx, |editor, cx| {
                editor
                    .set_source(&self.text, cx)
                    .expect("source mode Markdown parses with the configured GPUI extensions");
                editor.focus(window, cx);
            });
        }
        cx.emit(MarkdownEvent::Focus);
        cx.notify();
    }

    fn run_command(&mut self, command: EditorCommand, cx: &mut Context<Self>) {
        self.visual_editor.update(cx, |editor, cx| {
            editor
                .run(command, cx)
                .expect("Markdown toolbar command applies to the current selection");
        });
    }

    fn insert_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title("插入本地图片")
            .add_filter("图片", &["png", "jpg", "jpeg", "webp", "gif", "svg"]);
        let directory = self
            .path
            .parent()
            .expect("Markdown file has parent")
            .to_path_buf();
        let pane = cx.entity();
        cx.spawn_in(window, async move |_, cx| {
            let Some(handle) = picker.pick_file().await else {
                return;
            };
            let source = handle.path().to_path_buf();
            let filename = source
                .file_name()
                .expect("selected image has name")
                .to_owned();
            let destination = directory.join(&filename);
            let _ = pane.update_in(cx, |pane, _, cx| {
                if source != destination && destination.exists() {
                    pane.pending_image = Some((source, destination));
                    cx.notify();
                    return;
                }
                if source != destination
                    && let Err(error) = std::fs::copy(source, &destination)
                {
                    pane.image_error = Some(error.to_string());
                    cx.notify();
                    return;
                }
                pane.insert_image_reference(&filename.to_string_lossy(), cx);
            });
        })
        .detach();
    }

    fn insert_image_reference(&mut self, filename: &str, cx: &mut Context<Self>) {
        let insertion = format!("![图片](<{filename}>)");
        self.visual_editor.update(cx, |editor, cx| {
            let mut range = editor
                .capture_selection()
                .expect("Markdown caret or selection is text");
            editor
                .replace_captured(&mut range, &insertion, cx)
                .expect("captured Markdown selection remains current during insertion");
        });
    }

    fn confirm_image_import(&mut self, cx: &mut Context<Self>) {
        let (source, destination) = self.pending_image.take().expect("prompt has image paths");
        if let Err(error) = std::fs::copy(source, &destination) {
            self.image_error = Some(error.to_string());
        } else {
            let filename = destination
                .file_name()
                .expect("import destination has filename")
                .to_string_lossy();
            self.insert_image_reference(&filename, cx);
        }
        cx.notify();
    }

    fn navigate_heading(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.source {
            let offset = self.headings[index].range.start;
            let line = self.text[..offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            self.source_editor.update(cx, |editor, cx| {
                editor.set_cursor_position(Position::new(line as u32, 0), window, cx)
            });
        } else {
            let headings = self
                .visual_editor
                .read(cx)
                .model()
                .document()
                .leaves()
                .filter(|block| matches!(block.kind(), BlockKind::Heading { .. }))
                .map(|block| block.id())
                .collect::<Vec<_>>();
            let block = headings[index];
            self.visual_editor.update(cx, |editor, cx| {
                editor
                    .set_selection(BlockSelection::caret(BlockPosition::new(block, 0)), cx)
                    .expect("outline heading remains a caret position");
                editor.reveal_caret(cx);
                editor.focus(window, cx);
            });
        }
        cx.notify();
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
                    .child(crate::ui::icon(gpui_kit::assets::IconName::FileText))
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
                    .child(
                        action(
                            "md-font-size",
                            format!("{}px", self.settings.markdown_font_size),
                        )
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.settings.markdown_font_size = 14;
                            cx.emit(MarkdownEvent::Zoom(14));
                            cx.notify();
                        })),
                    )
                    .child(
                        action("md-outline", "☷")
                            .on_click(cx.listener(|pane, _, _, cx| pane.toggle_outline(cx))),
                    )
                    .child(action("md-source", "源码").on_click(
                        cx.listener(|pane, _, window, cx| pane.toggle_source(window, cx)),
                    )),
            );

        let mut content = div().flex().flex_1().min_h_0();
        if self.outline {
            let headings = self.headings.iter().enumerate().map(|(index, heading)| {
                action(("heading", index), heading.title.clone())
                    .pl(px(heading.level as f32 * 12.))
                    .text_xs()
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.navigate_heading(index, window, cx)
                    }))
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
            content = content.child(
                div()
                    .id("markdown-content")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .justify_center()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .text_size(px(self.settings.markdown_font_size as f32))
                    .font_family(if self.settings.interface_font.is_empty() {
                        SharedString::from("Segoe UI")
                    } else {
                        self.settings.interface_font.clone().into()
                    })
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .when(self.settings.line_width_limited, |content| {
                                content.max_w(px(850.))
                            })
                            .p_6()
                            .child(BlockEditor::new(&self.visual_editor)),
                    )
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll)
                                .mode(ScrollbarMode::Always)
                                .styles(|styles| {
                                    styles
                                        .thumb(|style| {
                                            style
                                                .bg(rgb(0x999996))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                        .thumb_hover(|style| {
                                            style
                                                .bg(rgb(0x777774))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                        .thumb_active(|style| {
                                            style
                                                .bg(rgb(0x5d5d5a))
                                                .width(px(10.))
                                                .inset(px(2.))
                                                .radius(px(6.))
                                        })
                                }),
                        ),
                    ),
            );
        }

        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .when(self.reading, |pane| pane.child(toolbar))
            .child(content)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(MarkdownEvent::Focus)),
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
                pane.run_command(EditorCommand::InsertTable { rows: 2, cols: 2 }, cx)
            }))
            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                if event.modifiers.control {
                    let delta = if event.delta.pixel_delta(px(20.)).y > px(0.) {
                        1
                    } else {
                        -1
                    };
                    pane.settings.markdown_font_size =
                        (pane.settings.markdown_font_size as i16 + delta).clamp(11, 26) as u8;
                    pane.math.update(cx, |renderer, cx| {
                        renderer.set_appearance(&pane.settings, cx)
                    });
                    cx.stop_propagation();
                    cx.notify();
                    cx.emit(MarkdownEvent::Zoom(pane.settings.markdown_font_size));
                }
            }))
            .context_menu(|menu, _, _| {
                menu.menu("加粗", Box::new(Bold))
                    .menu("斜体", Box::new(Italic))
                    .menu("标题", Box::new(HeadingAction))
                    .menu("无序列表", Box::new(BulletList))
                    .menu("任务列表", Box::new(TaskList))
                    .separator()
                    .menu("插入图片…", Box::new(InsertImage))
                    .menu("插入表格", Box::new(InsertTable))
            })
            .when_some(self.formula_editor.clone(), |pane, (editor, display)| {
                pane.child(crate::ui::modal_overlay(
                    "编辑公式",
                    "输入 LaTeX 正文，无需添加分隔符。".to_owned(),
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
                                .child(crate::ui::button("cancel-formula", "取消").on_click(
                                    cx.listener(|pane, _, window, cx| {
                                        pane.formula_editor = None;
                                        pane.visual_editor
                                            .update(cx, |editor, cx| editor.focus(window, cx));
                                        cx.notify();
                                    }),
                                ))
                                .child(crate::ui::button("apply-formula", "应用").on_click(
                                    cx.listener(move |pane, _, window, cx| {
                                        let latex = editor.read(cx).value().to_string();
                                        let source = if display {
                                            format!("$$\n{latex}\n$$")
                                        } else {
                                            format!("${latex}$")
                                        };
                                        pane.visual_editor.update(cx, |editor, cx| {
                                            editor.replace_selected_markdown(&source, cx);
                                            editor.focus(window, cx);
                                        });
                                        pane.formula_editor = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ))
            })
            .when_some(self.pending_image.clone(), |pane, (_, destination)| {
                pane.child(crate::ui::modal_overlay(
                    "替换同名图片？",
                    destination.to_string_lossy().into_owned(),
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::ui::button("cancel-image-overwrite", "取消").on_click(
                                cx.listener(|pane, _, _, cx| {
                                    pane.pending_image = None;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            crate::ui::button("confirm-image-overwrite", "替换").on_click(
                                cx.listener(|pane, _, _, cx| pane.confirm_image_import(cx)),
                            ),
                        ),
                ))
            })
            .when_some(self.image_error.clone(), |pane, error| {
                pane.child(crate::ui::modal_overlay(
                    "图片导入失败",
                    error,
                    div().flex().justify_end().child(
                        crate::ui::button("dismiss-image-error", "确定").on_click(cx.listener(
                            |pane, _, _, cx| {
                                pane.image_error = None;
                                cx.notify();
                            },
                        )),
                    ),
                ))
            })
    }
}
