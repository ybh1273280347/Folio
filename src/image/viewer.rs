use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::{
    Bounds, Context, CursorStyle, FocusHandle, Focusable, Image, ImageFormat, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollEvent, Window, assets::IconName, canvas, component::tooltip::Tooltip, div, img, point,
    prelude::FluentBuilder, px, *,
};
use image::Frame;

use crate::theme::color as rgb;

const BORDER: u32 = 0xe8e7e3;
const MUTED: u32 = 0x787774;

const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 16.0;
const ZOOM_STEP: f32 = 1.2;
const WHEEL_ZOOM_STEP: f32 = 1.12;
const LINE_SCROLL_PX: f32 = 36.0;
const FIT_PADDING: f32 = 48.0;
const MAX_SVG_RASTER_DIM: f32 = 4096.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomMode {
    Fit,
    Custom(f32),
}

pub struct ImagePane {
    path: PathBuf,
    is_svg: bool,
    natural_size: Option<(f32, f32)>,
    raster_image: Option<Arc<Image>>,
    svg_tree: Option<Arc<usvg::Tree>>,
    svg_rendered: Option<(u32, Arc<RenderImage>)>,
    svg_pending_bucket: Option<u32>,
    load_error: Option<String>,
    zoom_mode: ZoomMode,
    pan: Point<Pixels>,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag_state: Option<(Point<Pixels>, Point<Pixels>)>,
    focus_handle: FocusHandle,
}

