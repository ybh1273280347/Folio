use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Local;
use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat, ImageSource, SharedUri};

const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "svg"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PastedImageSource {
    Bytes {
        extension: &'static str,
        bytes: Vec<u8>,
    },
    CopyFile(PathBuf),
}

pub fn is_supported_image_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    SUPPORTED_IMAGE_EXTENSIONS.contains(&lower.as_str())
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

/// Extract valid SVG XML bytes from clipboard plain text (e.g. draw.io "Copy as SVG").
pub fn extract_svg_text(text: &str) -> Option<Vec<u8>> {
    let trimmed = text
        .trim_matches(|ch: char| ch == '\0' || ch == '\u{feff}' || ch.is_whitespace());
    if trimmed.is_empty() || trimmed.len() > 16 * 1024 * 1024 {
        return None;
    }
    let lower_head: String = trimmed
        .chars()
        .take(256)
        .collect::<String>()
        .to_ascii_lowercase();
    let looks_like_svg = lower_head.starts_with("<svg")
        || (lower_head.starts_with("<?xml") && lower_head.contains("<svg"))
        || (lower_head.starts_with("<!doctype svg") && lower_head.contains("<svg"));
    if !looks_like_svg {
        return None;
    }
    let doc = roxmltree::Document::parse(trimmed).ok()?;
    if doc.root_element().tag_name().name().eq_ignore_ascii_case("svg") {
        Some(trimmed.as_bytes().to_vec())
    } else {
        None
    }
}

fn convert_bytes_to_png(bytes: &[u8], src_format: image::ImageFormat) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory_with_format(bytes, src_format)
        .map_err(|err| format!("Failed to decode clipboard image: {err}"))?;
    let mut out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|err| format!("Failed to encode clipboard image as PNG: {err}"))?;
    Ok(out)
}

fn clipboard_image_to_bytes(img: &Image) -> Result<PastedImageSource, String> {
    let (extension, bytes) = match img.format {
        ImageFormat::Png => ("png", img.bytes.clone()),
        ImageFormat::Jpeg => ("jpg", img.bytes.clone()),
        ImageFormat::Webp => ("webp", img.bytes.clone()),
        ImageFormat::Gif => ("gif", img.bytes.clone()),
        ImageFormat::Svg => ("svg", sanitize_svg_bytes(&img.bytes)),
        ImageFormat::Bmp => ("png", convert_bytes_to_png(&img.bytes, image::ImageFormat::Bmp)?),
        ImageFormat::Tiff => ("png", convert_bytes_to_png(&img.bytes, image::ImageFormat::Tiff)?),
        ImageFormat::Ico => ("png", convert_bytes_to_png(&img.bytes, image::ImageFormat::Ico)?),
        ImageFormat::Pnm => ("png", convert_bytes_to_png(&img.bytes, image::ImageFormat::Pnm)?),
    };
    Ok(PastedImageSource::Bytes { extension, bytes })
}

/// Extract pasteable image/SVG entries from a GPUI `ClipboardItem`.
/// Returns an empty vector when the clipboard contains no images or SVG payloads.
pub fn extract_images_from_clipboard(item: &ClipboardItem) -> Result<Vec<PastedImageSource>, String> {
    let mut external_images = Vec::new();
    let mut direct_image: Option<(ImageFormat, PastedImageSource)> = None;
    let mut svg_text: Option<Vec<u8>> = None;

    for entry in item.entries() {
        match entry {
            ClipboardEntry::ExternalPaths(ext) => {
                for path in ext.paths() {
                    if is_supported_image_path(path) {
                        external_images.push(PastedImageSource::CopyFile(path.to_path_buf()));
                    }
                }
            }
            ClipboardEntry::Image(img) if direct_image.is_none() => {
                direct_image = Some((img.format, clipboard_image_to_bytes(img)?));
            }
            ClipboardEntry::String(s) if svg_text.is_none() => {
                svg_text = extract_svg_text(&s.text);
            }
            _ => {}
        }
    }

    if !external_images.is_empty() {
        return Ok(external_images);
    }

    match (direct_image, svg_text) {
        (Some((ImageFormat::Svg, source)), _) => Ok(vec![source]),
        // Some apps place both SVG markup text and a fallback DIB bitmap on the clipboard.
        (Some((ImageFormat::Bmp, _)), Some(bytes)) | (None, Some(bytes)) => {
            Ok(vec![PastedImageSource::Bytes {
                extension: "svg",
                bytes,
            }])
        }
        (Some((_, source)), _) => Ok(vec![source]),
        (None, None) => Ok(Vec::new()),
    }
}

