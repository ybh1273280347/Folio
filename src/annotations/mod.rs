//! Annotation placement and records shared by the editor, PDF viewer, and AI tools.

mod sidecar;

pub use sidecar::{load_annotations, relocate_annotations, save_annotations, try_load_annotations};

use crate::files;
use gpui_kit::base::text::MarkdownExtensions;
use markdown_editor::{BlockEditorModel, BlockKind, BlockPosition, BlockSelection, EditorCommand, MarkChange, MarkKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io, path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationItem {
    pub id: u64,
    pub file: String,
    pub page: usize,
    pub quote: String,
    pub occurrence: usize,
    pub color: String,
    pub comment: String,
    #[serde(default)]
    pub region: Option<AnnotationRect>,
    #[serde(default)]
    pub rects: Vec<AnnotationRect>,
}

/// 5 fluorescent highlight colors (name, hex string for Markdown `<mark>`, RGB hex for UI).
pub const HIGHLIGHT_COLORS: &[(&str, &str, u32)] = &[
    ("yellow", "#fef08a", 0xfacc15),
    ("green", "#bbf7d0", 0x4ade80),
    ("blue", "#bae6fd", 0x38bdf8),
    ("pink", "#fbcfe8", 0xf472b6),
    ("orange", "#fed7aa", 0xfb923c),
];

/// RGB hex for Markdown `<mark>` highlight text background (matches the light `#fef08a` default).
pub fn highlight_mark_rgb(name: &str) -> u32 {
    match name {
        "green" => 0xbbf7d0,
        "blue" => 0xbae6fd,
        "pink" => 0xfbcfe8,
        "orange" => 0xfed7aa,
        _ => 0xfef08a,
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    pub path: String,
    pub quote: Option<String>,
    pub comment: Option<String>,
    pub page: Option<usize>,
    pub occurrence: Option<usize>,
}

#[derive(Debug)]
pub struct Failure(pub Value);
impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self(json!({"status":"error", "error":{"code":code, "message":message.into()}}))
    }
}
impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        let code = match error.kind() {
            io::ErrorKind::NotFound => "file_not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            _ => "io_error",
        };
        Self::new(code, error.to_string())
    }
}

pub struct Prepared {
    pub item: AnnotationItem,
    /// Complete source produced by the editor's normal serializer, never an HTML string patch.
    pub markdown: Option<String>,
}

/// Canonical paths enforce the same project boundary for all annotation operations.
pub fn resolve_path(root: &Path, path: &str) -> Result<std::path::PathBuf, Failure> {
    let root = dunce::canonicalize(root)?;
    let target = if Path::new(path).is_absolute() {
        dunce::canonicalize(path)?
    } else { files::project_path(&root, path)? };
    if !target.starts_with(&root) {
        return Err(Failure::new("outside_project", crate::i18n::tags::ERR_ANN_OUTSIDE_PROJECT.get()));
    }
    if !target.is_file() || !target.extension().is_some_and(|e|
        e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("pdf")) {
        return Err(Failure::new("unsupported_format", crate::i18n::tags::ERR_ANN_UNSUPPORTED_FORMAT.get()));
    }
    Ok(target)
}

fn choose_match(params: &Params, candidates: Vec<Value>) -> Result<usize, Failure> {
    if candidates.is_empty() {
        return Err(Failure::new("quote_not_found", crate::i18n::tags::ERR_ANN_QUOTE_NOT_FOUND.get()));
    }
    if params.occurrence.is_none() && candidates.len() > 1 {
        let mut error = Failure::new("ambiguous_match", crate::i18n::tags::ERR_ANN_AMBIGUOUS_MATCH.get());
        error.0["candidates"] = json!(candidates);
        return Err(error);
    }
    let number = params.occurrence.unwrap_or(1);
    if number == 0 || number > candidates.len() {
        return Err(Failure::new("invalid_occurrence", crate::i18n::tags::err_ann_invalid_occurrence(candidates.len())));
    }
    Ok(number - 1)
}

pub fn validate_create(params: &Params) -> Result<(&str, &str), Failure> {
    let quote = params.quote.as_deref().filter(|q| !q.trim().is_empty())
        .ok_or_else(|| Failure::new("invalid_request", crate::i18n::tags::ERR_ANN_EMPTY_QUOTE.get()))?;
    let comment = params.comment.as_deref().filter(|q| !q.trim().is_empty())
        .ok_or_else(|| Failure::new("invalid_request", crate::i18n::tags::ERR_ANN_EMPTY_COMMENT.get()))?;
    Ok((quote, comment))
}

