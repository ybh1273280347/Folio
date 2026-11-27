use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui_kit::{
    Bounds, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollEvent, Window,
    assets::IconName,
    canvas,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        tooltip::Tooltip,
    },
    div, img, point,
    prelude::FluentBuilder,
    px, *,
};

const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 16.0;
const ZOOM_STEP: f32 = 1.25;
const WHEEL_ZOOM_STEP: f32 = 1.12;
const FIT_PADDING: f32 = 96.0;
const CARD_PADDING: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomMode {
    Fit,
    Custom(f32),
}

pub struct LightboxClose;

pub struct SvgLightbox {
    svg_tree: Arc<usvg::Tree>,
    initial_image: Arc<RenderImage>,
    rendered: Option<Arc<RenderImage>>,
    requested_density: Option<f32>,
    raster_task: Option<Task<()>>,
    zoom_mode: ZoomMode,
    pan: Point<Pixels>,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag_state: Option<(Point<Pixels>, Point<Pixels>)>,
    focus_handle: FocusHandle,
    previous_focus: Option<WeakFocusHandle>,
}

impl EventEmitter<LightboxClose> for SvgLightbox {}

impl SvgLightbox {
    pub fn new(
        svg_tree: Arc<usvg::Tree>,
        initial_image: Arc<RenderImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let previous_focus = window.focused(cx).map(|focus| focus.downgrade());
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        Self {
            svg_tree,
            initial_image,
            rendered: None,
            requested_density: None,
            raster_task: None,
            zoom_mode: ZoomMode::Fit,
            pan: Point::default(),
            viewport_bounds: Rc::new(Cell::new(Bounds {
                origin: Point::default(),
                size: window.viewport_size(),
            })),
            drag_state: None,
            focus_handle,
            previous_focus,
        }
    }

    pub(crate) fn release(&mut self, window: &mut Window, cx: &mut App) {
        self.raster_task = None;
        if let Some(image) = self.rendered.take() {
            window.drop_image(image).expect("remove lightbox image from atlas");
        }
        if let Some(focus) = self.previous_focus.as_ref().and_then(WeakFocusHandle::upgrade) {
            focus.focus(window, cx);
        }
    }

    fn fit_scale(&self) -> f32 {
        let size = self.svg_tree.size();
        let bounds = self.viewport_bounds.get();
        let vw = f32::from(bounds.size.width);
        let vh = f32::from(bounds.size.height);
        if vw <= 1.0 || vh <= 1.0 {
            return 1.0;
        }
        let avail_w = (vw - FIT_PADDING - CARD_PADDING * 2.0).max(64.0);
        let avail_h = (vh - FIT_PADDING - CARD_PADDING * 2.0).max(64.0);
        (avail_w / size.width()).min(avail_h / size.height()).clamp(MIN_SCALE, 4.0)
    }

    fn effective_scale(&self) -> f32 {
        match self.zoom_mode {
            ZoomMode::Fit => self.fit_scale(),
            ZoomMode::Custom(scale) => scale.clamp(MIN_SCALE, MAX_SCALE),
        }
    }

    fn card_rect(&self, scale: f32) -> Option<Bounds<Pixels>> {
        let size = self.svg_tree.size();
        let bounds = self.viewport_bounds.get();
        let vw = f32::from(bounds.size.width);
        let vh = f32::from(bounds.size.height);
        if vw <= 1.0 || vh <= 1.0 {
            return None;
        }
        let card_w = size.width() * scale + CARD_PADDING * 2.0;
        let card_h = size.height() * scale + CARD_PADDING * 2.0;
        let left = f32::from(bounds.origin.x) + (vw - card_w) * 0.5 + f32::from(self.pan.x);
        let top = f32::from(bounds.origin.y) + (vh - card_h) * 0.5 + f32::from(self.pan.y);
        Some(Bounds {
            origin: point(px(left), px(top)),
            size: Size {
                width: px(card_w),
                height: px(card_h),
            },
        })
    }

    fn clamp_pan(&self, pan: Point<Pixels>, scale: f32) -> Point<Pixels> {
        let size = self.svg_tree.size();
        let bounds = self.viewport_bounds.get();
        let vw = f32::from(bounds.size.width);
        let vh = f32::from(bounds.size.height);
        if vw <= 1.0 || vh <= 1.0 {
            return pan;
        }
        let rw = size.width() * scale + CARD_PADDING * 2.0;
        let rh = size.height() * scale + CARD_PADDING * 2.0;
        let keep_x = (rw * 0.5).min(64.0);
        let keep_y = (rh * 0.5).min(64.0);
        let limit_x = ((vw + rw) * 0.5 - keep_x).max(0.0);
        let limit_y = ((vh + rh) * 0.5 - keep_y).max(0.0);
        point(
            px(f32::from(pan.x).clamp(-limit_x, limit_x)),
            px(f32::from(pan.y).clamp(-limit_y, limit_y)),
        )
    }

