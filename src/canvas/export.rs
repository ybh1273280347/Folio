//! Vector SVG, raster PNG, and vector PDF export for canvas selections or full scenes.

use super::{
    interaction::{EdgeRoute, route_edge_for_doc},
    model::{
        CanvasDocument, CanvasEdge, CanvasNode, CanvasNodeType, EdgeEnd, EdgeKind,
    },
};
use crate::theme::Palette;
use krilla::{Document, geom::Size as KrillaSize, page::PageSettings};
use krilla_svg::{SurfaceExt, SvgSettings};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

const EXPORT_PADDING: f32 = 24.0;
const PNG_EXPORT_SCALE: f32 = 2.0;
const MAX_PNG_DIM: f32 = 8192.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Svg,
    Png,
}

impl ExportFormat {
    pub const fn ext(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
}

pub fn export_output_path(canvas_path: &Path, format: ExportFormat) -> PathBuf {
    canvas_path.with_extension(format.ext())
}

/// Computes exported nodes, edges, and padded envelope bounding box.
/// Defaults to all visible nodes and edges when neither nodes nor an edge are selected.
pub fn collect_export_subset<'a>(
    doc: &'a CanvasDocument,
    selected_nodes: &BTreeSet<String>,
    selected_edge: Option<&str>,
) -> Result<(Vec<&'a CanvasNode>, Vec<&'a CanvasEdge>, ExportBounds), String> {
    let mut node_ids = BTreeSet::new();
    let mut edge_ids = BTreeSet::new();
    let visible = doc.visible_nodes();

    if !selected_nodes.is_empty() {
        for id in selected_nodes {
            if visible.contains_key(id.as_str()) {
                node_ids.insert(id.clone());
            }
        }
        for edge in &doc.edges {
            if node_ids.contains(&edge.from_node) && node_ids.contains(&edge.to_node) {
                edge_ids.insert(edge.id.clone());
            }
        }
    } else if let Some(eid) = selected_edge
        && let Some(edge) = doc.edge(eid)
        && visible.contains_key(edge.from_node.as_str())
        && visible.contains_key(edge.to_node.as_str())
    {
        node_ids.insert(edge.from_node.clone());
        node_ids.insert(edge.to_node.clone());
        edge_ids.insert(edge.id.clone());
    } else {
        for node in &doc.nodes {
            if visible.contains_key(node.id.as_str()) {
                node_ids.insert(node.id.clone());
            }
        }
        for edge in &doc.edges {
            if node_ids.contains(&edge.from_node) && node_ids.contains(&edge.to_node) {
                edge_ids.insert(edge.id.clone());
            }
        }
    }

    let nodes: Vec<&CanvasNode> = doc
        .nodes
        .iter()
        .filter(|n| node_ids.contains(&n.id))
        .collect();
    if nodes.is_empty() {
        return Err(crate::i18n::canvas::ERR_EXPORT_EMPTY.get().to_owned());
    }
    let edges: Vec<&CanvasEdge> = doc
        .edges
        .iter()
        .filter(|e| edge_ids.contains(&e.id))
        .collect();

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for node in &nodes {
        min_x = min_x.min(node.x);
        min_y = min_y.min(node.y);
        max_x = max_x.max(node.x + node.width);
        max_y = max_y.max(node.y + node.height);
    }

    for edge in &edges {
        if let Some(route) = route_edge_for_doc(doc, edge, |id| visible.get(id).copied()) {
            for (px, py) in route.control_bounds_points().into_iter().flatten() {
                min_x = min_x.min(px - 8.0);
                min_y = min_y.min(py - 8.0);
                max_x = max_x.max(px + 8.0);
                max_y = max_y.max(py + 8.0);
            }
        }
    }

    let bounds = ExportBounds {
        min_x: min_x - EXPORT_PADDING,
        min_y: min_y - EXPORT_PADDING,
        width: (max_x - min_x + EXPORT_PADDING * 2.0).max(64.0),
        height: (max_y - min_y + EXPORT_PADDING * 2.0).max(64.0),
    };
    Ok((nodes, edges, bounds))
}