impl ImagePane {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            path,
            is_svg: false,
            natural_size: None,
            raster_image: None,
            svg_tree: None,
            svg_rendered: None,
            svg_pending_bucket: None,
            load_error: None,
            zoom_mode: ZoomMode::Fit,
            pan: Point::default(),
            viewport_bounds: Rc::new(Cell::new(Bounds::default())),
            drag_state: None,
            focus_handle: cx.focus_handle(),
        };
        pane.load_from_disk(cx);
        pane
    }

    pub fn file_path(&self) -> &Path {
        &self.path
    }

    pub fn set_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = path;
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load_from_disk(cx);
        cx.notify();
    }

    fn load_from_disk(&mut self, cx: &mut Context<Self>) {
        self.natural_size = None;
        self.raster_image = None;
        self.svg_tree = None;
        self.svg_rendered = None;
        self.svg_pending_bucket = None;
        self.load_error = None;

        let ext = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        self.is_svg = ext == "svg";

        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.load_error = Some(crate::i18n::media::img_load_failed(&err));
                return;
            }
        };

        if self.is_svg {
            let options = crate::mermaid::usvg_options();
            match usvg::Tree::from_data(&bytes, &options) {
                Ok(tree) => {
                    let size = tree.size();
                    self.natural_size = Some((size.width().max(1.0), size.height().max(1.0)));
                    self.svg_tree = Some(Arc::new(tree));
                    self.ensure_svg_rendered(1.0, cx);
                }
                Err(err) => {
                    self.load_error = Some(crate::i18n::media::img_load_failed(&err));
                }
            }
            return;
        }

        let Some(format) = gpui_format_for_ext(&ext) else {
            self.load_error = Some(crate::i18n::media::img_load_failed(&format!(
                "Unsupported format: .{ext}"
            )));
            return;
        };

        match image::load_from_memory(&bytes) {
            Ok(decoded) => {
                self.natural_size = Some((decoded.width().max(1) as f32, decoded.height().max(1) as f32));
                self.raster_image = Some(Arc::new(Image::from_bytes(format, bytes)));
            }
            Err(err) => {
                self.load_error = Some(crate::i18n::media::img_load_failed(&err));
            }
        }
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
        let avail_w = (vw - FIT_PADDING).max(32.0);
        let avail_h = (vh - FIT_PADDING).max(32.0);
        (avail_w / nw).min(avail_h / nh).clamp(MIN_SCALE, 4.0)
    }

    fn effective_scale(&self) -> f32 {
        match self.zoom_mode {
            ZoomMode::Fit => self.fit_scale(),
            ZoomMode::Custom(scale) => scale.clamp(MIN_SCALE, MAX_SCALE),
        }
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
        let rw = nw * scale;
        let rh = nh * scale;
        // Always keep at least 64px (or half the image if smaller) reachable inside the viewport.
        let keep_x = (rw * 0.5).min(64.0);
        let keep_y = (rh * 0.5).min(64.0);
        let limit_x = ((vw + rw) * 0.5 - keep_x).max(0.0);
        let limit_y = ((vh + rh) * 0.5 - keep_y).max(0.0);
        point(
            px(f32::from(pan.x).clamp(-limit_x, limit_x)),
            px(f32::from(pan.y).clamp(-limit_y, limit_y)),
        )
    }

    fn zoom_to_at(&mut self, new_scale: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
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
        // Quantize density to 0.25 steps so smooth wheel zooming stays responsive and cache-friendly.
        let target_density = (display_scale.clamp(0.25, max_density.min(8.0)) * 4.0).ceil() / 4.0;
        let bucket = (target_density * 100.0).round() as u32;

        if self.svg_rendered.as_ref().is_some_and(|(b, _)| *b == bucket)
            || self.svg_pending_bucket == Some(bucket)
        {
            return;
        }
        self.svg_pending_bucket = Some(bucket);
        cx.spawn(async move |pane, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move { rasterize_svg_tree(&tree, target_density) })
                .await;
            let _ = pane.update(cx, |pane, cx| {
                if pane.svg_pending_bucket == Some(bucket) {
                    pane.svg_pending_bucket = None;
                }
                match rendered {
                    Ok(image) => {
                        pane.svg_rendered = Some((bucket, image));
                        cx.notify();
                    }
                    Err(err) => {
                        if pane.svg_rendered.is_none() {
                            pane.load_error = Some(err);
                            cx.notify();
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn handle_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if self.natural_size.is_none() {
            return;
        }
        let (dx, dy) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(lines) => (lines.x * LINE_SCROLL_PX, lines.y * LINE_SCROLL_PX),
        };
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        cx.stop_propagation();
        if event.modifiers.control || event.modifiers.platform {
            let steps = match event.delta {
                ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
                ScrollDelta::Lines(lines) => lines.y,
            };
            if steps != 0.0 {
                let factor = WHEEL_ZOOM_STEP.powf(steps);
                self.zoom_by(factor, Some(event.position), cx);
            }
            return;
        }

        let scale = self.effective_scale();
        let next_pan = point(self.pan.x + px(dx), self.pan.y + px(dy));
        self.pan = self.clamp_pan(next_pan, scale);
        cx.notify();
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        if self.natural_size.is_none() {
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
        if !(event.keystroke.modifiers.control || event.keystroke.modifiers.platform) {
            return;
        }
        match event.keystroke.key.as_str() {
            "=" | "+" => {
                self.zoom_by(ZOOM_STEP, None, cx);
                cx.stop_propagation();
            }
            "-" | "_" => {
                self.zoom_by(1.0 / ZOOM_STEP, None, cx);
                cx.stop_propagation();
            }
            "0" => {
                self.reset_fit(cx);
                cx.stop_propagation();
            }
            "1" => {
                self.set_actual_size(None, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

impl Focusable for ImagePane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImagePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.effective_scale();
        let clamped_pan = self.clamp_pan(self.pan, scale);
        if clamped_pan != self.pan {
            self.pan = clamped_pan;
        }
        if self.is_svg {
            self.ensure_svg_rendered(scale * window.scale_factor(), cx);
        }

        let bounds_cell = self.viewport_bounds.clone();
        let dragging = self.drag_state.is_some();
        let percent = (scale * 100.0).round() as u32;
        let is_fit = matches!(self.zoom_mode, ZoomMode::Fit);
        let is_actual = !is_fit && percent == 100;
        let zoom_label = if is_fit {
            format!("{} ({percent}%)", crate::i18n::media::IMG_FIT.get())
        } else {
            format!("{percent}%")
        };

        let mut stage = div()
            .id("image-viewer-stage")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0xf5f5f4))
            .key_context("FolioImageViewer")
            .track_focus(&self.focus_handle)
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                pane.handle_scroll(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, event: &MouseDownEvent, window, cx| {
                    pane.handle_mouse_down(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|pane, event: &MouseMoveEvent, _, cx| {
                pane.handle_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|pane, _, _, cx| {
                    pane.handle_mouse_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|pane, _, _, cx| {
                    pane.handle_mouse_up(cx);
                }),
            )
            .on_key_down(cx.listener(|pane, event: &KeyDownEvent, _, cx| {
                pane.handle_key_down(event, cx);
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
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(error.clone()),
            );
        } else if let Some((nw, nh)) = self.natural_size {
            let bounds = self.viewport_bounds.get();
            let vw = f32::from(bounds.size.width);
            let vh = f32::from(bounds.size.height);
            let drawn_w = nw * scale;
            let drawn_h = nh * scale;
            let left = (vw - drawn_w) * 0.5 + f32::from(self.pan.x);
            let top = (vh - drawn_h) * 0.5 + f32::from(self.pan.y);

            let image_el = if self.is_svg {
                self.svg_rendered.as_ref().map(|(_, rendered)| {
                    img(rendered.clone())
                        .w(px(drawn_w))
                        .h(px(drawn_h))
                        .object_fit(ObjectFit::Fill)
                        .into_any_element()
                })
            } else {
                self.raster_image.as_ref().map(|image| {
                    img(image.clone())
                        .w(px(drawn_w))
                        .h(px(drawn_h))
                        .object_fit(ObjectFit::Fill)
                        .into_any_element()
                })
            };

            if let Some(image_el) = image_el {
                stage = stage.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(drawn_w))
                        .h(px(drawn_h))
                        .when(self.is_svg, |box_div| {
                            // Draw a crisp paper surface behind transparent SVGs so dark strokes stay legible.
                            box_div.bg(rgb(0xffffff)).shadow_sm()
                        })
                        .child(image_el),
                );
            }
        }

        if self.load_error.is_none() {
            let hud = div()
                .id("image-viewer-hud")
                .absolute()
                .bottom_4()
                .right_4()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(rgb(0xffffff).opacity(0.94))
                .border_1()
                .border_color(rgb(BORDER))
                .shadow_sm()
                .cursor_default()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    crate::ui::control("image-zoom-out", IconName::Minus)
                        .tooltip(|window, cx| {
                            Tooltip::new(crate::i18n::media::IMG_TOOLTIP_ZOOM_OUT.get())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.zoom_by(1.0 / ZOOM_STEP, None, cx);
                        })),
                )
                .child(
                    div()
                        .px_1p5()
                        .text_xs()
                        .text_color(rgb(0x37352f))
                        .child(zoom_label),
                )
                .child(
                    crate::ui::control("image-zoom-in", IconName::Plus)
                        .tooltip(|window, cx| {
                            Tooltip::new(crate::i18n::media::IMG_TOOLTIP_ZOOM_IN.get())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.zoom_by(ZOOM_STEP, None, cx);
                        })),
                )
                .child(div().w(px(1.)).h(px(14.)).mx_0p5().bg(rgb(BORDER)))
                .child(
                    div()
                        .id("image-zoom-actual")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_xs()
                        .cursor_pointer()
                        .when(is_actual, |btn| btn.bg(rgb(0xedf2fa)).text_color(rgb(0x165dff)))
                        .when(!is_actual, |btn| {
                            btn.text_color(rgb(MUTED))
                                .hover(|s| s.bg(rgb(0xf4f4f2)).text_color(rgb(0x37352f)))
                        })
                        .tooltip(|window, cx| {
                            Tooltip::new(crate::i18n::media::IMG_TOOLTIP_ACTUAL.get())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.set_actual_size(None, cx);
                        }))
                        .child(crate::i18n::media::IMG_ACTUAL_SIZE.get()),
                )
                .child(
                    div()
                        .id("image-zoom-fit")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_xs()
                        .cursor_pointer()
                        .when(is_fit, |btn| btn.bg(rgb(0xedf2fa)).text_color(rgb(0x165dff)))
                        .when(!is_fit, |btn| {
                            btn.text_color(rgb(MUTED))
                                .hover(|s| s.bg(rgb(0xf4f4f2)).text_color(rgb(0x37352f)))
                        })
                        .tooltip(|window, cx| {
                            Tooltip::new(crate::i18n::media::IMG_TOOLTIP_FIT.get())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.reset_fit(cx);
                        }))
                        .child(crate::i18n::media::IMG_FIT.get()),
                );

            let hint = div()
                .absolute()
                .bottom_4()
                .left_4()
                .px_2p5()
                .py_1()
                .rounded(px(6.))
                .bg(rgb(0xffffff).opacity(0.86))
                .border_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .text_color(rgb(MUTED))
                .child(crate::i18n::media::IMG_HINT_INTERACTION.get());

            stage = stage.child(hint).child(hud);
        }

        stage.into_any_element()
    }
}

fn gpui_format_for_ext(ext: &str) -> Option<ImageFormat> {
    match ext {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        "gif" => Some(ImageFormat::Gif),
        _ => None,
    }
}

fn rasterize_svg_tree(tree: &usvg::Tree, density: f32) -> Result<Arc<RenderImage>, String> {
    let size = tree.size();
    let pixel_width = (size.width() * density).ceil().max(1.0) as u32;
    let pixel_height = (size.height() * density).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pixel_width, pixel_height)
        .ok_or_else(|| "Failed to allocate SVG bitmap".to_owned())?;
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
        .ok_or_else(|| "Invalid SVG bitmap dimensions".to_owned())?;
    Ok(Arc::new(RenderImage::new([Frame::new(image)])))
}
