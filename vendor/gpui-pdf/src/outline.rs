//! PDF outline (bookmarks / "table of contents") extraction.
//!
//! Walks the document's `/Outlines` tree through hayro-syntax's low-level object
//! API and flattens it to `(title, nesting depth, target page index)`. Pure Rust,
//! no extra deps. Destinations given as an explicit `[pageRef /XYZ …]` array (and
//! `/A` GoTo actions wrapping one) resolve to a page index; named destinations are
//! left unresolved for now (the title still shows). Malformed trees are bounded by
//! a visited-set + item/-depth caps so a cyclic or huge outline can't hang us.

use std::collections::{HashMap, HashSet};

use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{
    Array, Dict, MaybeRef, Name, ObjRef, Object, Rect, String as PdfString,
};
use hayro::hayro_syntax::page::Rotation;

/// One entry in a PDF's outline, flattened depth-first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    /// The bookmark label.
    pub title: String,
    /// Nesting depth, 0 = top level.
    pub level: usize,
    /// 0-based target page index, or `None` if the destination couldn't be resolved.
    pub page: Option<usize>,
}

/// Hard caps so a malformed/hostile outline can't hang or OOM us.
const MAX_ITEMS: usize = 10_000;
const MAX_DEPTH: usize = 32;

/// Extract the document outline (bookmarks), flattened depth-first. Returns an
/// empty vec when the PDF has no `/Outlines`.
pub fn outline(doc: &Pdf) -> Vec<OutlineItem> {
    let xref = doc.xref();
    let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
        return Vec::new();
    };
    let Some(outlines) = catalog.get::<Dict>("Outlines") else {
        return Vec::new();
    };
    let Some(first) = outlines.get_ref("First") else {
        return Vec::new();
    };

    let page_index = build_page_index(doc);
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    walk_items(doc, first, 0, &page_index, &mut visited, &mut out);
    out
}

/// Follow `/Next` siblings, recursing into `/First` children one level deeper.
fn walk_items(
    doc: &Pdf,
    start: ObjRef,
    level: usize,
    page_index: &HashMap<ObjRef, usize>,
    visited: &mut HashSet<ObjRef>,
    out: &mut Vec<OutlineItem>,
) {
    if level > MAX_DEPTH {
        return;
    }
    let xref = doc.xref();
    let mut cur = Some(start);
    while let Some(r) = cur {
        if out.len() >= MAX_ITEMS || !visited.insert(r) {
            return;
        }
        let Some(item) = xref.get::<Dict>(r.into()) else {
            return;
        };
        if let Some(title) = item.get::<PdfString>("Title") {
            out.push(OutlineItem {
                title: decode_pdf_string(title.as_bytes()),
                level,
                page: resolve_dest_page(doc, &item, page_index),
            });
        }
        if let Some(child) = item.get_ref("First") {
            walk_items(doc, child, level + 1, page_index, visited, out);
        }
        cur = item.get_ref("Next");
    }
}

/// Resolve explicit and named `/Dest` values, including GoTo actions and name trees.
fn resolve_dest_page(doc: &Pdf, item: &Dict, page_index: &HashMap<ObjRef, usize>) -> Option<usize> {
    let action = item.get::<Dict>("A");
    let dest = item
        .get::<Object>("Dest")
        .or_else(|| action.as_ref().and_then(|a| a.get::<Object>("D")))?;
    resolve_destination(&dest, page_index)
        .or_else(|| named_destination_page(doc, &dest, page_index))
}

fn resolve_destination(dest: &Object, page_index: &HashMap<ObjRef, usize>) -> Option<usize> {
    match dest {
        Object::Array(dest) => dest_array_page(dest, page_index),
        Object::Dict(dest) => dest
            .get::<Array>("D")
            .and_then(|dest| dest_array_page(&dest, page_index)),
        _ => None,
    }
}

fn named_destination_page(
    doc: &Pdf,
    dest: &Object,
    page_index: &HashMap<ObjRef, usize>,
) -> Option<usize> {
    let key = match dest {
        Object::Name(name) => std::str::from_utf8(name.as_ref()).ok()?.to_owned(),
        Object::String(name) => decode_pdf_string(name.as_bytes()),
        _ => return None,
    };
    let xref = doc.xref();
    let catalog = xref.get::<Dict>(xref.root_id())?;
    if let Some(legacy) = catalog.get::<Dict>("Dests")
        && let Some(value) = legacy.get::<Object>(key.as_bytes())
        && let Some(page) = resolve_destination(&value, page_index)
    {
        return Some(page);
    }
    let names = catalog.get::<Dict>("Names")?.get::<Dict>("Dests")?;
    name_tree_destination(&names, &key, page_index)
}