pub fn export_canvas_to_file(
    doc: &CanvasDocument,
    canvas_path: &Path,
    selected_nodes: &BTreeSet<String>,
    selected_edge: Option<&str>,
    palette: Palette,
    format: ExportFormat,
) -> Result<PathBuf, String> {
    let dest = export_output_path(canvas_path, format);
    let canvas_dir = canvas_path.parent().unwrap_or_else(|| Path::new("."));
    let svg = render_svg(doc, canvas_dir, selected_nodes, selected_edge, palette)?;
    let bytes = match format {
        ExportFormat::Svg => svg.into_bytes(),
        ExportFormat::Png => svg_to_png(&svg)?,
        ExportFormat::Pdf => svg_to_pdf(&svg)?,
    };
    fs::write(&dest, bytes).map_err(|e| e.to_string())?;
    Ok(dest)
}

pub fn render_svg(
    doc: &CanvasDocument,
    canvas_dir: &Path,
    selected_nodes: &BTreeSet<String>,
    selected_edge: Option<&str>,
    palette: Palette,
) -> Result<String, String> {
    let (nodes, edges, bounds) = collect_export_subset(doc, selected_nodes, selected_edge)?;
    let bg_hex = hex_rgb(palette.background);
    let surface_hex = hex_rgb(palette.surface);
    let border_hex = hex_rgb(palette.border);
    let text_hex = hex_rgb(palette.text);
    let muted_hex = hex_rgb(palette.muted);
    let accent_hex = hex_rgb(palette.accent);
    let hover_hex = hex_rgb(palette.hover);

    let mut defs = String::new();
    let mut body = String::new();

    let _ = writeln!(
        body,
        r#"  <rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="{bg_hex}"/>"#,
        w = bounds.width,
        h = bounds.height,
    );

    // 1. Group containers underneath.
    for node in nodes.iter().filter(|n| n.node_type == CanvasNodeType::Group) {
        let x = node.x - bounds.min_x;
        let y = node.y - bounds.min_y;
        let stroke = resolve_color_hex(node.color.as_deref(), palette.border);
        let fill = if node.color.is_some() {
            stroke.clone()
        } else {
            hover_hex.clone()
        };
        let opacity = if node.color.is_some() { "0.14" } else { "0.35" };
        let _ = writeln!(
            body,
            r#"  <rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="8" fill="{fill}" fill-opacity="{opacity}" stroke="{stroke}" stroke-width="1.2"/>"#,
            w = node.width,
            h = node.height,
        );
        let label = node.display_text();
        if !label.is_empty() {
            let lx = x + node.width * 0.5;
            let ly = y + 18.0;
            let _ = writeln!(
                body,
                r#"  <text x="{lx:.2}" y="{ly:.2}" font-family="Microsoft YaHei, Segoe UI, sans-serif" font-size="12" fill="{text_hex}" text-anchor="middle">{}</text>"#,
                escape_xml(label)
            );
        }
    }

    // 2. Vector edges & arrowheads (matching canvas view mode routing).
    let nodes_by_id: std::collections::HashMap<_, _> = nodes.iter()
        .map(|&node| (node.id.as_str(), node)).collect();
    let svg_pt = |pt: (f32, f32)| (pt.0 - bounds.min_x, pt.1 - bounds.min_y);
    for edge in &edges {
        let Some(route) = route_edge_for_doc(doc, edge, |id| nodes_by_id.get(id).copied()) else {
            continue;
        };
        let is_ref = edge.kind() == EdgeKind::Reference;
        let stroke = if let Some(raw) = edge.color.as_deref() {
            resolve_color_hex(Some(raw), palette.muted)
        } else if is_ref {
            muted_hex.clone()
        } else {
            accent_hex.clone()
        };
        let stroke_w = if is_ref { "1.6" } else { "2.1" };
        let dash = if is_ref {
            r#" stroke-dasharray="6,4""#
        } else {
            ""
        };
        let d = match route {
            EdgeRoute::Bezier { p0, c1, c2, p3 } => {
                let (sx0, sy0) = svg_pt(p0);
                let (scx1, scy1) = svg_pt(c1);
                let (scx2, scy2) = svg_pt(c2);
                let (sx3, sy3) = svg_pt(p3);
                format!(
                    "M {sx0:.2} {sy0:.2} C {scx1:.2} {scy1:.2}, {scx2:.2} {scy2:.2}, {sx3:.2} {sy3:.2}"
                )
            }
            EdgeRoute::Line { p0, p3 } => {
                let (sx0, sy0) = svg_pt(p0);
                let (sx3, sy3) = svg_pt(p3);
                format!("M {sx0:.2} {sy0:.2} L {sx3:.2} {sy3:.2}")
            }
            EdgeRoute::Elbow { points, radius } => {
                let (sx0, sy0) = svg_pt(points[0]);
                let mut path = format!("M {sx0:.2} {sy0:.2}");
                let cuts = EdgeRoute::elbow_corner_cuts(points, radius);
                for (corner, cut) in [(points[1], cuts[0]), (points[2], cuts[1])] {
                    if let Some((in_pt, c, out_pt)) = cut {
                        let (ix, iy) = svg_pt(in_pt);
                        let (cx, cy) = svg_pt(c);
                        let (ox, oy) = svg_pt(out_pt);
                        let _ = write!(path, " L {ix:.2} {iy:.2} Q {cx:.2} {cy:.2} {ox:.2} {oy:.2}");
                    } else {
                        let (cx, cy) = svg_pt(corner);
                        let _ = write!(path, " L {cx:.2} {cy:.2}");
                    }
                }
                let (sx3, sy3) = svg_pt(points[3]);
                let _ = write!(path, " L {sx3:.2} {sy3:.2}");
                path
            }
        };

        let _ = writeln!(
            body,
            r#"  <path d="{d}" fill="none" stroke="{stroke}" stroke-width="{stroke_w}"{dash}/>"#
        );
        if edge.to_end.unwrap_or(EdgeEnd::Arrow) == EdgeEnd::Arrow {
            let (_, p3) = route.endpoints();
            let tail = route.arrow_tail();
            let (sx3, sy3) = svg_pt(p3);
            let (tx, ty) = svg_pt(tail);
            let dx = sx3 - tx;
            let dy = sy3 - ty;
            let len = (dx * dx + dy * dy).sqrt();
            if len > 1e-3 {
                let ux = dx / len;
                let uy = dy / len;
                let size = 9.0_f32;
                let wing = size * 0.55;
                let bx = sx3 - ux * size;
                let by = sy3 - uy * size;
                let lx = bx - uy * wing;
                let ly = by + ux * wing;
                let rx = bx + uy * wing;
                let ry = by - ux * wing;
                let _ = writeln!(
                    body,
                    r#"  <polygon points="{sx3:.2},{sy3:.2} {lx:.2},{ly:.2} {rx:.2},{ry:.2}" fill="{stroke}"/>"#
                );
            }
        }
    }

    // 3. Content cards & embedded images.
    for (idx, node) in nodes
        .iter()
        .filter(|n| n.node_type != CanvasNodeType::Group)
        .enumerate()
    {
        let x = node.x - bounds.min_x;
        let y = node.y - bounds.min_y;
        let w = node.width;
        let h = node.height;

        if node.is_image_file() {
            let clip_id = format!("img-clip-{idx}");
            let _ = writeln!(
                defs,
                r#"    <clipPath id="{clip_id}"><rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="8" ry="8"/></clipPath>"#
            );
            if let Some(rel) = node.file.as_deref()
                && let Some(data_uri) = load_image_data_uri(canvas_dir, rel)
            {
                let caption_h = if node.text.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                    22.0
                } else {
                    0.0
                };
                let ih = (h - caption_h).max(16.0);
                let _ = writeln!(
                    body,
                    r#"  <image x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{ih:.2}" preserveAspectRatio="xMidYMid meet" clip-path="url(#{clip_id})" href="{data_uri}"/>"#
                );
                if caption_h > 0.0
                    && let Some(caption) = node.text.as_deref()
                {
                    let cy = y + ih + 15.0;
                    let cx = x + w * 0.5;
                    let _ = writeln!(
                        body,
                        r#"  <text x="{cx:.2}" y="{cy:.2}" font-family="Microsoft YaHei, Segoe UI, sans-serif" font-size="11" fill="{text_hex}" text-anchor="middle">{}</text>"#,
                        escape_xml(caption)
                    );
                }
            }
            if node.color.is_some() {
                let stroke = resolve_color_hex(node.color.as_deref(), palette.border);
                let _ = writeln!(
                    body,
                    r#"  <rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="8" fill="none" stroke="{stroke}" stroke-width="1.5"/>"#
                );
            }
            continue;
        }

        let fill_hex = if node.color.is_some() {
            let tint = resolve_color_u32(node.color.as_deref(), palette.surface);
            blend_hex(palette.surface, tint, 0.12)
        } else {
            surface_hex.clone()
        };
        let stroke = if node.color.is_some() {
            resolve_color_hex(node.color.as_deref(), palette.border)
        } else {
            border_hex.clone()
        };
        let _ = writeln!(
            body,
            r#"  <rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="8" fill="{fill_hex}" stroke="{stroke}" stroke-width="1.2"/>"#
        );

        let mut text_top = y + 8.0;
        let anchor_label = node
            .anchor()
            .map(|a| {
                let name = Path::new(a.target_path())
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| a.target_path().to_owned());
                format!("[{}] {name}", crate::i18n::canvas::anchor_kind_label(a.kind_label()))
            })
            .or_else(|| {
                node.file.as_deref().map(|f| {
                    let name = Path::new(f)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| f.to_owned());
                    format!("[{}] {name}", crate::i18n::canvas::anchor_kind_label("file"))
                })
            });
        if let Some(badge) = anchor_label {
            let bx = x + w * 0.5;
            let by = text_top + 11.0;
            let _ = writeln!(
                body,
                r#"  <text x="{bx:.2}" y="{by:.2}" font-family="Microsoft YaHei, Segoe UI, sans-serif" font-size="10.5" fill="{accent_hex}" text-anchor="middle">{}</text>"#,
                escape_xml(&badge)
            );
            text_top += 18.0;
        }

        let display = node.display_text();
        if !display.is_empty() {
            let font_size = 13.0_f32;
            let line_h = 18.0_f32;
            let max_w = (w - 20.0).max(24.0);
            let lines = wrap_lines(display, max_w, font_size);
            let avail_h = (y + h - 8.0 - text_top).max(line_h);
            let max_lines = ((avail_h / line_h).floor() as usize).max(1);
            let visible_count = lines.len().min(max_lines);
            let total_text_h = visible_count as f32 * line_h;
            let start_y = text_top + ((avail_h - total_text_h) * 0.5).max(0.0) + font_size * 0.95;
            let cx = x + w * 0.5;
            for (i, line) in lines.into_iter().take(visible_count).enumerate() {
                let ly = start_y + i as f32 * line_h;
                let _ = writeln!(
                    body,
                    r#"  <text x="{cx:.2}" y="{ly:.2}" font-family="Microsoft YaHei, Segoe UI, sans-serif" font-size="{font_size:.1}" fill="{text_hex}" text-anchor="middle">{}</text>"#,
                    escape_xml(&line)
                );
            }
        }
    }

    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.2} {h:.2}">"#,
        w = bounds.width,
        h = bounds.height,
    );
    out.push('\n');
    if !defs.is_empty() {
        out.push_str("  <defs>\n");
        out.push_str(&defs);
        out.push_str("  </defs>\n");
    }
    out.push_str(&body);
    out.push_str("</svg>\n");
    Ok(out)
}

