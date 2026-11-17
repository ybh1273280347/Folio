//! Model-facing annotation placement uses the same Markdown model and PDF text layer as the UI.
use crate::{files, model::{AnnotationItem, AnnotationRect}, storage};
use gpui_kit::base::text::MarkdownExtensions;
use markdown_editor::{BlockEditorModel, BlockKind, BlockPosition, BlockSelection, EditorCommand, MarkChange};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{io, path::Path, sync::Arc};

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

/// Canonical paths enforce the same project boundary for both read and create operations.
pub fn resolve_path(root: &Path, path: &str) -> Result<std::path::PathBuf, Failure> {
    let root = dunce::canonicalize(root)?;
    let target = if Path::new(path).is_absolute() {
        dunce::canonicalize(path)?
    } else { files::project_path(&root, path)? };
    if !target.starts_with(&root) {
        return Err(Failure::new("outside_project", "批注工具只能访问当前项目内的文件。"));
    }
    if !target.is_file() || !target.extension().is_some_and(|e|
        e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("pdf")) {
        return Err(Failure::new("unsupported_format", "首版只支持 Markdown 和有文字层的 PDF 文件。"));
    }
    Ok(target)
}

fn choose_match(params: &Params, candidates: Vec<Value>) -> Result<usize, Failure> {
    if candidates.is_empty() {
        return Err(Failure::new("quote_not_found", "未找到原文。请使用单个文字块中的实际原文；不支持跨块、公式或图片定位。"));
    }
    if params.occurrence.is_none() && candidates.len() > 1 {
        let mut error = Failure::new("ambiguous_match", "原文匹配多处，请增加原文上下文或指定 occurrence。");
        error.0["candidates"] = json!(candidates);
        return Err(error);
    }
    let number = params.occurrence.unwrap_or(1);
    if number == 0 || number > candidates.len() {
        return Err(Failure::new("invalid_occurrence", format!("occurrence 应在 1 到 {} 之间。", candidates.len())));
    }
    Ok(number - 1)
}

pub fn validate_create(params: &Params) -> Result<(&str, &str), Failure> {
    let quote = params.quote.as_deref().filter(|q| !q.trim().is_empty())
        .ok_or_else(|| Failure::new("invalid_request", "quote 必须是非空原文。"))?;
    let comment = params.comment.as_deref().filter(|q| !q.trim().is_empty())
        .ok_or_else(|| Failure::new("invalid_request", "comment 必须是非空批注正文。"))?;
    Ok((quote, comment))
}

pub fn prepare_markdown(source: &str, params: &Params, extensions: MarkdownExtensions) -> Result<Prepared, Failure> {
    let (quote, comment) = validate_create(params)?;
    if params.page.is_some() {
        return Err(Failure::new("invalid_request", "Markdown 不使用 page 参数。"));
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
        return Err(Failure::new("unsupported_target", "此处不是可高亮的普通文字块，请选择段落、标题或表格单元格中的原文。"));
    }
    model.set_selection(BlockSelection::Text {
        anchor: BlockPosition::new(*block, *start), head: BlockPosition::new(*block, start + quote.len()),
    }).map_err(|e| Failure::new("placement_error", format!("{e:?}")))?;
    model.run(EditorCommand::ApplyMark(MarkChange::new().with_highlight(gpui_kit::rgb(0xfacc15).into())))
        .map_err(|e| Failure::new("placement_error", format!("{e:?}")))?;
    Ok(Prepared { item: AnnotationItem { id: 0, file: String::new(), page: 0,
        quote: quote.into(), occurrence: index, color: "yellow".into(), comment: comment.into(), region: None, rects: vec![] },
        markdown: Some(model.source()) })
}

pub fn prepare_pdf(bytes: Vec<u8>, params: &Params) -> Result<Prepared, Failure> {
    let (quote, comment) = validate_create(params)?;
    let page = params.page.filter(|p| *p > 0)
        .ok_or_else(|| Failure::new("invalid_page", "PDF 必须指定从 1 开始的实际文件页码。"))?;
    let document = gpui_pdf::parse(Arc::new(bytes)).map_err(|e| Failure::new("parse_error", format!("无法解析 PDF：{e:?}")))?;
    if page > document.pages().len() {
        return Err(Failure::new("invalid_page", format!("PDF 只有 {} 页。", document.pages().len())));
    }
    let text = gpui_pdf::extract_page_text(&document, page - 1)
        .filter(|text| !text.is_empty()).ok_or_else(|| Failure::new("no_text_layer", "该页没有可用原生文字层，首版不支持扫描页或 OCR 坐标定位。"))?;
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
        let mut error = Failure::new("already_annotated", "此位置已有不同的评论，未覆盖原批注。");
        error.0["annotation"] = item_json(item, path);
        return Err(error);
    }
    Ok(None)
}

pub fn list(path: &Path) -> Result<Value, Failure> {
    let items = storage::try_load_annotations(path)?;
    Ok(json!({"status":"ok", "annotations":items.iter().map(|a| item_json(a, path)).collect::<Vec<_>>()}))
}