fn name_tree_destination(
    node: &Dict,
    target: &str,
    page_index: &HashMap<ObjRef, usize>,
) -> Option<usize> {
    if let Some(names) = node.get::<Array>("Names") {
        let values: Vec<Object> = names.iter().collect();
        for pair in values.chunks_exact(2) {
            let name = match &pair[0] {
                Object::String(name) => decode_pdf_string(name.as_bytes()),
                Object::Name(name) => name.as_str().to_owned(),
                _ => continue,
            };
            if name == target {
                return resolve_destination(&pair[1], page_index);
            }
        }
    }
    node.get::<Array>("Kids")?
        .iter::<Dict>()
        .find_map(|child| name_tree_destination(&child, target, page_index))
}

/// The page index an explicit destination array (`[pageRef /XYZ …]`) points to —
/// its first element is an indirect reference to the page object.
fn dest_array_page(dest: &Array, page_index: &HashMap<ObjRef, usize>) -> Option<usize> {
    match dest.raw_iter().next()? {
        MaybeRef::Ref(page_ref) => page_index.get(&page_ref).copied(),
        MaybeRef::NotRef(_) => None,
    }
}

/// Where a clickable PDF link points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A 0-based page index within this document.
    Page(usize),
    /// An external URI.
    Uri(String),
}

/// A clickable `/Link` annotation: its rectangle in normalized page coordinates
/// (0..1 of the crop box, top-left origin, matching the rendered image) and target.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfLink {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub target: LinkTarget,
}

/// Extract the clickable `/Link` annotations for every page, indexed by page;
/// pages with none get an empty vec. Rotated pages are skipped for now (their
/// annotation rectangles would need rotating to line up with the render).
pub fn page_links(doc: &Pdf) -> Vec<Vec<PdfLink>> {
    let page_index = build_page_index(doc);
    let mut out = Vec::with_capacity(doc.pages().len());
    for page in doc.pages().iter() {
        let mut links = Vec::new();
        let cb = page.crop_box();
        let (pw, ph) = (cb.width(), cb.height());
        if !matches!(page.rotation(), Rotation::None) || pw <= 0.0 || ph <= 0.0 {
            out.push(links);
            continue;
        }
        if let Some(annots) = page.raw().get::<Array>("Annots") {
            for annot in annots.iter::<Dict>() {
                if annot
                    .get::<Name>("Subtype")
                    .is_none_or(|n| n.as_str() != "Link")
                {
                    continue;
                }
                let (Some(target), Some(r)) =
                    (link_target(&annot, &page_index), annot.get::<Rect>("Rect"))
                else {
                    continue;
                };
                // `/Rect` is in PDF user space (bottom-left origin); normalize to the
                // crop box with a top-left origin so it overlays the rendered page.
                let (ax0, ax1) = (r.x0.min(r.x1), r.x0.max(r.x1));
                let (ay0, ay1) = (r.y0.min(r.y1), r.y0.max(r.y1));
                links.push(PdfLink {
                    x: (((ax0 - cb.x0) / pw) as f32).clamp(0.0, 1.0),
                    y: (((cb.y1 - ay1) / ph) as f32).clamp(0.0, 1.0),
                    w: (((ax1 - ax0) / pw) as f32).clamp(0.0, 1.0),
                    h: (((ay1 - ay0) / ph) as f32).clamp(0.0, 1.0),
                    target,
                });
            }
        }
        out.push(links);
    }
    out
}

/// Whether a `/URI` action may reach the OS URL opener (`NSWorkspace openURL:`
/// / `ShellExecute`). A PDF is entirely attacker-authored — including the
/// clickable overlay's position and size — so this is an allowlist: only
/// `http://` and `https://`, schemes compared case-insensitively per RFC 3986.
/// Everything else is rejected (`file:` launches local content, `smb:` leaks
/// NTLM hashes on Windows, `javascript:`/`data:`/app-registered schemes like
/// `ms-msdt:` run handlers). Whitespace and control characters anywhere are a
/// rejection, not something to trim: openers strip them, so ` javascript:…`
/// would otherwise walk past the prefix check.
///
/// Deliberately duplicated from `zorite_markdown::syntax::is_safe_external_url`
/// — five lines is cheaper than the cross-crate dependency the crates rule
/// forbids. Keep the two in step.
fn is_safe_external_uri(uri: &str) -> bool {
    !uri.chars().any(|c| c.is_whitespace() || c.is_control())
        && ["http://", "https://"].iter().any(|scheme| {
            uri.as_bytes()
                .get(..scheme.len())
                .is_some_and(|got| got.eq_ignore_ascii_case(scheme.as_bytes()))
        })
}