pub fn svg_to_png(svg: &str) -> Result<Vec<u8>, String> {
    let preprocessed = crate::svg::preprocess_svg_bytes(svg.as_bytes());
    let options = crate::svg::usvg_options();
    let tree = usvg::Tree::from_data(&preprocessed.bytes, &options).map_err(|e| e.to_string())?;
    let raw = tree.size();
    let scale = PNG_EXPORT_SCALE
        .min(MAX_PNG_DIM / raw.width().max(1.0))
        .min(MAX_PNG_DIM / raw.height().max(1.0))
        .max(0.25);
    let pw = (raw.width() * scale).ceil().max(1.0) as u32;
    let ph = (raw.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph)
        .ok_or_else(|| "Failed to allocate PNG pixmap".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| e.to_string())
}

pub fn svg_to_pdf(svg: &str) -> Result<Vec<u8>, String> {
    let preprocessed = crate::svg::preprocess_svg_bytes(svg.as_bytes());
    let options = crate::svg::usvg_options();
    let tree = usvg::Tree::from_data(&preprocessed.bytes, &options).map_err(|e| e.to_string())?;
    let raw = tree.size();
    let width = (raw.width() * 0.75).max(1.0);
    let height = (raw.height() * 0.75).max(1.0);
    let size = KrillaSize::from_wh(width, height).ok_or("Invalid SVG dimensions")?;
    let settings = PageSettings::from_wh(width, height).ok_or("Invalid PDF page dimensions")?;

    let mut document = Document::new();
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();
    surface
        .draw_svg(&tree, size, SvgSettings::default())
        .ok_or("Failed to draw SVG into PDF")?;
    surface.finish();
    page.finish();
    document.finish().map_err(|e| format!("{e:?}"))
}

