use std::rc::Rc;

use gpui::{
    App, BorderStyle, Bounds, Edges, FontStyle, FontWeight as GpuiFontWeight, Hsla,
    Path, PathBuilder, Pixels, Point as GpuiPoint, SharedString, ShapedLine, TextAlign,
    TextRun, Window, font, point, px, quad,
};
use rusty_mermaid_core::{
    Color, FontWeight, MarkerType, PathSegment, Point, Primitive, Style, TextAnchor, TextStyle,
    Theme, Transform, marker_path, parse_inline_markdown, text_baseline_y_offset,
};

use crate::PreparedScene;

#[derive(Clone)]
pub(crate) struct PreparedCache {
    pub(crate) nodes: Vec<PreparedNode>,
}

impl PreparedCache {
    pub(crate) fn from_scene(scene: &rusty_mermaid_core::Scene, theme: &Theme) -> Self {
        Self {
            nodes: scene
                .elements()
                .iter()
                .map(|element| template_node(&element.primitive, theme))
                .collect(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct PreparedError {
    pub(crate) line: ShapedLine,
}

#[derive(Clone)]
pub(crate) struct PreparedFrame {
    pub(crate) scale: f32,
    pub(crate) cache: Option<Rc<PreparedCache>>,
    pub(crate) error: Option<PreparedError>,
}

pub(crate) struct PreparedScale {
    pub(crate) scale: f32,
    cache: Option<Rc<PreparedCache>>,
    error: Option<PreparedError>,
}

impl PreparedScale {
    pub(crate) fn success(scale: f32, cache: Rc<PreparedCache>) -> Self {
        Self {
            scale,
            cache: Some(cache),
            error: None,
        }
    }

    pub(crate) fn failure(scale: f32, error: PreparedError) -> Self {
        Self {
            scale,
            cache: None,
            error: Some(error),
        }
    }

    pub(crate) fn frame(&self) -> PreparedFrame {
        PreparedFrame {
            scale: self.scale,
            cache: self.cache.clone(),
            error: self.error.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) enum PreparedNode {
    Empty,
    Circle {
        fill: Option<Path<Pixels>>,
        stroke: Option<Path<Pixels>>,
    },
    Ellipse {
        fill: Option<Path<Pixels>>,
        stroke: Option<Path<Pixels>>,
    },
    Path {
        fill: Option<Path<Pixels>>,
        stroke: Option<Path<Pixels>>,
        marker_start: Option<PreparedMarker>,
        marker_end: Option<PreparedMarker>,
    },
    Text(PreparedText),
    Polygon {
        fill: Option<Path<Pixels>>,
        stroke: Option<Path<Pixels>>,
    },
    Group {
        children: Vec<PreparedNode>,
    },
    Arc {
        fill: Option<Path<Pixels>>,
        stroke: Option<Path<Pixels>>,
    },
}

#[derive(Clone)]
pub(crate) struct PreparedText {
    pub(crate) lines: Vec<PreparedTextLine>,
}

#[derive(Clone)]
pub(crate) struct PreparedTextLine {
    pub(crate) text: SharedString,
    pub(crate) runs: Rc<Vec<TextRun>>,
    pub(crate) shaped: Option<ShapedLine>,
}
#[derive(Clone)]

pub(crate) enum PreparedMarker {
    Fill {
        path: Path<Pixels>,
        color: Hsla,
    },
    Stroke {
        path: Path<Pixels>,
        color: Hsla,
    },
    FillStroke {
        fill: Path<Pixels>,
        stroke: Path<Pixels>,
        fill_color: Hsla,
        stroke_color: Hsla,
    },
    Circle {
        path: Path<Pixels>,
        color: Hsla,
    },
    Curves {
        paths: Vec<Path<Pixels>>,
        color: Hsla,
    },
}

#[derive(Clone, Copy)]
struct PaintCtx<'a> {
    theme: &'a Theme,
    zoom: f32,
    ox: f32,
    oy: f32,
    root_dx: f32,
    root_dy: f32,
}

pub(crate) fn prepare_cache(
    cache: &mut PreparedCache,
    scene: &rusty_mermaid_core::Scene,
    theme: &Theme,
    scale: f32,
    ox: f32,
    oy: f32,
    window: &mut Window,
) -> Result<(), String> {
    let ctx = PaintCtx {
        theme,
        zoom: scale,
        ox,
        oy,
        root_dx: 0.0,
        root_dy: 0.0,
    };
    for (element, node) in scene.elements().iter().zip(&mut cache.nodes) {
        prepare_node(node, &element.primitive, ctx, window)?;
    }
    Ok(())
}

fn template_node(primitive: &Primitive, theme: &Theme) -> PreparedNode {
    match primitive {
        Primitive::Circle { .. } => PreparedNode::Circle {
            fill: None,
            stroke: None,
        },
        Primitive::Ellipse { .. } => PreparedNode::Ellipse {
            fill: None,
            stroke: None,
        },
        Primitive::Path { .. } => PreparedNode::Path {
            fill: None,
            stroke: None,
            marker_start: None,
            marker_end: None,
        },
        Primitive::Text {
            content, style, ..
        } => PreparedNode::Text(prepare_text(content, style, theme)),
        Primitive::Polygon { .. } => PreparedNode::Polygon {
            fill: None,
            stroke: None,
        },
        Primitive::Group { children, .. } => PreparedNode::Group {
            children: children
                .iter()
                .map(|child| template_node(child, theme))
                .collect(),
        },
        Primitive::Arc { .. } => PreparedNode::Arc {
            fill: None,
            stroke: None,
        },
        Primitive::Rect { .. } => PreparedNode::Empty,
    }
}

fn prepare_node(
    node: &mut PreparedNode,
    primitive: &Primitive,
    ctx: PaintCtx<'_>,
    window: &mut Window,
) -> Result<(), String> {
    match (node, primitive) {
        (
            PreparedNode::Circle { fill, stroke },
            Primitive::Circle {
                center,
                radius,
                style,
            },
        ) => {
            let cx = center.x as f32 * ctx.zoom + ctx.ox;
            let cy = center.y as f32 * ctx.zoom + ctx.oy;
            let r = *radius as f32 * ctx.zoom;
            *fill = style
                .fill
                .map(|color| ellipse_fill_path(cx, cy, r, r))
                .transpose()?;
            *stroke = resolve_stroke_opt(style, ctx.theme)
                .map(|(color, width)| ellipse_stroke_path(cx, cy, r, r, width * ctx.zoom))
                .transpose()?;
        }
        (
            PreparedNode::Ellipse { fill, stroke },
            Primitive::Ellipse {
                center,
                rx,
                ry,
                style,
            },
        ) => {
            let cx = center.x as f32 * ctx.zoom + ctx.ox;
            let cy = center.y as f32 * ctx.zoom + ctx.oy;
            let rx = *rx as f32 * ctx.zoom;
            let ry = *ry as f32 * ctx.zoom;
            *fill = style
                .fill
                .map(|_| ellipse_fill_path(cx, cy, rx, ry))
                .transpose()?;
            *stroke = resolve_stroke_opt(style, ctx.theme)
                .map(|(_, width)| ellipse_stroke_path(cx, cy, rx, ry, width * ctx.zoom))
                .transpose()?;
        }
        (
            PreparedNode::Path {
                fill,
                stroke,
                marker_start,
                marker_end,
            },
            Primitive::Path {
                segments,
                style,
                marker_start: start,
                marker_end: end,
            },
        ) => {
            let stroke_info = resolve_stroke_gpui(style, ctx.theme);
            *stroke = Some(build_stroke_path(
                segments,
                ctx.zoom,
                ctx.ox,
                ctx.oy,
                stroke_info.1 * ctx.zoom,
                style,
            )?);
            *fill = style
                .fill
                .map(|_| build_fill_path(segments, ctx.zoom, ctx.ox, ctx.oy))
                .transpose()?;
            *marker_start = start
                .map(|marker| build_marker(marker, segments, true, stroke_info, ctx))
                .transpose()?;
            *marker_end = end
                .map(|marker| build_marker(marker, segments, false, stroke_info, ctx))
                .transpose()?;
        }
        (
            PreparedNode::Text(prepared),
            Primitive::Text { style, .. },
        ) => {
            prepare_text_lines(prepared, style, ctx.zoom, window);
        }
        (
            PreparedNode::Polygon { fill, stroke },
            Primitive::Polygon { points, style },
        ) => {
            if points.len() < 3 {
                *fill = None;
                *stroke = None;
            } else {
                *fill = style
                    .fill
                    .map(|_| build_polygon_path(points, ctx, style, false))
                    .transpose()?;
                *stroke = resolve_stroke_opt(style, ctx.theme)
                    .map(|_| build_polygon_path(points, ctx, style, true))
                    .transpose()?;
            }
        }
        (
            PreparedNode::Group { children },
            Primitive::Group {
                transform,
                children: primitives,
            },
        ) => {
            let (zoom, ox, oy) = apply_transform(transform, ctx.zoom, ctx.ox, ctx.oy);
            let child_ctx = PaintCtx {
                theme: ctx.theme,
                zoom,
                ox,
                oy,
                root_dx: 0.0,
                root_dy: 0.0,
            };
            for (child, primitive) in children.iter_mut().zip(primitives) {
                prepare_node(child, primitive, child_ctx, window)?;
            }
        }
        (
            PreparedNode::Arc { fill, stroke },
            Primitive::Arc {
                center,
                inner_r,
                outer_r,
                start_angle,
                end_angle,
                style,
            },
        ) => {
            let segments = rusty_mermaid_core::arc_sector_segments(
                *center,
                *inner_r,
                *outer_r,
                *start_angle,
                *end_angle,
            );
            *fill = style
                .fill
                .map(|_| build_fill_path(&segments, ctx.zoom, ctx.ox, ctx.oy))
                .transpose()?;
            *stroke = resolve_stroke_opt(style, ctx.theme)
                .map(|(_, width)| {
                    build_stroke_path(
                        &segments,
                        ctx.zoom,
                        ctx.ox,
                        ctx.oy,
                        width * ctx.zoom,
                        style,
                    )
                })
                .transpose()?;
        }
        (PreparedNode::Empty, Primitive::Rect { .. }) => {}
        _ => return Err("Mermaid preparation cache mismatch".to_owned()),
    }
    Ok(())
}

fn prepare_text(content: &str, style: &TextStyle, theme: &Theme) -> PreparedText {
    let color = to_gpui_color(style.fill.unwrap_or(Color::rgb(51, 51, 51)));
    let lines = content
        .split('\n')
        .map(|line| prepare_text_line(line, style, color, theme))
        .collect();
    PreparedText { lines }
}

fn prepare_text_line(
    line: &str,
    style: &TextStyle,
    color: Hsla,
    _theme: &Theme,
) -> PreparedTextLine {
    let (display, runs) = if let Some(spans) = parse_inline_markdown(line) {
        let mut display = String::new();
        let mut runs = Vec::with_capacity(spans.len());
        for span in spans {
            let mut text_font = font("Consolas");
            text_font.style = if span.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            };
            text_font.weight = if span.bold || style.font_weight == FontWeight::Bold {
                GpuiFontWeight::BOLD
            } else {
                GpuiFontWeight::NORMAL
            };
            runs.push(TextRun {
                len: span.text.len(),
                font: text_font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            display.push_str(&span.text);
        }
        (display, runs)
    } else {
        let mut text_font = font("Consolas");
        text_font.weight = if style.font_weight == FontWeight::Bold {
            GpuiFontWeight::BOLD
        } else {
            GpuiFontWeight::NORMAL
        };
        (
            line.to_owned(),
            vec![TextRun {
                len: line.len(),
                font: text_font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
        )
    };
    PreparedTextLine {
        text: display.into(),
        runs: Rc::new(runs),
        shaped: None,
    }
}

fn prepare_text_lines(
    prepared: &mut PreparedText,
    style: &TextStyle,
    zoom: f32,
    window: &mut Window,
) {
    for line in &mut prepared.lines {
        line.shaped = Some(window.text_system().shape_line(
            line.text.clone(),
            px(style.font_size as f32 * zoom),
            line.runs.as_ref(),
            None,
        ));
    }
}

pub(crate) fn prepare_error(window: &mut Window, message: String) -> PreparedError {
    let text: SharedString = format!("Mermaid 渲染失败：{message}").into();
    let run = TextRun {
        len: text.len(),
        font: font("Consolas"),
        color: Hsla {
            h: 0.0,
            s: 0.65,
            l: 0.45,
            a: 1.0,
        },
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, px(13.0), &[run], None);
    PreparedError { line }
}

pub(crate) fn paint_scene(
    prepared: &PreparedScene,
    bounds: Bounds<Pixels>,
    frame: &PreparedFrame,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(error) = &frame.error {
        let width: f32 = error.line.width().into();
        let x: f32 = bounds.origin.x.into();
        let y: f32 = bounds.origin.y.into();
        if let Err(error) = error.line.paint(
            point(px(x + 8.0), px(y + 8.0)),
            px(18.0),
            TextAlign::Left,
            Some(px(width)),
            window,
            cx,
        ) {
            prepared.record_paint_error(error.to_string(), window, cx);
        }
        return;
    }

    let cache = frame
        .cache
        .as_ref()
        .expect("successful Mermaid frame has no cache");
    let scale = frame.scale;
    let padding = prepared.theme.padding as f32 * scale;
    let bx: f32 = bounds.origin.x.into();
    let by: f32 = bounds.origin.y.into();
    let ctx = PaintCtx {
        theme: &prepared.theme,
        zoom: scale,
        ox: bx + padding,
        oy: by + padding,
        root_dx: bx,
        root_dy: by,
    };
    for (element, node) in prepared.scene.elements().iter().zip(&cache.nodes) {
        if let Err(error) = paint_node(&element.primitive, node, ctx, window, cx) {
            prepared.record_paint_error(error, window, cx);
            return;
        }
    }
}

/// `Window::paint_path` consumes its path, so cached paths are cloned once per
/// submission and translated from canvas-local coordinates to the current
/// canvas origin here.
fn paint_cached_path(
    window: &mut Window,
    path: &Path<Pixels>,
    color: Hsla,
    dx: f32,
    dy: f32,
) {
    let mut path = path.clone();
    path.bounds.origin.x = path.bounds.origin.x + px(dx);
    path.bounds.origin.y = path.bounds.origin.y + px(dy);
    path.content_mask.bounds.origin.x = path.content_mask.bounds.origin.x + px(dx);
    path.content_mask.bounds.origin.y = path.content_mask.bounds.origin.y + px(dy);
    for vertex in &mut path.vertices {
        vertex.xy_position.x = vertex.xy_position.x + px(dx);
        vertex.xy_position.y = vertex.xy_position.y + px(dy);
        vertex.content_mask.bounds.origin.x =
            vertex.content_mask.bounds.origin.x + px(dx);
        vertex.content_mask.bounds.origin.y =
            vertex.content_mask.bounds.origin.y + px(dy);
    }
    window.paint_path(path, color);
}

fn paint_node(
    primitive: &Primitive,
    node: &PreparedNode,
    ctx: PaintCtx<'_>,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    match (primitive, node) {
        (
            Primitive::Rect {
                bbox, rx, style, ..
            },
            PreparedNode::Empty,
        ) => paint_rect(window, bbox, *rx, style, ctx),
        (
            Primitive::Circle { style, .. },
            PreparedNode::Circle { fill, stroke },
        ) => paint_ellipse_paths(window, style, ctx, fill, stroke),
        (
            Primitive::Ellipse { style, .. },
            PreparedNode::Ellipse { fill, stroke },
        ) => paint_ellipse_paths(window, style, ctx, fill, stroke),
        (
            Primitive::Path { style, .. },
            PreparedNode::Path {
                fill,
                stroke,
                marker_start: prepared_start,
                marker_end: prepared_end,
            },
        ) => {
            if let Some(path) = stroke {
                paint_cached_path(
                    window,
                    path,
                    resolve_stroke_gpui(style, ctx.theme).0,
                    ctx.root_dx,
                    ctx.root_dy,
                );
            }
            if let Some(path) = fill {
                if let Some(color) = style.fill {
                    paint_cached_path(
                        window,
                        path,
                        styled_color(color, style.opacity),
                        ctx.root_dx,
                        ctx.root_dy,
                    );
                }
            }
            paint_prepared_marker(prepared_start.as_ref(), window, ctx.root_dx, ctx.root_dy);
            paint_prepared_marker(prepared_end.as_ref(), window, ctx.root_dx, ctx.root_dy);
        }
        (
            Primitive::Text {
                position,
                anchor,
                style,
                ..
            },
            PreparedNode::Text(prepared_text),
        ) => paint_text(window, cx, position, *anchor, style, ctx, prepared_text)?,
        (
            Primitive::Polygon { style, .. },
            PreparedNode::Polygon { fill, stroke },
        ) => {
            if let Some(path) = fill {
                if let Some(color) = style.fill {
                    paint_cached_path(
                        window,
                        path,
                        styled_color(color, style.opacity),
                        ctx.root_dx,
                        ctx.root_dy,
                    );
                }
            }
            if let Some(path) = stroke {
                paint_cached_path(
                    window,
                    path,
                    resolve_stroke_gpui(style, ctx.theme).0,
                    ctx.root_dx,
                    ctx.root_dy,
                );
            }
        }
        (
            Primitive::Group {
                transform,
                children,
            },
            PreparedNode::Group {
                children: prepared_children,
            },
        ) => {
            let (zoom, ox, oy) = apply_transform(transform, ctx.zoom, ctx.ox, ctx.oy);
            let child_ctx = PaintCtx {
                theme: ctx.theme,
                zoom,
                ox,
                oy,
                root_dx: ctx.root_dx,
                root_dy: ctx.root_dy,
            };
            for (primitive, node) in children.iter().zip(prepared_children) {
                paint_node(primitive, node, child_ctx, window, cx)?;
            }
        }
        (
            Primitive::Arc { style, .. },
            PreparedNode::Arc { fill, stroke },
        ) => {
            if let Some(path) = fill {
                if let Some(color) = style.fill {
                    paint_cached_path(
                        window,
                        path,
                        styled_color(color, style.opacity),
                        ctx.root_dx,
                        ctx.root_dy,
                    );
                }
            }
            if let Some(path) = stroke {
                paint_cached_path(
                    window,
                    path,
                    resolve_stroke_gpui(style, ctx.theme).0,
                    ctx.root_dx,
                    ctx.root_dy,
                );
            }
        }
        _ => unreachable!("prepared Mermaid primitive cache mismatch"),
    }
    Ok(())
}

fn paint_rect(
    window: &mut Window,
    bbox: &rusty_mermaid_core::BBox,
    rx: f64,
    style: &Style,
    ctx: PaintCtx<'_>,
) {
    let left = (bbox.x - bbox.width / 2.0) as f32 * ctx.zoom + ctx.ox;
    let top = (bbox.y - bbox.height / 2.0) as f32 * ctx.zoom + ctx.oy;
    let w = bbox.width as f32 * ctx.zoom;
    let h = bbox.height as f32 * ctx.zoom;
    let r = rx as f32 * ctx.zoom;
    let rect_bounds = Bounds {
        origin: point(px(left), px(top)),
        size: gpui::size(px(w), px(h)),
    };
    let bg = style
        .fill
        .map(|color| styled_color(color, style.opacity))
        .unwrap_or(gpui::transparent_black());
    let (border_color, border_width) = resolve_stroke_gpui(style, ctx.theme);
    window.paint_quad(quad(
        rect_bounds,
        px(r),
        bg,
        Edges::all(px(border_width * ctx.zoom)),
        border_color,
        BorderStyle::Solid,
    ));
}

fn paint_ellipse_paths(
    window: &mut Window,
    style: &Style,
    ctx: PaintCtx<'_>,
    fill: &Option<Path<Pixels>>,
    stroke: &Option<Path<Pixels>>,
) {
    if let Some(path) = fill {
        if let Some(color) = style.fill {
            paint_cached_path(
                window,
                path,
                styled_color(color, style.opacity),
                ctx.root_dx,
                ctx.root_dy,
            );
        }
    }
    if let Some(path) = stroke {
        paint_cached_path(
            window,
            path,
            resolve_stroke_gpui(style, ctx.theme).0,
            ctx.root_dx,
            ctx.root_dy,
        );
    }
}

fn paint_text(
    window: &mut Window,
    cx: &mut App,
    position: &Point,
    anchor: TextAnchor,
    style: &TextStyle,
    ctx: PaintCtx<'_>,
    prepared: &PreparedText,
) -> Result<(), String> {
    let x = position.x as f32 * ctx.zoom + ctx.ox;
    let y = position.y as f32 * ctx.zoom + ctx.oy;
    let font_size = style.font_size as f32 * ctx.zoom;
    let line_height = font_size * rusty_mermaid_core::constants::LINE_HEIGHT_MULTIPLIER_F32;
    let baseline_offset = text_baseline_y_offset(style.font_size, prepared.lines.len()) as f32 * ctx.zoom;

    for (line_idx, line) in prepared.lines.iter().enumerate() {
        if line.text.is_empty() {
            continue;
        }
        let shaped = line.shaped.as_ref().expect("prepared Mermaid text has no shape");
        let text_w: f32 = shaped.width().into();
        let text_x = match anchor {
            TextAnchor::Start => x,
            TextAnchor::Middle => x - text_w / 2.0,
            TextAnchor::End => x - text_w,
        };
        let text_y = y + baseline_offset - font_size + line_idx as f32 * line_height;
        shaped.paint(
            point(px(text_x), px(text_y)),
            px(line_height),
            TextAlign::Left,
            None,
            window,
            cx,
        ).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn to_gpui_color(c: Color) -> Hsla {
    let r = c.r as f32 / 255.0;
    let g = c.g as f32 / 255.0;
    let b = c.b as f32 / 255.0;
    let a = c.a as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return Hsla { h: 0.0, s: 0.0, l, a };
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < 1e-6 {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() < 1e-6 {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    Hsla { h, s, l, a }
}

fn styled_color(color: Color, opacity: Option<f64>) -> Hsla {
    let mut color = to_gpui_color(color);
    color.a *= opacity.unwrap_or(1.0) as f32;
    color
}

fn resolve_stroke_gpui(style: &Style, theme: &Theme) -> (Hsla, f32) {
    (
        styled_color(style.resolved_stroke(theme), style.opacity),
        style.resolved_stroke_width(theme) as f32,
    )
}

fn resolve_stroke_opt(style: &Style, theme: &Theme) -> Option<(Hsla, f32)> {
    style
        .resolve_stroke_opt(theme)
        .map(|(color, width)| (styled_color(color, style.opacity), width as f32))
}

fn transform_pt(p: &Point, zoom: f32, ox: f32, oy: f32) -> GpuiPoint<Pixels> {
    point(px(p.x as f32 * zoom + ox), px(p.y as f32 * zoom + oy))
}

fn apply_transform(t: &Transform, zoom: f32, ox: f32, oy: f32) -> (f32, f32, f32) {
    match t {
        Transform::Identity => (zoom, ox, oy),
        Transform::Translate(dx, dy) => (zoom, ox + *dx as f32 * zoom, oy + *dy as f32 * zoom),
        Transform::Scale(sx, _) => (*sx as f32 * zoom, ox, oy),
        Transform::Rotate { .. } => (zoom, ox, oy),
    }
}

fn build_stroke_path(
    segments: &[PathSegment],
    zoom: f32,
    ox: f32,
    oy: f32,
    width: f32,
    style: &Style,
) -> Result<Path<Pixels>, String> {
    let mut pb = PathBuilder::stroke(px(width));
    if let Some(dashes) = &style.stroke_dasharray {
        let dash_px: Vec<Pixels> = dashes.iter().map(|dash| px(*dash as f32 * zoom)).collect();
        pb = pb.dash_array(&dash_px);
    }
    build_path_segments(&mut pb, segments, zoom, ox, oy);
    pb.build().map_err(|error| error.to_string())
}

fn build_fill_path(
    segments: &[PathSegment],
    zoom: f32,
    ox: f32,
    oy: f32,
) -> Result<Path<Pixels>, String> {
    let mut pb = PathBuilder::fill();
    build_path_segments(&mut pb, segments, zoom, ox, oy);
    pb.build().map_err(|error| error.to_string())
}

fn build_path_segments(
    pb: &mut PathBuilder,
    segments: &[PathSegment],
    zoom: f32,
    ox: f32,
    oy: f32,
) {
    for segment in segments {
        match segment {
            PathSegment::MoveTo(p) => pb.move_to(transform_pt(p, zoom, ox, oy)),
            PathSegment::LineTo(p) => pb.line_to(transform_pt(p, zoom, ox, oy)),
            PathSegment::CubicTo { cp1, cp2, to } => pb.cubic_bezier_to(
                transform_pt(to, zoom, ox, oy),
                transform_pt(cp1, zoom, ox, oy),
                transform_pt(cp2, zoom, ox, oy),
            ),
            PathSegment::QuadTo { cp, to } => {
                pb.curve_to(transform_pt(to, zoom, ox, oy), transform_pt(cp, zoom, ox, oy))
            }
            PathSegment::ArcTo {
                rx,
                ry,
                rotation,
                large_arc,
                sweep,
                to,
            } => pb.arc_to(
                point(px(*rx as f32 * zoom), px(*ry as f32 * zoom)),
                px(*rotation as f32),
                *large_arc,
                *sweep,
                transform_pt(to, zoom, ox, oy),
            ),
            PathSegment::Close => pb.close(),
        }
    }
}

fn build_polygon_path(
    points: &[Point],
    ctx: PaintCtx<'_>,
    style: &Style,
    stroke: bool,
) -> Result<Path<Pixels>, String> {
    let mut pb = if stroke {
        let width = style.resolved_stroke_width(ctx.theme) as f32 * ctx.zoom;
        PathBuilder::stroke(px(width))
    } else {
        PathBuilder::fill()
    };
    pb.move_to(transform_pt(&points[0], ctx.zoom, ctx.ox, ctx.oy));
    for point in &points[1..] {
        pb.line_to(transform_pt(point, ctx.zoom, ctx.ox, ctx.oy));
    }
    pb.close();
    pb.build().map_err(|error| error.to_string())
}

fn ellipse_fill_path(
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
) -> Result<Path<Pixels>, String> {
    let mut pb = PathBuilder::fill();
    let radii = point(px(rx), px(ry));
    pb.move_to(point(px(cx + rx), px(cy)));
    pb.arc_to(radii, px(0.0), false, true, point(px(cx - rx), px(cy)));
    pb.arc_to(radii, px(0.0), false, true, point(px(cx + rx), px(cy)));
    pb.close();
    pb.build().map_err(|error| error.to_string())
}

fn ellipse_stroke_path(
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    width: f32,
) -> Result<Path<Pixels>, String> {
    let mut pb = PathBuilder::stroke(px(width));
    let radii = point(px(rx), px(ry));
    pb.move_to(point(px(cx + rx), px(cy)));
    pb.arc_to(radii, px(0.0), false, true, point(px(cx - rx), px(cy)));
    pb.arc_to(radii, px(0.0), false, true, point(px(cx + rx), px(cy)));
    pb.close();
    pb.build().map_err(|error| error.to_string())
}

fn build_marker(
    marker: MarkerType,
    segments: &[PathSegment],
    start: bool,
    (color, width): (Hsla, f32),
    ctx: PaintCtx<'_>,
) -> Result<PreparedMarker, String> {
    let tangent = if start {
        rusty_mermaid_core::path_start_tangent(segments)
    } else {
        rusty_mermaid_core::path_end_tangent(segments)
    };
    let Some((tip, angle)) = tangent else {
        return Err("Mermaid path marker has no tangent".to_owned());
    };
    let sw = width as f64;
    match marker_path(marker, tip, angle, sw) {
        rusty_mermaid_core::MarkerPath::FillPolygon { points } => {
            let mut pb = PathBuilder::fill();
            pb.move_to(transform_pt(&points[0], ctx.zoom, ctx.ox, ctx.oy));
            for point in &points[1..] {
                pb.line_to(transform_pt(point, ctx.zoom, ctx.ox, ctx.oy));
            }
            pb.close();
            Ok(PreparedMarker::Fill {
                path: pb.build().map_err(|error| error.to_string())?,
                color,
            })
        }
        rusty_mermaid_core::MarkerPath::StrokePolyline {
            points,
            width,
            closed,
        } => {
            let mut pb = PathBuilder::stroke(px(width as f32 * ctx.zoom));
            pb.move_to(transform_pt(&points[0], ctx.zoom, ctx.ox, ctx.oy));
            for point in &points[1..] {
                pb.line_to(transform_pt(point, ctx.zoom, ctx.ox, ctx.oy));
            }
            if closed {
                pb.close();
            }
            Ok(PreparedMarker::Stroke {
                path: pb.build().map_err(|error| error.to_string())?,
                color,
            })
        }
        rusty_mermaid_core::MarkerPath::FillAndStrokePolygon {
            points,
            stroke_width,
            fill_is_marker_color,
        } => {
            let mut fill_pb = PathBuilder::fill();
            fill_pb.move_to(transform_pt(&points[0], ctx.zoom, ctx.ox, ctx.oy));
            for point in &points[1..] {
                fill_pb.line_to(transform_pt(point, ctx.zoom, ctx.ox, ctx.oy));
            }
            fill_pb.close();
            let mut stroke_pb = PathBuilder::stroke(px(stroke_width as f32 * ctx.zoom));
            stroke_pb.move_to(transform_pt(&points[0], ctx.zoom, ctx.ox, ctx.oy));
            for point in &points[1..] {
                stroke_pb.line_to(transform_pt(point, ctx.zoom, ctx.ox, ctx.oy));
            }
            stroke_pb.close();
            Ok(PreparedMarker::FillStroke {
                fill: fill_pb.build().map_err(|error| error.to_string())?,
                stroke: stroke_pb.build().map_err(|error| error.to_string())?,
                fill_color: if fill_is_marker_color {
                    color
                } else {
                    Hsla {
                        h: 0.0,
                        s: 0.0,
                        l: 1.0,
                        a: color.a,
                    }
                },
                stroke_color: color,
            })
        }
        rusty_mermaid_core::MarkerPath::FillCircle { center, radius } => Ok(
            PreparedMarker::Circle {
                path: ellipse_fill_path(
                    center.x as f32 * ctx.zoom + ctx.ox,
                    center.y as f32 * ctx.zoom + ctx.oy,
                    radius as f32 * ctx.zoom,
                    radius as f32 * ctx.zoom,
                )?,
                color,
            },
        ),
        rusty_mermaid_core::MarkerPath::StrokeCurves { curves, width } => {
            let mut paths = Vec::with_capacity(curves.len());
            for [start, control, end] in curves {
                let mut pb = PathBuilder::stroke(px(width as f32 * ctx.zoom));
                pb.move_to(transform_pt(&start, ctx.zoom, ctx.ox, ctx.oy));
                pb.curve_to(
                    transform_pt(&end, ctx.zoom, ctx.ox, ctx.oy),
                    transform_pt(&control, ctx.zoom, ctx.ox, ctx.oy),
                );
                paths.push(pb.build().map_err(|error| error.to_string())?);
            }
            Ok(PreparedMarker::Curves { paths, color })
        }
    }
}

fn paint_prepared_marker(
    marker: Option<&PreparedMarker>,
    window: &mut Window,
    dx: f32,
    dy: f32,
) {
    let Some(marker) = marker else {
        return;
    };
    match marker {
        PreparedMarker::Fill { path, color } | PreparedMarker::Stroke { path, color } => {
            paint_cached_path(window, path, *color, dx, dy);
        }
        PreparedMarker::FillStroke {
            fill,
            stroke,
            fill_color,
            stroke_color,
        } => {
            paint_cached_path(window, fill, *fill_color, dx, dy);
            paint_cached_path(window, stroke, *stroke_color, dx, dy);
        }
        PreparedMarker::Circle { path, color } => {
            paint_cached_path(window, path, *color, dx, dy);
        }
        PreparedMarker::Curves { paths, color } => {
            for path in paths {
                paint_cached_path(window, path, *color, dx, dy);
            }
        }
    }
}
