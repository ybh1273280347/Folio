//! PDF lifetime follows the visible workspace pane; the GPUI viewer owns page
//! virtualization, off-thread rendering, zoom anchoring, fit-to-viewport,
//! Chrome-style fluorescent highlights, and box-selection of effective content.
use crate::theme::color as rgb;
use crate::{
    annotations::{self, AnnotationItem, AnnotationRect, HIGHLIGHT_COLORS},
    ui,
};
use gpui::{MouseButton, Subscription};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_pdf::{Highlight, NormRect, PdfStyle, PdfView};
use std::{path::PathBuf, rc::Rc};

pub enum PdfPaneEvent {
    AskAi { path: PathBuf, text: String },
}
impl EventEmitter<PdfPaneEvent> for PdfPane {}

pub struct PdfPane {
    path: PathBuf,
    view: Option<Entity<PdfView>>,
    visible: bool,
    annotations: Vec<AnnotationItem>,
    active_annotation_id: Option<u64>,
    selected_annotation_id: Option<u64>,
    editing_drawer_annotation_id: Option<u64>,
    annotation_list_focus: FocusHandle,
    annotation_comment_input: Entity<TextareaState>,
    drawer_comment_input: Entity<TextareaState>,
    annotations_drawer_open: bool,
    pending_jump_page: Option<usize>,
    translate: crate::translate::TranslatePopover,
    _subscriptions: Vec<Subscription>,
}

impl PdfPane {
    pub fn path(&self) -> &std::path::Path { &self.path }

    pub fn jump_to_page(&mut self, page_1based: usize, cx: &mut Context<Self>) {
        let target = page_1based.saturating_sub(1);
        if let Some(view) = &self.view {
            view.update(cx, |v, cx| v.flash_to_page(target, cx));
        } else {
            self.pending_jump_page = Some(target);
        }
    }