fn load_image_data_uri(canvas_dir: &Path, rel: &str) -> Option<String> {
    let p = Path::new(rel);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        canvas_dir.join(p)
    };
    let bytes = fs::read(&full).ok()?;
    let ext = full
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if ext == "svg" {
        return Some(format!(
            "data:image/svg+xml;base64,{}",
            encode_base64(&bytes)
        ));
    }
    if ext == "png" {
        return Some(format!("data:image/png;base64,{}", encode_base64(&bytes)));
    }
    if ext == "jpg" || ext == "jpeg" {
        return Some(format!("data:image/jpeg;base64,{}", encode_base64(&bytes)));
    }
    let decoded = image::load_from_memory(&bytes).ok()?;
    let mut png_out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png_out), image::ImageFormat::Png)
        .ok()?;
    Some(format!("data:image/png;base64,{}", encode_base64(&png_out)))
}

fn wrap_lines(text: &str, max_width: f32, font_size: f32) -> Vec<String> {
    let mut out = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut width = 0.0_f32;
        for ch in raw_line.chars() {
            let cw = if ch.is_ascii() {
                font_size * 0.56
            } else {
                font_size * 1.0
            };
            if width + cw > max_width && !current.is_empty() {
                out.push(current);
                current = String::new();
                width = 0.0;
            }
            current.push(ch);
            width += cw;
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn hex_rgb(hex: u32) -> String {
    format!("#{:06x}", hex & 0x00ff_ffff)
}

fn resolve_color_u32(raw: Option<&str>, fallback: u32) -> u32 {
    match raw {
        Some("1") => 0xd93025,
        Some("2") => 0xe67e22,
        Some("3") => 0xeab308,
        Some("4") => 0x22c55e,
        Some("5") => 0x06b6d4,
        Some("6") => 0x8b5cf6,
        Some(hex) if hex.starts_with('#') && hex.len() == 7 => {
            u32::from_str_radix(&hex[1..], 16).unwrap_or(fallback)
        }
        _ => fallback,
    }
}

fn resolve_color_hex(raw: Option<&str>, fallback: u32) -> String {
    hex_rgb(resolve_color_u32(raw, fallback))
}

fn blend_hex(base: u32, tint: u32, alpha: f32) -> String {
    let inv = 1.0 - alpha;
    let mix = |shift: u32| -> u32 {
        let b = ((base >> shift) & 0xff) as f32;
        let t = ((tint >> shift) & 0xff) as f32;
        (b * inv + t * alpha).round().clamp(0.0, 255.0) as u32
    };
    format!("#{:02x}{:02x}{:02x}", mix(16), mix(8), mix(0))
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::{CanvasOp, CanvasViewMode, TreeLayoutMode};

    #[test]
    fn export_subset_defaults_to_all_and_crops_selection_tightly() {
        let mut doc = CanvasDocument::default();
        doc.apply_ops(&[
            CanvasOp::AddNode {
                id: Some("n1".into()),
                node_type: None,
                text: Some("中心主题".into()),
                file: None,
                x: Some(100.0),
                y: Some(100.0),
                width: Some(200.0),
                height: Some(80.0),
                color: Some("5".into()),
                parent_id: None,
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("n2".into()),
                node_type: None,
                text: Some("Sub".into()),
                file: None,
                x: Some(420.0),
                y: Some(100.0),
                width: Some(200.0),
                height: Some(80.0),
                color: None,
                parent_id: Some("n1".into()),
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("n3".into()),
                node_type: None,
                text: Some("Far away".into()),
                file: None,
                x: Some(1200.0),
                y: Some(900.0),
                width: Some(200.0),
                height: Some(80.0),
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
        ])
        .unwrap();

        let empty = BTreeSet::new();
        let (all_nodes, _, all_bounds) = collect_export_subset(&doc, &empty, None).unwrap();
        assert_eq!(all_nodes.len(), 3);
        assert!(all_bounds.width > 1200.0);

        let mut sel = BTreeSet::new();
        sel.insert("n1".to_owned());
        let (sel_nodes, sel_edges, sel_bounds) = collect_export_subset(&doc, &sel, None).unwrap();
        assert_eq!(sel_nodes.len(), 1);
        assert!(sel_edges.is_empty());
        assert!((sel_bounds.width - (200.0 + EXPORT_PADDING * 2.0)).abs() < 0.5);
        assert!((sel_bounds.height - (80.0 + EXPORT_PADDING * 2.0)).abs() < 0.5);
    }

    #[test]
    fn export_renders_valid_svg_png_and_pdf() {
        let mut doc = CanvasDocument::default();
        doc.nodes.push(CanvasNode::new_text("a", 0.0, 0.0, "Hello Canvas"));
        doc.nodes.push(CanvasNode::new_text("b", 320.0, 40.0, "Target"));
        doc.connect_hierarchy("a", "b").unwrap();

        let palette = crate::theme::palette();
        let empty = BTreeSet::new();
        let svg = render_svg(&doc, Path::new("."), &empty, None, palette).unwrap();
        assert!(svg.contains("<svg "));
        assert!(svg.contains("Hello Canvas"));

        let png = svg_to_png(&svg).unwrap();
        assert!(png.starts_with(b"\x89PNG"));

        let pdf = svg_to_pdf(&svg).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));

        // Freeform defaults to cubic Bezier (`C`), while aligned single-branch Tree mode exports straight lines (`L`)
        // and multi-branch Tree mode exports rounded orthogonal elbows (`Q`).
        assert_eq!(doc.view_mode(), CanvasViewMode::Freeform);
        assert!(svg.contains(" C "));

        doc.set_tree_mode(TreeLayoutMode::MindmapRight);
        let aligned_tree_svg = render_svg(&doc, Path::new("."), &empty, None, palette).unwrap();
        assert!(aligned_tree_svg.contains(" L "));
        assert!(!aligned_tree_svg.contains(" C "));

        doc.nodes.push(CanvasNode::new_text("c", 320.0, 200.0, "Second Branch"));
        doc.connect_hierarchy("a", "c").unwrap();
        doc.set_tree_mode(TreeLayoutMode::MindmapRight);
        let branched_tree_svg = render_svg(&doc, Path::new("."), &empty, None, palette).unwrap();
        assert!(branched_tree_svg.contains(" Q "));
        assert!(!branched_tree_svg.contains(" C "));
    }
}