pub fn prepare_markdown(source: &str, params: &Params, extensions: MarkdownExtensions) -> Result<Prepared, Failure> {
    let (quote, comment) = validate_create(params)?;
    if params.page.is_some() {
        return Err(Failure::new("invalid_request", crate::i18n::tags::ERR_ANN_MD_NO_PAGE.get()));
    }
    let mut model = BlockEditorModel::from_markdown(source, extensions)
        .map_err(|e| Failure::new("parse_error", e.to_string()))?;
    let mut matches = Vec::new();
    for leaf in model.document().leaves() {
        let Some(text) = leaf.text() else { continue; };
        for (start, _) in text.match_indices(quote) {
            let prefix = text[..start].chars().count();
            let preview: String = text.chars().skip(prefix.saturating_sub(35)).take(120).collect();
            matches.push((leaf.id(), start, leaf.kind(), preview));
        }
    }
    let index = choose_match(params, matches.iter().enumerate()
        .map(|(i, (_, _, _, preview))| json!({"occurrence":i+1, "preview":preview})).collect())?;
    let (block, start, kind, _) = &matches[index];
    if !matches!(kind, BlockKind::Paragraph | BlockKind::Heading { .. } | BlockKind::TableCell) || quote.contains('\u{fffc}') {
        return Err(Failure::new("unsupported_target", crate::i18n::tags::ERR_ANN_UNSUPPORTED_TARGET.get()));
    }
    model.set_selection(BlockSelection::Text {
        anchor: BlockPosition::new(*block, *start), head: BlockPosition::new(*block, start + quote.len()),
    }).map_err(|e| Failure::new("placement_error", format!("{e:?}")))?;
    model.run(EditorCommand::ApplyMark(MarkChange::new().with_highlight(gpui_kit::rgb(highlight_mark_rgb("yellow")).into())))
        .map_err(|e| Failure::new("placement_error", format!("{e:?}")))?;
    Ok(Prepared { item: AnnotationItem { id: 0, file: String::new(), page: 0,
        quote: quote.into(), occurrence: index, color: "yellow".into(), comment: comment.into(), region: None, rects: vec![] },
        markdown: Some(model.source()) })
}

pub fn prepare_pdf(bytes: Vec<u8>, params: &Params) -> Result<Prepared, Failure> {
    let (quote, comment) = validate_create(params)?;
    let page = params.page.filter(|p| *p > 0)
        .ok_or_else(|| Failure::new("invalid_page", crate::i18n::tags::ERR_ANN_PDF_PAGE_REQUIRED.get()))?;
    let document = gpui_pdf::parse(Arc::new(bytes)).map_err(|e| Failure::new("parse_error", crate::i18n::tags::err_ann_pdf_parse(&e)))?;
    if page > document.pages().len() {
        return Err(Failure::new("invalid_page", crate::i18n::tags::err_ann_pdf_page_count(document.pages().len())));
    }
    let text = gpui_pdf::extract_page_text(&document, page - 1)
        .filter(|text| !text.is_empty()).ok_or_else(|| Failure::new("no_text_layer", crate::i18n::tags::ERR_ANN_PDF_NO_TEXT_LAYER.get()))?;
    let matches = text.find_matches(quote);
    let index = choose_match(params, matches.iter().enumerate().map(|(i, rects)|
        json!({"page":page, "occurrence":i+1, "vertical_position":rects.first().map(|r| r.y)})).collect())?;
    let rects = matches[index].iter().map(|r| AnnotationRect { x:r.x, y:r.y, w:r.w, h:r.h }).collect();
    Ok(Prepared { item: AnnotationItem { id:0, file:String::new(), page:page-1, quote:quote.into(),
        occurrence:index, color:"yellow".into(), comment:comment.into(), region:None, rects }, markdown:None })
}

pub fn item_json(item: &AnnotationItem, path: &Path) -> Value {
    json!({"id":item.id, "path":path, "quote":item.quote, "comment":item.comment, "color":item.color,
        "occurrence":item.occurrence+1,
        "page": if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) { Some(item.page+1) } else { None }})
}

/// Remove only the inline highlight at the saved quote, keeping text and other marks.
pub fn remove_markdown_highlight(
    source: &str, item: &AnnotationItem, extensions: MarkdownExtensions,
) -> Result<Option<String>, Failure> {
    let mut model = BlockEditorModel::from_markdown(source, extensions)
        .map_err(|e| Failure::new("parse_error", e.to_string()))?;
    let selection = model.quote_selection(&item.quote, item.occurrence, false)
        .ok_or_else(|| Failure::new("quote_not_found", crate::i18n::tags::ERR_ANN_QUOTE_NOT_FOUND.get()))?;
    model.set_selection(selection)
        .map_err(|e| Failure::new("placement_error", format!("{e:?}")))?;
    let changed = model.run(EditorCommand::ApplyMark(MarkChange::new().clear(MarkKind::Highlight)))
        .map_err(|e| Failure::new("placement_error", format!("{e:?}")))?;
    Ok(changed.then(|| model.source()))
}

/// Runs on the UI thread: callers merge a fresh store and refresh all panes before the next request commits.
pub fn existing(items: &[AnnotationItem], prepared: &Prepared, path: &Path) -> Result<Option<Value>, Failure> {
    let pdf = prepared.markdown.is_none();
    let normalize = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    if let Some(item) = items.iter().find(|item| item.page == prepared.item.page
        && item.occurrence == prepared.item.occurrence
        && if pdf { normalize(&item.quote) == normalize(&prepared.item.quote) } else { item.quote == prepared.item.quote })
    {
        if item.comment == prepared.item.comment {
            return Ok(Some(json!({"status":"ok", "created":false, "annotation":item_json(item, path)})));
        }
        let mut error = Failure::new("already_annotated", crate::i18n::tags::ERR_ANN_ALREADY_ANNOTATED.get());
        error.0["annotation"] = item_json(item, path);
        return Err(error);
    }
    Ok(None)
}

pub fn list(path: &Path) -> Result<Value, Failure> {
    let items = try_load_annotations(path)?;
    Ok(json!({"status":"ok", "annotations":items.iter().map(|a| item_json(a, path)).collect::<Vec<_>>()}))
}
