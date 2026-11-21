//! Placement and storage contracts run without a GPU window or a live model gateway.
#![allow(dead_code)]
#[path = "../src/annotations.rs"] mod annotations;
#[path = "../src/i18n/mod.rs"] mod i18n;
#[path = "../src/model.rs"] mod model;
#[path = "../src/theme.rs"] mod theme;
#[path = "../src/files.rs"] mod files;
#[path = "../src/storage.rs"] mod storage;

use annotations::{Params, prepare_markdown, prepare_pdf};
use gpui_kit::base::text::MarkdownExtensions;

fn request(quote: &str) -> Params {
    Params { path:"notes.md".into(), quote:Some(quote.into()), comment:Some("解释这一处".into()), page:None, occurrence:None }
}

#[test]
fn annotation_markdown_requires_disambiguation_and_preserves_other_formatting() {
    let source = "# 标题\n\n第一次 target 和 **加粗**。\n\n第二次 target 和 [链接](https://example.com)。\n";
    let mut params = request("target");
    let error = prepare_markdown(source, &params, MarkdownExtensions::default()).err().unwrap();
    assert_eq!(error.0["error"]["code"], "ambiguous_match");
    assert_eq!(error.0["candidates"].as_array().unwrap().len(), 2);
    params.occurrence = Some(2);
    let prepared = prepare_markdown(source, &params, MarkdownExtensions::default()).unwrap();
    let marked = prepared.markdown.unwrap();
    assert!(marked.contains("第一次 target 和 **加粗**。"));
    assert!(marked.contains("<mark"));
    assert!(marked.contains("[链接](https://example.com)"));
    assert_eq!(prepared.item.occurrence, 1);
}

#[test]
fn annotation_rejects_cross_block_and_code_targets() {
    let error = prepare_markdown("第一段\n\n第二段", &request("第一段\n第二段"), MarkdownExtensions::default()).err().unwrap();
    assert_eq!(error.0["error"]["code"], "quote_not_found");
    let code = prepare_markdown("```text\nonly in code\n```", &request("only in code"), MarkdownExtensions::default()).err().unwrap();
    assert_eq!(code.0["error"]["code"], "unsupported_target");
}

#[test]
fn annotation_store_merges_files_and_preserves_existing_comments() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    let prepared = prepare_markdown("target", &request("target"), MarkdownExtensions::default()).unwrap();
    let mut manual = prepared.item.clone();
    manual.id = 7;
    manual.comment = "人工评论".into();
    storage::save_annotations(&first, &[manual.clone()]).unwrap();
    storage::save_annotations(&second, &[prepared.item.clone()]).unwrap();
    let loaded = storage::try_load_annotations(&first).unwrap();
    assert_eq!(loaded[0].comment, "人工评论");
    assert_eq!(storage::try_load_annotations(&second).unwrap().len(), 1);
    assert_eq!(annotations::existing(&loaded, &prepared, &first).unwrap_err().0["error"]["code"], "already_annotated");
    let same = annotations::Prepared { item:manual, markdown:prepared.markdown };
    assert_eq!(annotations::existing(&loaded, &same, &first).unwrap().unwrap()["created"], false);
}

#[test]
fn annotation_relocation_preserves_other_files_in_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("old.md");
    let target = dir.path().join("new.md");
    let sibling = dir.path().join("sibling.md");
    let note = prepare_markdown("target", &request("target"), MarkdownExtensions::default()).unwrap().item;
    storage::save_annotations(&source, std::slice::from_ref(&note)).unwrap();
    storage::save_annotations(&sibling, std::slice::from_ref(&note)).unwrap();

    storage::relocate_annotations(&source, &target).unwrap();

    assert!(storage::try_load_annotations(&source).unwrap().is_empty());
    assert_eq!(storage::try_load_annotations(&target).unwrap()[0].comment, note.comment);
    assert_eq!(storage::try_load_annotations(&sibling).unwrap().len(), 1);
}

#[test]
fn annotation_damaged_store_is_reported_without_overwriting_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.md");
    let store = dir.path().join(".folio/annotations.json");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    std::fs::write(&store, "damaged JSON").unwrap();
    assert!(annotations::list(&file).is_err());
    assert!(storage::save_annotations(&file, &[]).is_err());
    assert_eq!(std::fs::read_to_string(store).unwrap(), "damaged JSON");
}

#[test]
fn annotation_pdf_uses_actual_page_text_and_rejects_scans() {
    let bytes = include_bytes!("fixtures/annotations.pdf");
    let mut params = request("TARGET phrase");
    params.page = Some(1);
    let ambiguous = prepare_pdf(bytes.to_vec(), &params).err().unwrap();
    assert_eq!(ambiguous.0["error"]["code"], "ambiguous_match");
    params.occurrence = Some(2);
    let placed = prepare_pdf(bytes.to_vec(), &params).unwrap();
    assert_eq!(placed.item.page, 0);
    assert_eq!(placed.item.occurrence, 1);
    assert!(!placed.item.rects.is_empty());
    assert!(placed.item.rects[0].y > 0.3 && placed.item.rects[0].y < 0.5);
    params.page = Some(2);
    assert_eq!(prepare_pdf(bytes.to_vec(), &params).err().unwrap().0["error"]["code"], "no_text_layer");
    params.page = Some(3);
    assert_eq!(prepare_pdf(bytes.to_vec(), &params).err().unwrap().0["error"]["code"], "invalid_page");
}

#[test]
fn annotation_paths_cannot_escape_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let inside = project.join("inside.md");
    let outside = dir.path().join("outside.md");
    std::fs::write(&inside, "inside").unwrap();
    std::fs::write(&outside, "outside").unwrap();
    assert_eq!(annotations::resolve_path(&project, "inside.md").unwrap(), dunce::canonicalize(inside).unwrap());
    assert_eq!(annotations::resolve_path(&project, outside.to_str().unwrap()).unwrap_err().0["error"]["code"], "outside_project");
    assert!(annotations::resolve_path(&project, "../outside.md").is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside");
}
