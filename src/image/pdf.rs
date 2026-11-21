use std::{
    fs, io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use image::GenericImageView;
use krilla::{Document, geom::Size as KrillaSize, page::PageSettings};
use krilla_svg::{SurfaceExt, SvgSettings};

use crate::build::{BuildResult, BuildStatus};

const MAX_PDF_POINTS: f32 = 14400.0;

fn cancelled_result(started: Instant) -> BuildResult {
    BuildResult {
        status: BuildStatus::Cancelled,
        elapsed: started.elapsed(),
        log: crate::i18n::media::MD_PDF_CANCELLED.get().into(),
        preview: None,
    }
}

fn clamp_pdf_dimensions(raw_w: f32, raw_h: f32) -> (f32, f32) {
    let mut w = (raw_w * 0.75).max(1.0);
    let mut h = (raw_h * 0.75).max(1.0);
    let max_edge = w.max(h);
    if max_edge > MAX_PDF_POINTS {
        let ratio = MAX_PDF_POINTS / max_edge;
        w = (w * ratio).max(1.0);
        h = (h * ratio).max(1.0);
    }
    (w, h)
}

fn svg_bytes_to_pdf(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let preprocessed = crate::svg::preprocess_svg_bytes(bytes);
    let options = crate::svg::usvg_options();
    let tree = usvg::Tree::from_data(&preprocessed.bytes, &options).map_err(io::Error::other)?;
    let raw = tree.size();
    let (width, height) = clamp_pdf_dimensions(raw.width(), raw.height());
    let size = KrillaSize::from_wh(width, height)
        .ok_or_else(|| io::Error::other("Invalid SVG dimensions"))?;
    let settings = PageSettings::from_wh(width, height)
        .ok_or_else(|| io::Error::other("Invalid PDF page dimensions"))?;

    let mut document = Document::new();
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();
    surface
        .draw_svg(&tree, size, SvgSettings::default())
        .ok_or_else(|| io::Error::other("Failed to draw SVG into PDF"))?;
    surface.finish();
    page.finish();
    document
        .finish()
        .map_err(|err| io::Error::other(crate::i18n::media::md_pdf_write_failed(&err)))
}

fn raster_bytes_to_pdf(bytes: &[u8], ext: &str) -> io::Result<Vec<u8>> {
    let (pixel_w, pixel_h, krilla_img) = match ext {
        "png" => match krilla::image::Image::from_png(krilla::Data::from(bytes.to_vec()), true) {
            Ok(img) => {
                let (w, h) = image::load_from_memory(bytes)
                    .map_err(io::Error::other)?
                    .dimensions();
                (w, h, img)
            }
            Err(_) => decode_rgba_for_krilla(bytes)?,
        },
        "jpg" | "jpeg" => {
            match krilla::image::Image::from_jpeg(krilla::Data::from(bytes.to_vec()), true) {
                Ok(img) => {
                    let (w, h) = image::load_from_memory(bytes)
                        .map_err(io::Error::other)?
                        .dimensions();
                    (w, h, img)
                }
                Err(_) => decode_rgba_for_krilla(bytes)?,
            }
        }
        _ => decode_rgba_for_krilla(bytes)?,
    };

    let (width, height) = clamp_pdf_dimensions(pixel_w.max(1) as f32, pixel_h.max(1) as f32);
    let size = KrillaSize::from_wh(width, height)
        .ok_or_else(|| io::Error::other("Invalid image dimensions"))?;
    let settings = PageSettings::from_wh(width, height)
        .ok_or_else(|| io::Error::other("Invalid PDF page dimensions"))?;

    let mut document = Document::new();
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();
    surface.draw_image(krilla_img, size);
    surface.finish();
    page.finish();
    document
        .finish()
        .map_err(|err| io::Error::other(crate::i18n::media::md_pdf_write_failed(&err)))
}

fn decode_rgba_for_krilla(bytes: &[u8]) -> io::Result<(u32, u32, krilla::image::Image)> {
    let decoded = image::load_from_memory(bytes).map_err(io::Error::other)?;
    let (w, h) = decoded.dimensions();
    let rgba = decoded.to_rgba8();
    Ok((w, h, krilla::image::Image::from_rgba8(rgba.into_raw(), w, h)))
}

/// Export an SVG or raster image file to a single-page PDF sized to the image.
pub fn export_image_to_pdf(
    source: &Path,
    destination: &Path,
    preview: &Path,
    cancelled: Arc<AtomicBool>,
    started: Instant,
) -> io::Result<BuildResult> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(cancelled_result(started));
    }
    let bytes = fs::read(source)?;
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let pdf = if ext == "svg" {
        svg_bytes_to_pdf(&bytes)?
    } else {
        raster_bytes_to_pdf(&bytes, &ext)?
    };

    if cancelled.load(Ordering::Relaxed) {
        return Ok(cancelled_result(started));
    }

    if let Some(parent) = preview.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(preview, &pdf)?;
    fs::write(destination, &pdf)?;

    Ok(BuildResult {
        status: BuildStatus::Success,
        elapsed: started.elapsed(),
        log: crate::i18n::media::md_pdf_exported(destination),
        preview: Some(preview.to_path_buf()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_svg_with_trailing_nul_to_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("diagram.svg");
        let dest = dir.path().join("diagram.pdf");
        let preview = dir.path().join("preview.pdf");
        fs::write(
            &src,
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"100\"><rect width=\"200\" height=\"100\" fill=\"#2563eb\"/></svg>\0",
        )
        .unwrap();

        let res = export_image_to_pdf(
            &src,
            &dest,
            &preview,
            Arc::new(AtomicBool::new(false)),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(res.status, BuildStatus::Success);
        assert!(fs::read(&dest).unwrap().starts_with(b"%PDF-"));
        assert!(fs::read(&preview).unwrap().starts_with(b"%PDF-"));
    }

    #[test]
    fn exports_png_to_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pic.png");
        let dest = dir.path().join("pic.pdf");
        let preview = dir.path().join("preview.pdf");

        let img = image::RgbaImage::from_pixel(16, 12, image::Rgba([255, 0, 0, 255]));
        img.save_with_format(&src, image::ImageFormat::Png).unwrap();

        let res = export_image_to_pdf(
            &src,
            &dest,
            &preview,
            Arc::new(AtomicBool::new(false)),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(res.status, BuildStatus::Success);
        assert!(fs::read(&dest).unwrap().starts_with(b"%PDF-"));
    }
}
