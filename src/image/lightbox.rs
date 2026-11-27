use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui_kit::{
    Bounds, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollEvent, SharedString, Window,
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
use image::Frame;

const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 16.0;
const ZOOM_STEP: f32 = 1.25;
const WHEEL_ZOOM_STEP: f32 = 1.12;
const FIT_PADDING: f32 = 96.0;
const MAX_SVG_RASTER_DIM: f32 = 4096.0;
const CARD_PADDING: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomMode {
    Fit,
    Custom(f32),
}

pub struct LightboxClose;

pub struct SvgLightbox {
    natural_size: Option<(f32, f32)>,
    svg_tree: Option<Arc<usvg::Tree>>,
    rendered: Option<(u32, Arc<RenderImage>)>,
    pending_bucket: Option<u32>,
    load_error: Option<String>,
    zoom_mode: ZoomMode,
    pan: Point<Pixels>,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag_state: Option<(Point<Pixels>, Point<Pixels>)>,
    focus_handle: FocusHandle,
}

impl EventEmitter<LightboxClose> for SvgLightbox {}

impl SvgLightbox {
    pub fn new(
        svg: SharedString,
        initial_image: Option<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let options = crate::svg::usvg_options();
        let (natural_size, svg_tree, load_error) = match usvg::Tree::from_str(&svg, &options) {
            Ok(tree) => {
                let size = tree.size();
                (
                    Some((size.width().max(1.0), size.height().max(1.0))),
                    Some(Arc::new(tree)),
                    None,
                )
            }
            Err(err) => (
                None,
                None,
                Some(crate::i18n::media::mermaid_svg_parse_failed(&err)),
            ),
        };

        let mut lightbox = Self {
            natural_size,
            svg_tree,
            rendered: initial_image.map(|image| (0, image)),
            pending_bucket: None,
            load_error,
            zoom_mode: ZoomMode::Fit,
            pan: Point::default(),
            viewport_bounds: Rc::new(Cell::new(Bounds::default())),
            drag_state: None,
            focus_handle,
        };
        lightbox.ensure_svg_rendered(window.scale_factor(), cx);
        lightbox
    }

    fn fit_scale(&self) -> f32 {
        let Some((nw, nh)) = self.natural_size else {
            return 1.0;
        };
        let bounds = self.viewport_bounds.get();
        let vw = f32::from(bounds.size.width);
        let vh = f32::from(bounds.size.height);
        if vw <= 1.0 || vh <= 1.0 || nw <= 0.0 || nh <= 0.0 {
            return 1.0;
        }
        let avail_w = (vw - FIT_PADDING - CARD_PADDING * 2.0).max(64.0);
        let avail_h = (vh - FIT_PADDING - CARD_PADDING * 2.0).max(64.0);
        (avail_w / nw).min(avail_h / nh).clamp(MIN_SCALE, 4.0)
    }

    fn effective_scale(&self) -> f32 {
        match self.zoom_mode {
            ZoomMode::Fit => self.fit_scale(),
            ZoomMode::Custom(scale) => scale.clamp(MIN_SCALE, MAX_SCALE),
        }
    }

    fn card_rect(&self, scale: f32) -> Option<Bounds<Pixels>> {
        let (nw, nh) = self.natural_size?;
        let bounds = self.viewport_bounds.get();
        let vw = f32::from(bounds.size.width);
        let vh = f32::from(bounds.size.height);
        if vw <= 1.0 || vh <= 1.0 {
            return None;
        }
        let card_w = nw * scale + CARD_PADDING * 2.0;
        let card_h = nh * scale + CARD_PADDING * 2.0;
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
        let Some((nw, nh)) = self.natural_size else {
            return Point::default();
        };
        let bounds = self.viewport_bounds.get();
        let vw = f32::from(bounds.size.width);
        let vh = f32::from(bounds.size.height);
        if vw <= 1.0 || vh <= 1.0 {
            return pan;
        }
        let rw = nw * scale + CARD_PADDING * 2.0;
        let rh = nh * scale + CARD_PADDING * 2.0;
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

    fn ensure_svg_rendered(&mut self, display_scale: f32, cx: &mut Context<Self>) {
        let Some(tree) = self.svg_tree.clone() else {
            return;
        };
        let Some((nw, nh)) = self.natural_size else {
            return;
        };
        let max_density = (MAX_SVG_RASTER_DIM / nw.max(nh)).max(0.25);
        let target_density = (display_scale.clamp(0.25, max_density.min(8.0)) * 4.0).ceil() / 4.0;
        let bucket = (target_density * 100.0).round() as u32;

        if self.rendered.as_ref().is_some_and(|(b, _)| *b == bucket)
            || self.pending_bucket == Some(bucket)
        {
            return;
        }
        self.pending_bucket = Some(bucket);
        cx.spawn(async move |view, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move { rasterize_svg_tree(&tree, target_density) })
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.pending_bucket == Some(bucket) {
                    view.pending_bucket = None;
                }
                match rendered {
                    Ok(image) => {
                        view.rendered = Some((bucket, image));
                        cx.notify();
                    }
                    Err(err) => {
                        if view.rendered.is_none() {
                            view.load_error = Some(err);
                            cx.notify();
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn handle_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.natural_size.is_none() {
            return;
        }
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
        self.ensure_svg_rendered(scale * window.scale_factor(), cx);

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
                    move |bounds, _, _| {
                        bounds_cell.set(bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        if let Some(error) = &self.load_error {
            stage = stage.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .rounded(px(8.))
                            .bg(crate::theme::color(palette.surface))
                            .border_1()
                            .border_color(crate::theme::color(palette.border))
                            .text_sm()
                            .text_color(crate::theme::color(palette.danger))
                            .child(error.clone()),
                    ),
            );
        } else if let Some((nw, nh)) = self.natural_size {
            let bounds = self.viewport_bounds.get();
            let vw = f32::from(bounds.size.width);
            let vh = f32::from(bounds.size.height);
            let drawn_w = nw * scale;
            let drawn_h = nh * scale;
            let card_w = drawn_w + CARD_PADDING * 2.0;
            let card_h = drawn_h + CARD_PADDING * 2.0;
            let left = (vw - card_w) * 0.5 + f32::from(self.pan.x);
            let top = (vh - card_h) * 0.5 + f32::from(self.pan.y);

            if let Some((_, rendered)) = &self.rendered {
                stage = stage.child(
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
                        ),
                );
            }
        }

        // Top-right close button
        stage = stage.child(
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

        if self.load_error.is_none() {
            let hud = div()
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
                );

            stage = stage.child(hud);
        }

        stage.into_any_element()
    }
}

fn rasterize_svg_tree(tree: &usvg::Tree, density: f32) -> Result<Arc<RenderImage>, String> {
    let size = tree.size();
    let pixel_width = (size.width() * density).ceil().max(1.0) as u32;
    let pixel_height = (size.height() * density).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pixel_width, pixel_height)
        .ok_or_else(|| crate::i18n::media::MERMAID_BITMAP_ALLOC.get().to_owned())?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(density, density),
        &mut pixmap.as_mut(),
    );
    let mut bytes = pixmap.take();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(pixel_width, pixel_height, bytes)
        .ok_or_else(|| crate::i18n::media::MERMAID_BITMAP_INVALID.get().to_owned())?;
    Ok(Arc::new(RenderImage::new([Frame::new(image)])))
}
