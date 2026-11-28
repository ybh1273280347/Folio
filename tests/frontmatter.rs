use gpui_kit::base::text::MarkdownExtensions;
use markdown_editor::{BlockEditorModel, BlockPosition, BlockSelection, EditOrigin, EditorCommand};

#[test]
fn frontmatter_bytes_survive_visual_body_edit() {
    let header = "\u{feff}--- \r\nname: 文档\r\n\r\n... \t\r\n \r\n\r\n";
    let source = format!("{header}正文");
    let mut model = BlockEditorModel::from_markdown(&source, MarkdownExtensions::default()).unwrap();
    let body = model.document().leaves().last().unwrap().id();
    model.set_selection(BlockSelection::caret(BlockPosition::new(body, "正文".len()))).unwrap();
    model.insert_text("继续", EditOrigin::Command("insert_text")).unwrap();
    assert_eq!(model.source(), format!("{header}正文继续"));
}

#[test]
fn frontmatter_only_document_preserves_eof_and_empty_line() {
    let source = "---\r\n\r\n--- ";
    let model = BlockEditorModel::from_markdown(source, MarkdownExtensions::default()).unwrap();
    assert_eq!(model.source(), source);
}

#[test]
fn frontmatter_stays_first_when_moving_body_blocks() {
    let source = "---\ntitle: Note\n---\n\nFirst\n\nSecond";
    let mut model = BlockEditorModel::from_markdown(source, MarkdownExtensions::default()).unwrap();
    let first_body = model.document().leaves().nth(1).unwrap().id();
    model.set_selection(BlockSelection::caret(BlockPosition::new(first_body, 0))).unwrap();
    assert!(!model.run(EditorCommand::MoveBlocks { down: false }).unwrap());
    assert_eq!(model.source(), source);

    assert!(model.run(EditorCommand::MoveBlocks { down: true }).unwrap());
    assert_eq!(model.source(), "---\ntitle: Note\n---\n\nSecond\n\nFirst");
}