    fn zoom_to_at(
        &mut self,
        new_scale: f32,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let old_scale = self.effective_scale();
        let new_scale = new_scale.clamp(MIN_SCALE, MAX_SCALE);
        if (new_scale - old_scale).abs() < 0.0001 && matches!(self.zoom_mode, ZoomMode::Custom(_)) {
            return;
        }
        let bounds = self.viewport_bounds.get();
        let new_pan = if let Some(pos) = anchor
            && f32::from(bounds.size.width) > 1.0
            && f32::from(bounds.size.height) > 1.0
        {
            let center = bounds.center();
            let cursor_rel = point(pos.x - center.x, pos.y - center.y);
            let ratio = new_scale / old_scale.max(0.001);
            point(
                cursor_rel.x - (cursor_rel.x - self.pan.x) * ratio,
                cursor_rel.y - (cursor_rel.y - self.pan.y) * ratio,
            )
        } else {
            let ratio = new_scale / old_scale.max(0.001);
            point(self.pan.x * ratio, self.pan.y * ratio)
        };
        self.zoom_mode = ZoomMode::Custom(new_scale);
        self.pan = self.clamp_pan(new_pan, new_scale);
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let next = self.effective_scale() * factor;
        self.zoom_to_at(next, anchor, cx);
    }

    fn reset_fit(&mut self, cx: &mut Context<Self>) {
        self.zoom_mode = ZoomMode::Fit;
        self.pan = Point::default();
        cx.notify();
    }

    fn set_actual_size(&mut self, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if anchor.is_some() {
            self.zoom_to_at(1.0, anchor, cx);
        } else {
            self.zoom_mode = ZoomMode::Custom(1.0);
            self.pan = Point::default();
            cx.notify();
        }
    }

    fn ensure_svg_rendered(&mut self, display_scale: f32, window: &Window, cx: &mut Context<Self>) {
        let bucket = (display_scale.clamp(0.25, 8.0) * 4.0).ceil() / 4.0;
        let density = crate::svg::bounded_raster_density(&self.svg_tree, bucket);
        if self.raster_task.is_some() || self.requested_density == Some(density) {
            return;
        }
        // Keep one raster job in flight. The completion redraw picks up the latest zoom.
        self.requested_density = Some(density);
        let tree = Arc::clone(&self.svg_tree);
        self.raster_task = Some(cx.spawn_in(window, async move |view, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move { crate::svg::rasterize_tree(&tree, density) })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.raster_task = None;
                if let Some(image) = rendered
                    && let Some(previous) = view.rendered.replace(image)
                {
                    window.drop_image(previous).expect("remove replaced lightbox image from atlas");
                }
                cx.notify();
            });
        }));
    }

    fn handle_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let steps = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
            ScrollDelta::Lines(lines) => lines.y,
        };
        if steps == 0.0 {
            return;
        }
        let factor = WHEEL_ZOOM_STEP.powf(steps);
        self.zoom_by(factor, Some(event.position), cx);
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.focus_handle.focus(window, cx);
        let scale = self.effective_scale();
        let hit_card = self
            .card_rect(scale)
            .is_some_and(|rect| rect.contains(&event.position));
        if !hit_card {
            cx.emit(LightboxClose);
            return;
        }
        if event.click_count >= 2 {
            match self.zoom_mode {
                ZoomMode::Fit => self.set_actual_size(Some(event.position), cx),
                ZoomMode::Custom(_) => self.reset_fit(cx),
            }
            return;
        }
        self.drag_state = Some((event.position, self.pan));
        cx.notify();
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((start_pos, start_pan)) = self.drag_state else {
            return;
        };
        if !event.dragging() {
            self.drag_state = None;
            cx.notify();
            return;
        }
        let scale = self.effective_scale();
        let next_pan = point(
            start_pan.x + (event.position.x - start_pos.x),
            start_pan.y + (event.position.y - start_pos.y),
        );
        self.pan = self.clamp_pan(next_pan, scale);
        cx.notify();
    }

    fn handle_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.drag_state.take().is_some() {
            cx.notify();
        }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                cx.emit(LightboxClose);
            }
            "=" | "+" => {
                cx.stop_propagation();
                self.zoom_by(ZOOM_STEP, None, cx);
            }
            "-" | "_" => {
                cx.stop_propagation();
                self.zoom_by(1.0 / ZOOM_STEP, None, cx);
            }
            "0" => {
                cx.stop_propagation();
                self.reset_fit(cx);
            }
            "1" => {
                cx.stop_propagation();
                self.set_actual_size(None, cx);
            }
            _ => {}
        }
    }
}

