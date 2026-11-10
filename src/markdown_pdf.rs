//! Vector PDF publication consumes GPUI's document paint operations, without a second layout engine.
use crate::{compiler::BuildResult, model::BuildStatus};
use gpui_kit::*;
use krilla::{
    Document,
    geom::{PathBuilder, Rect, Transform},
    num::NormalizedF32,
    page::PageSettings,
    paint::Fill,
    text::{Font as PdfFont, GlyphId as PdfGlyphId, KrillaGlyph},
};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

pub struct ExportRequest {
    pub destination: PathBuf,
    pub preview: PathBuf,
    pub cancelled: Arc<AtomicBool>,
    pub complete: Box<dyn FnOnce(io::Result<BuildResult>) + Send>,
    pub started: Instant,
}
pub type FontFaces = HashMap<FontId, (Vec<u8>, u32)>;

pub fn fonts(ops: &[VectorPaint], cx: &App) -> io::Result<FontFaces> {
    let mut faces = HashMap::new();
    for op in ops {
        if let VectorPaint::Glyph { font_id, .. } = op {
            if !faces.contains_key(font_id) {
                faces.insert(
                    *font_id,
                    cx.text_system()
                        .font_data(*font_id)
                        .map_err(io::Error::other)?,
                );
            }
        }
    }
    Ok(faces)
}

pub fn publish(
    request: ExportRequest,
    ops: Vec<VectorPaint>,
    size: Size<Pixels>,
    faces: io::Result<FontFaces>,
) {
    let result = faces.and_then(|faces| write(&request, &ops, size, faces));
    (request.complete)(result);
}

