//! Shared SVG input compatibility and font resolution for Folio's renderers.

mod compat;
mod fonts;

pub(crate) use compat::preprocess_svg_bytes;
pub(crate) use fonts::usvg_options;

use std::sync::Arc;

use gpui_kit::RenderImage;

const MAX_RASTER_DIMENSION: f32 = 4096.0;

pub(crate) fn bounded_raster_density(tree: &usvg::Tree, density: f32) -> f32 {
    let size = tree.size();
    density.min(MAX_RASTER_DIMENSION / size.width().max(size.height()))
}

/// Rasterize a parsed tree into GPUI's premultiplied BGRA image format.
pub(crate) fn rasterize_tree(tree: &usvg::Tree, density: f32) -> Option<Arc<RenderImage>> {
    let size = tree.size();
    let width = (size.width() * density).ceil().clamp(1.0, MAX_RASTER_DIMENSION) as u32;
    let height = (size.height() * density).ceil().clamp(1.0, MAX_RASTER_DIMENSION) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(density, density),
        &mut pixmap.as_mut(),
    );
    let mut bytes = pixmap.take();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(width, height, bytes).expect("pixmap dimensions match its bytes");
    Some(Arc::new(RenderImage::new([image::Frame::new(image)])))
}

/// Strip trailing NUL bytes that Windows clipboard providers (e.g. Chromium/draw.io)
/// often append to `image/svg+xml` buffers.
pub(crate) fn sanitize_svg_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    bytes[..end].to_vec()
}