    pub fn sync_annotations(&mut self, items: &[AnnotationItem], cx: &mut Context<Self>) {
        self.annotations = items.to_vec();
        self.sync_highlights(cx);
        cx.notify();
    }
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let annotations = annotations::load_annotations(&path);
        let annotation_comment_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .submit_on_enter(true)
                .placeholder(crate::i18n::media::PDF_ANN_PLACEHOLDER.get())
        });
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
        let drawer_comment_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .submit_on_enter(true)
                .placeholder(crate::i18n::media::PDF_ANN_PLACEHOLDER.get())
        });
        let drawer_sub = cx.subscribe_in(
            &drawer_comment_input,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    if let Some(id) = this.editing_drawer_annotation_id {
                        let comment = state.read(cx).value().trim().to_string();
                        if let Some(item) = this.annotations.iter_mut().find(|a| a.id == id) {
                            item.comment = comment;
                        }
                        let _ = annotations::save_annotations(&this.path, &this.annotations);
                        this.editing_drawer_annotation_id = None;
                        cx.notify();
                    }
                }
            },
        );
        let mut this = Self {
            path,
            view: None,
            visible: true,
            annotations,
            active_annotation_id: None,
            selected_annotation_id: None,
            editing_drawer_annotation_id: None,
            annotation_list_focus: cx.focus_handle(),
            annotation_comment_input,
            drawer_comment_input,
            annotations_drawer_open: false,
            pending_jump_page: None,
            translate: crate::translate::TranslatePopover::default(),
            _subscriptions: vec![sub, drawer_sub],
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
        gpui_kit::rgb(hex).into()
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
            .map(|(id, _label, hex)| (SharedString::from(*id), gpui_kit::rgb(*hex).into()))
            .collect();
        let weak = cx.entity().downgrade();
        let weak_click = weak.clone();
        let weak_drawer = weak.clone();
        let weak_translate = weak.clone();
        let weak_ask_ai = weak.clone();
        let pending_jump = self.pending_jump_page;
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
                            this.selected_annotation_id = Some(id);
                            this.open_annotation_card(id, false, window, cx);
                        });
                    }
                });
            }));
            v.set_on_toggle_annotations(Rc::new(move |window, cx| {
                let weak_drawer = weak_drawer.clone();
                window.defer(cx, move |_, cx| {
                    if let Some(pane) = weak_drawer.upgrade() {
                        pane.update(cx, |pane, cx| {
                            pane.annotations_drawer_open = !pane.annotations_drawer_open;
                            cx.notify();
                        });
                    }
                });
            }));
            v.set_on_translate(Rc::new(move |text, window, cx| {
                let weak_translate = weak_translate.clone();
                window.defer(cx, move |_, cx| {
                    if let Some(pane) = weak_translate.upgrade() {
                        pane.update(cx, |pane, cx| {
                            crate::translate::TranslatePopover::open_and_translate(
                                pane,
                                text.clone(),
                                None,
                                cx,
                                |this| &mut this.translate,
                            );
                        });
                    }
                });
            }));
            v.set_on_ask_ai(Rc::new(move |text, window, cx| {
                let weak_ask_ai = weak_ask_ai.clone();
                window.defer(cx, move |_, cx| {
                    if let Some(pane) = weak_ask_ai.upgrade() {
                        pane.update(cx, |pane, cx| {
                            let path = pane.path.clone();
                            cx.emit(PdfPaneEvent::AskAi { path, text });
                        });
                    }
                });
            }));
            v.set_highlights(highlights, cx);
            if let Some(page) = pending_jump {
                v.flash_to_page(page, cx);
            }
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
        let _ = annotations::save_annotations(&self.path, &self.annotations);
        self.sync_highlights(cx);
        if open_comment {
            self.open_annotation_card(id, true, window, cx);
        } else {
            cx.notify();
        }
    }

    fn open_annotation_card(&mut self, id: u64, focus_comment: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.annotations.iter().find(|a| a.id == id) {
            let comment = item.comment.clone();
            self.active_annotation_id = Some(id);
            let input = self.annotation_comment_input.clone();
            let list_focus = self.annotation_list_focus.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |input, cx| {
                    input.set_value(comment, window, cx);
                    if focus_comment { input.focus(window, cx); }
                });
                if !focus_comment { list_focus.focus(window, cx); }
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
        let _ = annotations::save_annotations(&self.path, &self.annotations);
            cx.notify();
        }
    }

    fn set_annotation_color(&mut self, id: u64, color: &str, cx: &mut Context<Self>) {
        if let Some(item) = self.annotations.iter_mut().find(|a| a.id == id) {
            item.color = color.to_string();
        let _ = annotations::save_annotations(&self.path, &self.annotations);
            self.sync_highlights(cx);
            cx.notify();
        }
    }

    fn delete_annotation(&mut self, id: u64, cx: &mut Context<Self>) {
        self.annotations.retain(|a| a.id != id);
        if self.active_annotation_id == Some(id) {
            self.active_annotation_id = None;
        }
        if self.selected_annotation_id == Some(id) { self.selected_annotation_id = None; }
        if self.editing_drawer_annotation_id == Some(id) {
            self.editing_drawer_annotation_id = None;
        }
        annotations::save_annotations(&self.path, &self.annotations).expect("failed to save annotation deletion");
        self.sync_highlights(cx);
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.annotations = annotations::load_annotations(&self.path);
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

    pub fn refresh_locale(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.annotation_comment_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::media::PDF_ANN_PLACEHOLDER.get(), window, cx);
        });
        self.drawer_comment_input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::media::PDF_ANN_PLACEHOLDER.get(), window, cx);
        });
    }

    fn render_active_annotation_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active_id = self.active_annotation_id?;
        let item = self.annotations.iter().find(|a| a.id == active_id)?;
        let quote_preview = if item.quote.is_empty() {
            crate::i18n::media::pdf_ann_page_region(item.page + 1)
        } else if item.quote.chars().count() > 60 {
            let s: String = item.quote.chars().take(60).collect();
            crate::i18n::media::pdf_ann_page_quote(item.page + 1, &s, true)
        } else {
            crate::i18n::media::pdf_ann_page_quote(item.page + 1, &item.quote, false)
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
                                            .bg(gpui_kit::rgb(*hex))
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
                                    ui::button_danger_soft("pdf-ann-delete", crate::i18n::common::DELETE.get())
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
                .child(Textarea::new(&self.annotation_comment_input).w_full().text_size(px(12.0)))
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
                .w_full()
                .min_w_0()
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
                                .child(crate::i18n::media::pdf_ann_count(pdf_annotations.len())),
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
                                    .child(crate::i18n::media::PDF_ANN_EMPTY.get()),
                            )
                        })
                        .children(pdf_annotations.iter().enumerate().map(|(idx, item)| {
                            let id = item.id;
                            let page = item.page;
                            let quote = if item.quote.is_empty() {
                                crate::i18n::media::PDF_ANN_REGION_LABEL.get().to_string()
                            } else {
                                item.quote.clone()
                            };
                            let comment = item.comment.clone();
                            let active_color = item.color.clone();
                            let is_editing = self.editing_drawer_annotation_id == Some(id);
                            let color_hex = HIGHLIGHT_COLORS
                                .iter()
                                .find(|(cid, _, _)| *cid == item.color)
                                .map(|(_, _, hex)| *hex)
                                .unwrap_or(0xfde047);
                            let card_bg = Hsla::from(rgb(color_hex)).alpha(
                                if self.selected_annotation_id == Some(id) || is_editing {
                                    0.14
                                } else {
                                    0.08
                                },
                            );
                            let card_border = Hsla::from(rgb(color_hex)).alpha(
                                if self.selected_annotation_id == Some(id) || is_editing {
                                    0.65
                                } else {
                                    0.32
                                },
                            );
                            let drawer_swatches = HIGHLIGHT_COLORS.iter().enumerate().map(
                                |(cidx, (color_id, _label, hex))| {
                                    let is_current = active_color == *color_id;
                                    let cid = (*color_id).to_string();
                                    div()
                                        .id(("pdf-drawer-ann-color", idx * 10 + cidx))
                                        .size(px(14.))
                                        .rounded(px(7.))
                                        .bg(gpui_kit::rgb(*hex))
                                        .border_1()
                                        .border_color(if is_current {
                                            rgb(0x37352f)
                                        } else {
                                            rgb(0xd8d6d0)
                                        })
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.set_annotation_color(id, &cid, cx);
                                        }))
                                },
                            );
                            div()
                                .id(("pdf-ann-item", idx))
                                .flex()
                                .flex_col()
                                .p(px(8.))
                                .gap(px(6.))
                                .rounded(px(8.))
                                .bg(card_bg)
                                .border_1()
                                .border_color(card_border)
                                .cursor_pointer()
                                .hover(|s| s.border_color(Hsla::from(rgb(color_hex)).alpha(0.65)))
                                .active(|s| s.border_color(Hsla::from(rgb(color_hex)).alpha(0.85)))
                                .on_click(cx.listener({
                                    let comment_for_click = comment.clone();
                                    move |this, _, window, cx| {
                                        if let Some(view) = &this.view {
                                            view.update(cx, |v, cx| v.reveal_highlight(page, cx));
                                        }
                                        let comment_cl = comment_for_click.clone();
                                    let input = this.drawer_comment_input.clone();
                                    this.active_annotation_id = None;
                                    this.selected_annotation_id = Some(id);
                                    this.editing_drawer_annotation_id = Some(id);
                                    window.defer(cx, move |window, cx| {
                                        input.update(cx, |input, cx| {
                                            input.set_value(comment_cl, window, cx);
                                            input.focus(window, cx);
                                        });
                                    });
                                    cx.notify();
                                    }
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
                                                        .text_color(rgb(0x64748b))
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
                                                .active(|s| s.text_color(rgb(0x8a1c13)))
                                                .child("×")
                                                .on_click(cx.listener(
                                                    move |this, _, _window, cx| {
                                                        cx.stop_propagation();
                                                        this.delete_annotation(id, cx);
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
                                            .gap(px(6.))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_key_down(cx.listener(
                                                |this, ev: &KeyDownEvent, window, cx| {
                                                    if ev.keystroke.key == "escape" {
                                                        this.editing_drawer_annotation_id = None;
                                                        this.annotation_list_focus.focus(window, cx);
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    }
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(5.))
                                                    .children(drawer_swatches),
                                            )
                                            .child(
                                                Textarea::new(&self.drawer_comment_input)
                                                    .w_full()
                                                    .text_size(px(12.0)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_end()
                                                    .gap(px(6.))
                                                    .child(
                                                        ui::button("cancel-pdf-drawer-ann", crate::i18n::common::CANCEL.get())
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.editing_drawer_annotation_id =
                                                                        None;
                                                                    this.annotation_list_focus
                                                                        .focus(window, cx);
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        ui::button_primary(
                                                            "save-pdf-drawer-ann",
                                                            crate::i18n::common::SAVE.get(),
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, window, cx| {
                                                                let next_comment = this
                                                                    .drawer_comment_input
                                                                    .read(cx)
                                                                    .value()
                                                                    .trim()
                                                                    .to_string();
                                                                if let Some(a) = this
                                                                    .annotations
                                                                    .iter_mut()
                                                                    .find(|a| a.id == id)
                                                                {
                                                                    a.comment = next_comment;
                                                                }
                                                                let _ = annotations::save_annotations(
                                                                    &this.path,
                                                                    &this.annotations,
                                                                );
                                                                this.editing_drawer_annotation_id =
                                                                    None;
                                                                this.annotation_list_focus
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
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x334155))
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
        let viewer = div()
            .flex()
            .relative()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(0xf1f1ef))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .when_some(self.view.clone(), |area, view| area.child(view))
                    .children(self.render_active_annotation_popover(cx))
                    .children(self.translate.render(
                        "pdf-translate",
                        None,
                        None,
                        None,
                        cx,
                        |this| &mut this.translate,
                    )),
            )
            ;
        let content = if let Some(drawer) = self.render_annotations_drawer(cx) {
            h_resizable("pdf-annotations-layout")
                .child(resizable_panel().size_range(px(200.)..Pixels::MAX).child(viewer))
                .child(resizable_panel().size(px(268.)).size_range(px(160.)..px(700.)).fixed().child(drawer))
                .into_any_element()
        } else {
            viewer.into_any_element()
        };
        div().size_full().track_focus(&self.annotation_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.translate.is_open() {
                    this.translate.dismiss();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.translate.is_open() {
                    this.translate.dismiss();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                // The comment input has its own focus, so deletion keys there still edit text.
                if matches!(event.keystroke.key.as_str(), "delete" | "backspace")
                    && this.editing_drawer_annotation_id.is_none()
                    && this.annotation_list_focus.is_focused(window)
                {
                    if let Some(id) = this.selected_annotation_id {
                        this.delete_annotation(id, cx);
                        cx.stop_propagation();
                    }
                }
            }))
            .child(content)
    }
}
