//! PDF lifetime follows the visible workspace pane; the GPUI viewer owns page
//! virtualization, off-thread rendering, zoom anchoring, fit-to-viewport,
//! Chrome-style fluorescent highlights, and box-selection of effective content.
use crate::{
    model::{AnnotationItem, AnnotationRect, HIGHLIGHT_COLORS},
    storage, ui,
};
use gpui::{MouseButton, Subscription};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_pdf::{Highlight, NormRect, PdfStyle, PdfView};
use std::{path::PathBuf, rc::Rc};

pub struct PdfPane {
    path: PathBuf,
    view: Option<Entity<PdfView>>,
    visible: bool,
    annotations: Vec<AnnotationItem>,
    active_annotation_id: Option<u64>,
    annotation_comment_input: Entity<InputState>,
    annotations_drawer_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl PdfPane {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let annotations = storage::load_annotations(&path);
        let annotation_comment_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("输入 PDF 批注内容（回车保存）…"));
        let sub = cx.subscribe_in(
            &annotation_comment_input,
            window,
            |this, _, event: &InputEvent, _window, cx| match event {
                InputEvent::Change => {
                    this.save_active_annotation_comment(cx);
                }
                InputEvent::PressEnter { .. } => {
                    this.save_active_annotation_comment(cx);
                    this.active_annotation_id = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        let mut this = Self {
            path,
            view: None,
            visible: true,
            annotations,
            active_annotation_id: None,
            annotation_comment_input,
            annotations_drawer_open: false,
            _subscriptions: vec![sub],
        };
        this.view = Some(this.open_view(window, cx));
        this
    }

    fn color_hsla(name: &str) -> Hsla {
        let hex = HIGHLIGHT_COLORS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, _, h)| *h)
            .unwrap_or(0xfacc15);
        rgb(hex).into()
    }

    fn build_highlights(&self) -> Vec<Highlight> {
        self.annotations
            .iter()
            .map(|a| {
                let rects: Vec<NormRect> = a
                    .rects
                    .iter()
                    .map(|r| NormRect {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                    })
                    .collect();
                let region = a.region.map(|r| NormRect {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                });
                Highlight {
                    id: a.id,
                    page: a.page,
                    quote: a.quote.clone(),
                    occurrence: a.occurrence,
                    color: Self::color_hsla(&a.color),
                    region,
                    rects,
                }
            })
            .collect()
    }

    fn sync_highlights(&self, cx: &mut Context<Self>) {
        let Some(view) = &self.view else {
            return;
        };
        let highlights = self.build_highlights();
        view.update(cx, |v, cx| v.set_highlights(highlights, cx));
    }