pub fn unique_destination_path(target_dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let clean_stem = if stem.trim().is_empty() {
        "image"
    } else {
        stem.trim()
    };
    let clean_ext = extension.trim_start_matches('.');
    let first = if clean_ext.is_empty() {
        target_dir.join(clean_stem)
    } else {
        target_dir.join(format!("{clean_stem}.{clean_ext}"))
    };
    if !first.exists() {
        return first;
    }
    for index in 1usize.. {
        let candidate = if clean_ext.is_empty() {
            target_dir.join(format!("{clean_stem}-{index}"))
        } else {
            target_dir.join(format!("{clean_stem}-{index}.{clean_ext}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded range always produces a unique path")
}

/// Persist all extracted clipboard images/SVGs into `target_dir` without prompting.
pub fn save_clipboard_images_to_dir(
    item: &ClipboardItem,
    target_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let sources = extract_images_from_clipboard(item)?;
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(target_dir)
        .map_err(|err| format!("Failed to create image directory {}: {err}", target_dir.display()))?;

    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut saved = Vec::with_capacity(sources.len());

    for source in sources {
        let dest = match source {
            PastedImageSource::Bytes { extension, bytes } => {
                let stem = format!("paste-{timestamp}");
                let dest = unique_destination_path(target_dir, &stem, extension);
                fs::write(&dest, bytes)
                    .map_err(|err| format!("Failed to write {}: {err}", dest.display()))?;
                dest
            }
            PastedImageSource::CopyFile(src) => {
                let stem = src
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("image");
                let ext = src
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("png");
                let dest = unique_destination_path(target_dir, stem, ext);
                fs::copy(&src, &dest).map_err(|err| {
                    format!("Failed to copy {} to {}: {err}", src.display(), dest.display())
                })?;
                dest
            }
        };
        saved.push(dest);
    }

    Ok(saved)
}

/// Directory `<md_dir>/images/<md_stem>/` for Markdown clipboard image pastes.
pub fn markdown_paste_image_dir(markdown_path: &Path) -> PathBuf {
    let parent = markdown_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = markdown_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("document");
    parent.join("images").join(stem)
}

/// Relative forward-slash path from `markdown_path`'s parent directory to `saved_image`.
pub fn markdown_relative_image_ref(markdown_path: &Path, saved_image: &Path) -> String {
    let doc_dir = markdown_path.parent().unwrap_or_else(|| Path::new("."));
    let rel = saved_image.strip_prefix(doc_dir).unwrap_or(saved_image);
    rel.to_string_lossy().replace('\\', "/")
}

/// Resolve a non-remote Markdown image URI to a local filesystem path when applicable.
pub fn resolve_markdown_image_path(doc_dir: &Path, uri: &str) -> Option<PathBuf> {
    let raw = uri.trim();
    if raw.is_empty()
        || raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("data:")
    {
        return None;
    }
    if raw.starts_with("file://") {
        return url::Url::parse(raw).ok()?.to_file_path().ok();
    }

    let candidate_path = Path::new(raw);
    let resolved = if candidate_path.is_absolute() {
        candidate_path.to_path_buf()
    } else {
        let direct = doc_dir.join(candidate_path);
        if direct.exists() {
            direct
        } else {
            let decoded = crate::files::percent_decode(raw);
            let decoded_path = doc_dir.join(Path::new(&decoded));
            if decoded_path.exists() {
                decoded_path
            } else {
                direct
            }
        }
    };
    Some(resolved)
}

/// Resolve Markdown image URIs relative to the owning document's directory.
pub fn resolve_markdown_image_source(doc_dir: &Path, uri: &SharedUri) -> ImageSource {
    match resolve_markdown_image_path(doc_dir, uri.as_ref()) {
        Some(path) => path.as_path().into(),
        None => uri.clone().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_valid_drawio_svg_text() {
        let svg = r#"  <svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50"/></svg>  "#;
        let extracted = extract_svg_text(svg).expect("valid SVG should extract");
        assert!(extracted.starts_with(b"<svg"));
        assert!(extracted.ends_with(b"</svg>"));
    }

    #[test]
    fn extracts_xml_prefixed_svg_text() {
        let svg = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>"#;
        assert!(extract_svg_text(svg).is_some());
    }

    #[test]
    fn rejects_non_svg_text_and_non_svg_xml() {
        assert!(extract_svg_text("hello <svg></svg>").is_none());
        assert!(extract_svg_text("<div>not svg</div>").is_none());
        assert!(extract_svg_text("<svg>unclosed").is_none());
    }

    #[test]
    fn computes_scheme_b_dir_and_relative_ref() {
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("notes").join("chapter-1.md");
        let img_dir = markdown_paste_image_dir(&md);
        assert_eq!(img_dir, dir.path().join("notes").join("images").join("chapter-1"));
        let saved = img_dir.join("paste-20261121-234500.svg");
        assert_eq!(
            markdown_relative_image_ref(&md, &saved),
            "images/chapter-1/paste-20261121-234500.svg"
        );
    }

    #[test]
    fn saves_clipboard_svg_text_with_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#;
        let item = ClipboardItem::new_string(svg.to_owned());

        let first = save_clipboard_images_to_dir(&item, dir.path()).unwrap();
        let second = save_clipboard_images_to_dir(&item, dir.path()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_ne!(first[0], second[0]);
        assert_eq!(first[0].extension().and_then(|e| e.to_str()), Some("svg"));
        assert_eq!(fs::read_to_string(&first[0]).unwrap(), svg);
    }

    #[test]
    fn resolves_relative_markdown_image_path_against_doc_dir() {
        let dir = tempfile::tempdir().unwrap();
        let img_dir = dir.path().join("images").join("doc");
        fs::create_dir_all(&img_dir).unwrap();
        let img_file = img_dir.join("pic 1.png");
        fs::write(&img_file, b"png").unwrap();

        let resolved = resolve_markdown_image_path(dir.path(), "images/doc/pic%201.png");
        assert_eq!(resolved.as_deref(), Some(img_file.as_path()));
        assert_eq!(resolve_markdown_image_path(dir.path(), "https://example.com/a.png"), None);
    }

    #[test]
    fn strips_trailing_nul_bytes_from_clipboard_svg_image() {
        let dir = tempfile::tempdir().unwrap();
        let raw_svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\"/></svg>\0";
        let item = ClipboardItem::new_image(&Image {
            format: ImageFormat::Svg,
            bytes: raw_svg.to_vec(),
            id: 1,
        });
        let saved = save_clipboard_images_to_dir(&item, dir.path()).unwrap();
        assert_eq!(saved.len(), 1);
        let written = fs::read(&saved[0]).unwrap();
        assert!(!written.ends_with(b"\0"));
        assert!(usvg::Tree::from_data(&written, &usvg::Options::default()).is_ok());
    }
}