/// Resolve a `/Link` annotation's target: `/Dest`, or an `/A` action (`/URI` for
/// external links, `/GoTo` for internal jumps).
fn link_target(annot: &Dict, page_index: &HashMap<ObjRef, usize>) -> Option<LinkTarget> {
    if let Some(dest) = annot.get::<Array>("Dest") {
        return dest_array_page(&dest, page_index).map(LinkTarget::Page);
    }
    let action = annot.get::<Dict>("A")?;
    match action.get::<Name>("S").as_ref().map(|n| n.as_str()) {
        Some("URI") => {
            let uri = decode_pdf_string(action.get::<PdfString>("URI")?.as_bytes());
            if !is_safe_external_uri(&uri) {
                // Dropped here rather than at the click: no annotation, no
                // overlay, so a hostile URI never even reads as clickable.
                log::warn!("pdf: dropped link annotation with unsupported URI scheme");
                return None;
            }
            Some(LinkTarget::Uri(uri))
        }
        Some("GoTo") => {
            dest_array_page(&action.get::<Array>("D")?, page_index).map(LinkTarget::Page)
        }
        _ => None,
    }
}

/// Map every page's object reference to its 0-based index by walking the page
/// tree, so destinations (which reference a page by object ref) can be resolved.
fn build_page_index(doc: &Pdf) -> HashMap<ObjRef, usize> {
    let xref = doc.xref();
    let mut map = HashMap::new();
    let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
        return map;
    };
    let Some(root) = catalog.get_ref("Pages") else {
        return map;
    };
    let mut idx = 0;
    let mut visited = HashSet::new();
    walk_pages(doc, root, 0, &mut idx, &mut visited, &mut map);
    map
}

fn walk_pages(
    doc: &Pdf,
    r: ObjRef,
    depth: usize,
    idx: &mut usize,
    visited: &mut HashSet<ObjRef>,
    map: &mut HashMap<ObjRef, usize>,
) {
    if depth > MAX_DEPTH || !visited.insert(r) {
        return;
    }
    let xref = doc.xref();
    let Some(dict) = xref.get::<Dict>(r.into()) else {
        return;
    };
    // An internal node has `/Kids`; a leaf is a `/Page`.
    if let Some(kids) = dict.get::<Array>("Kids") {
        for kid in kids.raw_iter() {
            if let MaybeRef::Ref(kr) = kid {
                walk_pages(doc, kr, depth + 1, idx, visited, map);
            }
        }
    } else {
        map.insert(r, *idx);
        *idx += 1;
    }
}

/// Decode a PDF text string: UTF-16BE when it carries the BOM, otherwise treated
/// as Latin-1 (a close-enough stand-in for PDFDocEncoding for titles).
fn decode_pdf_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units).trim().to_string()
    } else {
        bytes
            .iter()
            .map(|&b| b as char)
            .collect::<String>()
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16be_bom_decodes() {
        // "Hi" in UTF-16BE with BOM.
        let b = [0xFE, 0xFF, 0x00, b'H', 0x00, b'i'];
        assert_eq!(decode_pdf_string(&b), "Hi");
    }

    #[test]
    fn latin1_decodes_and_trims() {
        assert_eq!(decode_pdf_string(b"  Intro  "), "Intro");
        assert_eq!(decode_pdf_string(&[b'C', 0xE9]), "Cé"); // 0xE9 = é in Latin-1
    }

    #[test]
    fn only_http_uris_survive() {
        assert!(is_safe_external_uri("https://example.com"));
        assert!(is_safe_external_uri("HTTP://EXAMPLE.COM"));
        for bad in [
            "file:///etc/passwd",
            "smb://evil/share",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "//evil.com",
            r"\\evil\share",
            " javascript:alert(1)",
            "java\tscript:x",
            "",
        ] {
            assert!(!is_safe_external_uri(bad), "should reject {bad:?}");
        }
    }
}