impl Focusable for SvgLightbox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SvgLightbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.effective_scale();
        let clamped_pan = self.clamp_pan(self.pan, scale);
        if clamped_pan != self.pan {
            self.pan = clamped_pan;
        }
        self.ensure_svg_rendered(scale * window.scale_factor(), window, cx);

        let bounds_cell = self.viewport_bounds.clone();
        let dragging = self.drag_state.is_some();
        let percent = (scale * 100.0).round() as u32;
        let is_fit = matches!(self.zoom_mode, ZoomMode::Fit);
        let is_actual = !is_fit && percent == 100;
        let palette = crate::theme::palette();
        let backdrop = if palette.is_dark() {
            rgba(0x09090bcc)
        } else {
            rgba(0x18181bb3)
        };

        let mut stage = div()
            .id("svg-lightbox-stage")
            .occlude()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(backdrop)
            .key_context("FolioSvgLightbox")
            .track_focus(&self.focus_handle)
            .on_scroll(cx.listener(|view, event: &ScrollEvent, _, cx| {
                view.handle_scroll(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.handle_mouse_down(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                view.handle_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.handle_mouse_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.handle_mouse_up(cx);
                }),
            )
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                view.handle_key_down(event, cx);
            }))
            .child(
                canvas(
                    move |bounds, window, _| {
                        if bounds_cell.replace(bounds) != bounds {
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        let card = {
            let size = self.svg_tree.size();
            let bounds = self.viewport_bounds.get();
            let vw = f32::from(bounds.size.width);
            let vh = f32::from(bounds.size.height);
            let drawn_w = size.width() * scale;
            let drawn_h = size.height() * scale;
            let card_w = drawn_w + CARD_PADDING * 2.0;
            let card_h = drawn_h + CARD_PADDING * 2.0;
            let left = (vw - card_w) * 0.5 + f32::from(self.pan.x);
            let top = (vh - card_h) * 0.5 + f32::from(self.pan.y);

            let rendered = self.rendered.as_ref().unwrap_or(&self.initial_image);
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(card_w))
                .h(px(card_h))
                .p(px(CARD_PADDING))
                .rounded(px(10.))
                .bg(crate::theme::color(palette.surface))
                .border_1()
                .border_color(crate::theme::color(palette.border))
                .shadow(palette.modal_shadow())
                .cursor(if dragging {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OpenHand
                })
                .child(
                    img(rendered.clone())
                        .w(px(drawn_w))
                        .h(px(drawn_h))
                        .object_fit(ObjectFit::Fill),
                )
        };

        // Top-right close button
        stage = stage.child(card).child(
            div()
                .id("svg-lightbox-close-wrap")
                .absolute()
                .top_4()
                .right_4()
                .rounded(px(8.))
                .bg(crate::theme::color(palette.surface))
                .border_1()
                .border_color(crate::theme::color(palette.border))
                .shadow_md()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("svg-lightbox-close")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(28.))
                        .w(px(28.))
                        .icon(crate::ui::icon(IconName::Close))
                        .tooltip(crate::i18n::media::LIGHTBOX_CLOSE.get())
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(LightboxClose);
                        })),
                ),
        );

        let hud = {
            div()
                .id("svg-lightbox-hud")
                .absolute()
                .bottom_4()
                .right_4()
                .h(px(30.))
                .px_1p5()
                .rounded(px(8.))
                .bg(crate::theme::color(palette.surface))
                .border_1()
                .border_color(crate::theme::color(palette.border))
                .shadow_md()
                .flex()
                .items_center()
                .gap_1()
                .cursor_default()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("lightbox-zoom-out")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Minus))
                        .tooltip(crate::i18n::media::IMG_TOOLTIP_ZOOM_OUT)
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.zoom_by(1.0 / ZOOM_STEP, None, cx);
                        })),
                )
                .child(
                    div()
                        .id("lightbox-zoom-fit")
                        .px_1p5()
                        .h(px(22.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(crate::theme::color(palette.muted))
                        .hover(|s| s.bg(crate::theme::color(palette.hover)))
                        .tooltip(|window, cx| {
                            Tooltip::new(crate::i18n::media::IMG_TOOLTIP_FIT.get())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.reset_fit(cx);
                        }))
                        .child(format!("{percent}%")),
                )
                .child(
                    Button::new("lightbox-zoom-in")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Plus))
                        .tooltip(crate::i18n::media::IMG_TOOLTIP_ZOOM_IN)
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.zoom_by(ZOOM_STEP, None, cx);
                        })),
                )
                .child(
                    div()
                        .w(px(1.))
                        .h(px(14.))
                        .bg(crate::theme::color(palette.border)),
                )
                .child(
                    div()
                        .id("lightbox-zoom-actual")
                        .px_1p5()
                        .h(px(22.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .text_xs()
                        .when(is_actual, |btn| {
                            btn.cursor_default()
                                .text_color(crate::theme::color(palette.muted))
                                .opacity(0.5)
                        })
                        .when(!is_actual, |btn| {
                            btn.cursor_pointer()
                                .text_color(crate::theme::color(palette.text))
                                .hover(|s| s.bg(crate::theme::color(palette.hover)))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.set_actual_size(None, cx);
                                }))
                        })
                        .tooltip(|window, cx| {
                            Tooltip::new(crate::i18n::media::IMG_TOOLTIP_ACTUAL.get())
                                .build(window, cx)
                        })
                        .child(crate::i18n::media::IMG_ACTUAL_SIZE.get()),
                )
        };

        stage.child(hud).into_any_element()
    }
}
