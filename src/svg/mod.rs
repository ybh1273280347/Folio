//! Shared SVG input compatibility and font resolution for Folio's renderers.

mod compat;
mod fonts;

pub(crate) use compat::preprocess_svg_bytes;
pub(crate) use fonts::usvg_options;

/// Strip trailing NUL bytes that Windows clipboard providers (e.g. Chromium/draw.io)
/// often append to `image/svg+xml` buffers.
pub(crate) fn sanitize_svg_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    bytes[..end].to_vec()
}