    fn open_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PdfView> {
        let style = Rc::new(|| PdfStyle {
            bg: rgb(0xf1f1ef).into(),
            border: rgb(0xe8e7e3).into(),
            placeholder_bg: rgb(0xffffff).into(),
            placeholder_fg: rgb(0x787774).into(),
            header_fg: rgb(0x37352f).into(),
            header_muted: rgb(0x787774).into(),
        });
        let path = self.path.clone();
        let view = cx.new(|cx| PdfView::new(path, style, Rc::new(|| 1.), window, cx));
        let palette: Vec<(SharedString, Hsla)> = HIGHLIGHT_COLORS
            .iter()
            .map(|(id, _label, hex)| (SharedString::from(*id), rgb(*hex).into()))
            .collect();
        let weak = cx.entity().downgrade();
        let weak_click = weak.clone();
        let highlights = self.build_highlights();
        view.update(cx, |v, cx| {
            v.fit_width(cx);
            v.set_highlight_palette(palette, cx);
            v.set_on_create_annotation(Rc::new(
                move |page, quote, occurrence, rects, is_box, color, open_comment, window, cx| {
                    let weak = weak.clone();
                    let color = color.to_string();
                    window.defer(cx, move |window, cx| {
                        if let Some(pane) = weak.upgrade() {
                            pane.update(cx, |this, cx| {
                                this.on_pdf_create_annotation(
                                    page,
                                    quote,
                                    occurrence,
                                    rects,
                                    is_box,
                                    color,
                                    open_comment,
                                    window,
                                    cx,
                                );
                            });
                        }
                    });
                },
            ));
            v.set_on_highlight(Rc::new(move |id, window, cx| {
                let weak_click = weak_click.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(pane) = weak_click.upgrade() {
                        pane.update(cx, |this, cx| {
                            this.open_annotation_card(id, window, cx);
                        });
                    }
                });
            }));
            v.set_highlights(highlights, cx);
        });
        view
    }

    fn on_pdf_create_annotation(
        &mut self,
        page: usize,
        quote: String,
        occurrence: usize,
        rects: Vec<NormRect>,
        is_box: bool,
        color: String,
        open_comment: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1);
        let mapped_rects: Vec<AnnotationRect> = rects
            .into_iter()
            .map(|r| AnnotationRect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
            })
            .collect();
        let region = (is_box && mapped_rects.len() == 1).then(|| mapped_rects[0]);
        let item = AnnotationItem {
            id,
            file: String::new(),
            page,
            quote,
            occurrence,
            color: if color.is_empty() {
                "yellow".to_string()
            } else {
                color
            },
            comment: String::new(),
            region,
            rects: mapped_rects,
        };
        self.annotations.push(item);
        let _ = storage::save_annotations(&self.path, &self.annotations);
        self.sync_highlights(cx);
        if open_comment {
            self.open_annotation_card(id, window, cx);
        } else {
            cx.notify();
        }
    }

    fn open_annotation_card(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.annotations.iter().find(|a| a.id == id) {
            let comment = item.comment.clone();
            self.active_annotation_id = Some(id);
            let input = self.annotation_comment_input.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |input, cx| {
                    input.set_value(comment, window, cx);
                    input.focus(window, cx);
                });
            });
            cx.notify();
        }
    }

    fn save_active_annotation_comment(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_annotation_id else {
            return;
        };
        let comment = self.annotation_comment_input.read(cx).value().to_string();
        if let Some(item) = self.annotations.iter_mut().find(|a| a.id == id) {
            item.comment = comment;
            let _ = storage::save_annotations(&self.path, &self.annotations);
            cx.notify();
        }
    }

    fn set_annotation_color(&mut self, id: u64, color: &str, cx: &mut Context<Self>) {
        if let Some(item) = self.annotations.iter_mut().find(|a| a.id == id) {
            item.color = color.to_string();
            let _ = storage::save_annotations(&self.path, &self.annotations);
            self.sync_highlights(cx);
            cx.notify();
        }
    }

    fn delete_annotation(&mut self, id: u64, cx: &mut Context<Self>) {
        self.annotations.retain(|a| a.id != id);
        if self.active_annotation_id == Some(id) {
            self.active_annotation_id = None;
        }
        let _ = storage::save_annotations(&self.path, &self.annotations);
        self.sync_highlights(cx);
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.annotations = storage::load_annotations(&self.path);
            self.view = Some(self.open_view(window, cx));
        } else if let Some(view) = self.view.take() {
            view.update(cx, |view, cx| view.release(window, cx));
        }
    }

    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.view.take() {
            view.update(cx, |view, cx| view.release(window, cx));
        }
    }

    fn render_active_annotation_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active_id = self.active_annotation_id?;
        let item = self.annotations.iter().find(|a| a.id == active_id)?;
        let quote_preview = if item.quote.is_empty() {
            format!("第 {} 页 · 框选区域批注", item.page + 1)
        } else if item.quote.chars().count() > 60 {
            let s: String = item.quote.chars().take(60).collect();
            format!("第 {} 页 · “{s}…”", item.page + 1)
        } else {
            format!("第 {} 页 · “{}”", item.page + 1, item.quote)
        };
        let active_color = item.color.clone();

        Some(
            div()
                .id("pdf-active-annotation-card")
                .absolute()
                .top(px(44.))
                .right(px(20.))
                .w(px(310.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0xe8e7e3))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(8.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .gap(px(5.))
                                .items_center()
                                .children(HIGHLIGHT_COLORS.iter().enumerate().map(
                                    |(idx, (color_id, _label, hex))| {
                                        let is_current = active_color == *color_id;
                                        let cid = (*color_id).to_string();
                                        div()
                                            .id(("pdf-ann-color", idx))
                                            .size(px(16.))
                                            .rounded(px(8.))
                                            .bg(rgb(*hex))
                                            .border_1()
                                            .border_color(if is_current {
                                                rgb(0x37352f)
                                            } else {
                                                rgb(0xd8d6d0)
                                            })
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.set_annotation_color(active_id, &cid, cx);
                                            }))
                                    },
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .child(
                                    ui::button("pdf-ann-delete", "删除")
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.delete_annotation(active_id, cx);
                                        })),
                                )
                                .child(
                                    ui::button("pdf-ann-close", "×")
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.active_annotation_id = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(0x787774))
                        .child(quote_preview),
                )
                .child(Input::new(&self.annotation_comment_input).bordered(true))
                .into_any_element(),
        )
    }

    fn render_annotations_drawer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.annotations_drawer_open {
            return None;
        }
        let pdf_annotations: Vec<&AnnotationItem> = self.annotations.iter().collect();
        Some(
            div()
                .id("pdf-annotations-drawer")
                .flex()
                .flex_col()
                .w(px(268.))
                .h_full()
                .flex_shrink_0()
                .bg(rgb(0xfafaf9))
                .border_l_1()
                .border_color(rgb(0xe8e7e3))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .flex()
                        .h(px(34.))
                        .px(px(12.))
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(rgb(0xe8e7e3))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0x37352f))
                                .child(format!("PDF 批注 ({})", pdf_annotations.len())),
                        )
                        .child(
                            ui::button("pdf-ann-drawer-close", "×").on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.annotations_drawer_open = false;
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .id("pdf-ann-drawer-list")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .p(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .when(pdf_annotations.is_empty(), |list| {
                            list.child(
                                div()
                                    .p(px(12.))
                                    .text_size(px(12.))
                                    .text_color(rgb(0x9b9a97))
                                    .child("暂无批注。直接拖拽选中 PDF 文字，或点击顶部 ⬚（或按住 Alt 拖拽框选）即可高亮与批注区域内的有效内容。"),
                            )
                        })
                        .children(pdf_annotations.iter().enumerate().map(|(idx, item)| {
                            let id = item.id;
                            let page = item.page;
                            let quote = if item.quote.is_empty() {
                                "框选区域".to_string()
                            } else {
                                item.quote.clone()
                            };
                            let comment = item.comment.clone();
                            let color_hex = HIGHLIGHT_COLORS
                                .iter()
                                .find(|(cid, _, _)| *cid == item.color)
                                .map(|(_, _, hex)| *hex)
                                .unwrap_or(0xfde047);
                            div()
                                .id(("pdf-ann-item", idx))
                                .flex()
                                .flex_col()
                                .p(px(8.))
                                .gap(px(4.))
                                .rounded(px(6.))
                                .bg(rgb(0xffffff))
                                .border_1()
                                .border_color(rgb(0xe8e7e3))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0xf4f3ef)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(view) = &this.view {
                                        view.update(cx, |v, cx| v.reveal_highlight(page, cx));
                                    }
                                    this.open_annotation_card(id, window, cx);
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.))
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .gap(px(6.))
                                                .items_center()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .child(
                                                    div()
                                                        .size(px(8.))
                                                        .rounded(px(4.))
                                                        .bg(rgb(color_hex))
                                                        .flex_shrink_0(),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.5))
                                                        .text_color(rgb(0x787774))
                                                        .truncate()
                                                        .child(format!("P{} · {quote}", page + 1)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(("pdf-ann-del", idx))
                                                .px(px(4.))
                                                .text_size(px(11.))
                                                .text_color(rgb(0x9b9a97))
                                                .hover(|s| s.text_color(rgb(0xb42318)))
                                                .child("×")
                                                .on_click(cx.listener(
                                                    move |this, _, _window, cx| {
                                                        cx.stop_propagation();
                                                        this.delete_annotation(id, cx);
                                                    },
                                                )),
                                        ),
                                )
                                .when(!comment.is_empty(), |card| {
                                    card.child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(rgb(0x37352f))
                                            .child(comment),
                                    )
                                })
                        })),
                )
                .into_any_element(),
        )
    }
}

impl Render for PdfPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ann_count = self.annotations.len();
        div()
            .flex()
            .relative()
            .size_full()
            .bg(rgb(0xf1f1ef))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .when_some(self.view.clone(), |area, view| area.child(view))
                    .child(
                        div()
                            .id("pdf-annotations-drawer-btn")
                            .absolute()
                            .top(px(5.))
                            .right(px(52.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .bg(if self.annotations_drawer_open {
                                rgb(0xe8e7e3)
                            } else {
                                rgb(0xf1f1ef)
                            })
                            .hover(|s| s.bg(rgb(0xe8e7e3)))
                            .text_size(px(11.5))
                            .text_color(rgb(0x37352f))
                            .child(if ann_count > 0 {
                                format!("💬 {ann_count}")
                            } else {
                                "💬".to_string()
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.annotations_drawer_open = !this.annotations_drawer_open;
                                cx.notify();
                            })),
                    )
                    .children(self.render_active_annotation_popover(cx)),
            )
            .children(self.render_annotations_drawer(cx))
    }
}