fn write(
    request: &ExportRequest,
    ops: &[VectorPaint],
    size: Size<Pixels>,
    faces: FontFaces,
) -> io::Result<BuildResult> {
    let cancelled = || BuildResult {
        status: BuildStatus::Cancelled,
        elapsed: request.started.elapsed(),
        log: "PDF 导出已取消。".into(),
        preview: None,
    };
    if request.cancelled.load(Ordering::Relaxed) {
        return Ok(cancelled());
    }
    let mut fonts = HashMap::new();
    let mut characters = HashMap::new();
    let mut glyph_bounds = HashMap::new();
    for (id, (bytes, index)) in faces {
        // Most paint calls carry original text. Reverse cmap covers standalone UI marks.
        let face = ttf_parser::Face::parse(&bytes, index).map_err(io::Error::other)?;
        for op in ops {
            if let VectorPaint::Glyph {
                font_id, glyph_id, ..
            } = op
            {
                if *font_id == id {
                    if let Some(b) = face.glyph_bounding_box(ttf_parser::GlyphId(glyph_id.0 as u16))
                    {
                        let em = face.units_per_em() as f32;
                        glyph_bounds.insert(
                            (id, *glyph_id),
                            (
                                b.x_min as f32 / em,
                                -(b.y_max as f32) / em,
                                (b.x_max - b.x_min) as f32 / em,
                                (b.y_max - b.y_min) as f32 / em,
                            ),
                        );
                    }
                }
            }
        }
        let mut cmap = HashMap::new();
        for table in face
            .tables()
            .cmap
            .into_iter()
            .flat_map(|cmap| cmap.subtables)
        {
            if table.is_unicode() {
                table.codepoints(|code| {
                    if let (Some(ch), Some(glyph)) = (char::from_u32(code), table.glyph_index(code))
                    {
                        cmap.entry(glyph.0 as u32).or_insert(ch);
                    }
                });
            }
        }
        characters.insert(id, cmap);
        fonts.insert(
            id,
            PdfFont::new(bytes.into(), index)
                .ok_or_else(|| io::Error::other("无法将当前字体嵌入 PDF"))?,
        );
    }
    let mut document = Document::new();
    let scale = 595.276 / f32::from(size.width);
    let page_height = 841.89 / scale;
    let mut top = 0.;
    let height = f32::from(size.height);
    while top < height {
        if request.cancelled.load(Ordering::Relaxed) {
            return Ok(cancelled());
        }
        let mut bottom = (top + page_height).min(height);
        // Move a normal block to the next page instead of cutting a formula or short paragraph.
        for op in ops {
            if let VectorPaint::Block(bounds) = op {
                let start = f32::from(bounds.top());
                let end = f32::from(bounds.bottom());
                if start > top
                    && start < bottom
                    && end > bottom
                    && f32::from(bounds.size.height) < page_height
                {
                    bottom = start;
                }
            }
        }
        // Very long paragraphs may span pages; cut between their actual glyph bounds.
        for op in ops {
            if let VectorPaint::Glyph {
                origin,
                font_id,
                glyph_id,
                font_size,
                ..
            } = op
            {
                if let Some((_, y, _, h)) = glyph_bounds.get(&(*font_id, *glyph_id)) {
                    let start = f32::from(origin.y) + y * f32::from(*font_size);
                    let end = start + h * f32::from(*font_size);
                    if start > top && start < bottom && end > bottom {
                        bottom = start;
                    }
                }
            }
        }
        let mut page = document.start_page_with(PageSettings::from_wh(595.276, 841.89).unwrap());
        let mut surface = page.surface();
        surface.push_transform(&Transform::from_scale(scale, scale));
        surface.push_transform(&Transform::from_translate(0., -top));
        let page_clip = rectangle(0., top, f32::from(size.width), bottom - top).unwrap();
        surface.push_clip_path(&page_clip, &Default::default());
        for op in ops {
            // Do not emit off-page text: PDF readers can extract clipped text as duplicates.
            let (start, end) = match op {
                VectorPaint::Glyph {
                    origin,
                    font_id,
                    glyph_id,
                    font_size,
                    ..
                } => {
                    let (_, y, _, h) = glyph_bounds
                        .get(&(*font_id, *glyph_id))
                        .copied()
                        .unwrap_or((0., 0., 0., 0.001));
                    let start = f32::from(origin.y) + y * f32::from(*font_size);
                    (start, start + h * f32::from(*font_size))
                }
                VectorPaint::Quad { quad, .. } => (
                    f32::from(quad.bounds.top()),
                    f32::from(quad.bounds.bottom()),
                ),
                VectorPaint::Path(path) => (
                    f32::from(path.bounds.top()),
                    f32::from(path.bounds.bottom()),
                ),
                VectorPaint::Image { bounds, .. } => {
                    (f32::from(bounds.top()), f32::from(bounds.bottom()))
                }
                VectorPaint::Mask { bounds, .. } => {
                    (f32::from(bounds.top()), f32::from(bounds.bottom()))
                }
                _ => continue,
            };
            if end <= top || start >= bottom {
                continue;
            }
            let bounds = match op {
                VectorPaint::Glyph { clip, .. }
                | VectorPaint::Quad { clip, .. }
                | VectorPaint::Image { clip, .. }
                | VectorPaint::Mask { clip, .. } => *clip,
                VectorPaint::Path(path) => path.content_mask.bounds,
                VectorPaint::Block(_) | VectorPaint::PendingImage => continue,
            };
            let Some(clip) = rectangle(
                f32::from(bounds.left()),
                f32::from(bounds.top()),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ) else {
                continue;
            };
            surface.push_clip_path(&clip, &Default::default());
            match op {
                VectorPaint::Glyph {
                    origin,
                    font_id,
                    glyph_id,
                    font_size,
                    color,
                    text,
                    ..
                } => {
                    let fallback = characters[font_id]
                        .get(&glyph_id.0)
                        .map(char::to_string)
                        .unwrap_or_default();
                    let text = text.as_deref().unwrap_or(&fallback);
                    set_fill(&mut surface, *color);
                    surface.draw_glyphs(
                        krilla::geom::Point::from_xy(f32::from(origin.x), f32::from(origin.y)),
                        &[KrillaGlyph::new(
                            PdfGlyphId::new(glyph_id.0),
                            0.,
                            0.,
                            0.,
                            0.,
                            0..text.len(),
                            None,
                        )],
                        fonts[font_id].clone(),
                        text,
                        f32::from(*font_size),
                        false,
                    );
                }
                VectorPaint::Quad { quad, .. } => {
                    let b = quad.bounds;
                    let (x, y, w, h) = (
                        f32::from(b.left()),
                        f32::from(b.top()),
                        f32::from(b.size.width),
                        f32::from(b.size.height),
                    );
                    let color = quad
                        .background
                        .as_solid()
                        .ok_or_else(|| io::Error::other("PDF 不支持当前渐变背景"))?;
                    let outline = rounded_rectangle(b, quad.corner_radii);
                    surface.push_clip_path(&outline, &Default::default());
                    if color.a > 0. {
                        set_fill(&mut surface, color);
                        surface.draw_path(&outline);
                    }
                    set_fill(&mut surface, quad.border_color);
                    for (x, y, w, h) in [
                        (x, y, w, f32::from(quad.border_widths.top)),
                        (
                            x,
                            y + h - f32::from(quad.border_widths.bottom),
                            w,
                            f32::from(quad.border_widths.bottom),
                        ),
                        (x, y, f32::from(quad.border_widths.left), h),
                        (
                            x + w - f32::from(quad.border_widths.right),
                            y,
                            f32::from(quad.border_widths.right),
                            h,
                        ),
                    ] {
                        if let Some(path) = rectangle(x, y, w, h) {
                            surface.draw_path(&path);
                        }
                    }
                    surface.pop();
                }
                VectorPaint::Path(path) => {
                    let color = path
                        .color
                        .as_solid()
                        .ok_or_else(|| io::Error::other("PDF 不支持当前渐变路径"))?;
                    set_fill(&mut surface, color);
                    // GPUI paths are tessellated fills; quadratic triangles carry their control point.
                    for triangle in path.vertices.chunks_exact(3) {
                        let mut p = PathBuilder::new();
                        let a = triangle[0].xy_position;
                        let b = triangle[1].xy_position;
                        let c = triangle[2].xy_position;
                        p.move_to(f32::from(a.x), f32::from(a.y));
                        if triangle[1].st_position.x == 0.5 {
                            p.quad_to(
                                f32::from(b.x),
                                f32::from(b.y),
                                f32::from(c.x),
                                f32::from(c.y),
                            );
                        } else {
                            p.line_to(f32::from(b.x), f32::from(b.y));
                            p.line_to(f32::from(c.x), f32::from(c.y));
                        }
                        p.close();
                        if let Some(path) = p.finish() {
                            surface.draw_path(&path);
                        }
                    }
                }
                VectorPaint::Image {
                    bounds,
                    data,
                    frame,
                    grayscale,
                    ..
                } => {
                    let mut rgba = data.as_bytes(*frame).expect("painted image frame").to_vec();
                    for pixel in rgba.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                        if *grayscale {
                            let value = (pixel[0] as f32 * 0.2126
                                + pixel[1] as f32 * 0.7152
                                + pixel[2] as f32 * 0.0722)
                                as u8;
                            pixel[..3].fill(value);
                        }
                    }
                    let dims = data.size(*frame);
                    draw_image(&mut surface, *bounds, rgba, dims);
                }
                VectorPaint::Mask {
                    bounds,
                    size,
                    alpha,
                    color,
                    transformation,
                    ..
                } => {
                    let rgba: Rgba = (*color).into();
                    let pixels = alpha
                        .iter()
                        .flat_map(|alpha| {
                            [
                                (rgba.r * 255.) as u8,
                                (rgba.g * 255.) as u8,
                                (rgba.b * 255.) as u8,
                                (*alpha as f32 * rgba.a) as u8,
                            ]
                        })
                        .collect();
                    let m = transformation.rotation_scale;
                    surface.push_transform(&Transform::from_row(
                        m[0][0],
                        m[1][0],
                        m[0][1],
                        m[1][1],
                        transformation.translation[0],
                        transformation.translation[1],
                    ));
                    draw_image(&mut surface, *bounds, pixels, *size);
                    surface.pop();
                }
                _ => unreachable!(),
            }
            surface.pop();
        }
        surface.pop();
        surface.pop();
        surface.pop();
        surface.finish();
        page.finish();
        top = bottom;
    }
    let pdf = document
        .finish()
        .map_err(|error| io::Error::other(format!("PDF 写入失败：{error:?}")))?;
    if request.cancelled.load(Ordering::Relaxed) {
        return Ok(cancelled());
    }
    std::fs::create_dir_all(request.preview.parent().expect("preview parent"))?;
    std::fs::write(&request.preview, &pdf)?;
    std::fs::write(&request.destination, &pdf)?;
    Ok(BuildResult {
        status: BuildStatus::Success,
        elapsed: request.started.elapsed(),
        log: format!(
            "已按当前 Markdown 字体、公式、换行导出矢量 PDF（文字可选择和复制）：{}",
            request.destination.display()
        ),
        preview: Some(request.preview.clone()),
    })
}
fn rectangle(x: f32, y: f32, w: f32, h: f32) -> Option<krilla::geom::Path> {
    if w <= 0. || h <= 0. {
        return None;
    }
    let mut p = PathBuilder::new();
    p.push_rect(Rect::from_xywh(x, y, w, h)?);
    p.finish()
}
fn rounded_rectangle(bounds: Bounds<Pixels>, corners: Corners<Pixels>) -> krilla::geom::Path {
    let (x, y, w, h) = (
        f32::from(bounds.left()),
        f32::from(bounds.top()),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    );
    let r = corners.clamp_radii_for_quad_size(bounds.size);
    let (tl, tr, br, bl) = (
        f32::from(r.top_left),
        f32::from(r.top_right),
        f32::from(r.bottom_right),
        f32::from(r.bottom_left),
    );
    let k = 0.5522848;
    let mut p = PathBuilder::new();
    p.move_to(x + tl, y);
    p.line_to(x + w - tr, y);
    p.cubic_to(
        x + w - tr + k * tr,
        y,
        x + w,
        y + tr - k * tr,
        x + w,
        y + tr,
    );
    p.line_to(x + w, y + h - br);
    p.cubic_to(
        x + w,
        y + h - br + k * br,
        x + w - br + k * br,
        y + h,
        x + w - br,
        y + h,
    );
    p.line_to(x + bl, y + h);
    p.cubic_to(
        x + bl - k * bl,
        y + h,
        x,
        y + h - bl + k * bl,
        x,
        y + h - bl,
    );
    p.line_to(x, y + tl);
    p.cubic_to(x, y + tl - k * tl, x + tl - k * tl, y, x + tl, y);
    p.close();
    p.finish().expect("painted quad bounds")
}
fn draw_image(
    surface: &mut krilla::surface::Surface<'_>,
    bounds: Bounds<Pixels>,
    pixels: Vec<u8>,
    size: Size<DevicePixels>,
) {
    let image = krilla::image::Image::from_rgba8(pixels, size.width.0 as u32, size.height.0 as u32);
    surface.push_transform(&Transform::from_translate(
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
    ));
    surface.draw_image(
        image,
        krilla::geom::Size::from_wh(f32::from(bounds.size.width), f32::from(bounds.size.height))
            .unwrap(),
    );
    surface.pop();
}
fn set_fill(surface: &mut krilla::surface::Surface<'_>, color: Hsla) {
    let rgba: Rgba = color.into();
    surface.set_fill(Some(Fill {
        paint: krilla::color::rgb::Color::new(
            (rgba.r * 255.) as u8,
            (rgba.g * 255.) as u8,
            (rgba.b * 255.) as u8,
        )
        .into(),
        opacity: NormalizedF32::new(color.a).unwrap(),
        rule: Default::default(),
    }));
}
