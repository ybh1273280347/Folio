//! Tests for `<details>` and foldable `[!xxx]-` / `[!xxx]+` callouts.
use gpui_kit::base::text::MarkdownExtensions;
use markdown_editor::{BlockEditorModel, EditorCommand, EditOrigin};

fn model(source: &str) -> BlockEditorModel {
    BlockEditorModel::from_markdown(source, MarkdownExtensions::default()).unwrap()
}

#[test]
fn foldable_callouts_round_trip_all_variants_and_edge_cases() {
    let cases = [
        "> [!NOTE]-\n> note body",
        "> [!TIP]+ Custom Title\n> tip body",
        "> [!WARNING]-\n> warning body",
        "> [!IMPORTANT]+ Head\n> important body",
        "> [!CAUTION]-\n> caution body",
        "> [!CUSTOM]- My Custom\n> custom body",
        "<details>\n<summary>详情</summary>\n\n正文\n\n</details>",
        "<details open>\n<summary>展开</summary>\n\n正文\n\n</details>",
    ];
    for source in cases {
        let m = model(source);
        assert_eq!(m.source(), source);
    }

    // CRLF and non-paragraph-looking first lines must import without panicking.
    let crlf = "> [!NOTE]- - item\r\n> body";
    let m = model(crlf);
    assert!(m.source().starts_with("> [!NOTE]-"));
}

#[test]
fn fold_alert_input_rules_unify_plus_and_minus_and_editing() {
    // Minus variant: stays on title leaf (collapsed), serializes as [!NOTE]-.
    let mut m = model("");
    m.run(EditorCommand::ToggleBlockquote).unwrap();
    m.insert_text("[!NOTE]-", EditOrigin::InputRule("typing")).unwrap();
    m.split_block().unwrap();
    assert_eq!(m.source(), "> [!NOTE]-");
    let leaves: Vec<_> = m.document().leaves().map(|l| l.id()).collect();
    assert_eq!(leaves.len(), 2);
    assert_eq!(m.caret().unwrap().block, leaves[0]);

    // Plus variant: enters body leaf (expanded), serializes as [!TIP]+.
    let mut p = model("");
    p.run(EditorCommand::ToggleBlockquote).unwrap();
    p.insert_text("[!TIP]+", EditOrigin::InputRule("typing")).unwrap();
    p.split_block().unwrap();
    let p_leaves: Vec<_> = p.document().leaves().map(|l| l.id()).collect();
    assert_eq!(p_leaves.len(), 2);
    assert_eq!(p.caret().unwrap().block, p_leaves[1]);
    p.insert_text("body text", EditOrigin::InputRule("typing")).unwrap();
    assert_eq!(p.source(), "> [!TIP]+\n> body text");

    // Backspace at offset 0 of an empty first body paragraph removes it and returns to title.
    let mut b = model("");
    b.run(EditorCommand::ToggleBlockquote).unwrap();
    b.insert_text("[!WARNING]+ Title", EditOrigin::InputRule("typing")).unwrap();
    b.split_block().unwrap();
    assert_eq!(b.document().leaves().count(), 2);
    b.delete(false).unwrap();
    let b_leaves: Vec<_> = b.document().leaves().map(|l| l.id()).collect();
    assert_eq!(b_leaves.len(), 1);
    assert_eq!(b.caret().unwrap().block, b_leaves[0]);
    assert_eq!(b.source(), "> [!WARNING]- Title".replace('-', "+"));

    // ToggleBlockquote on a Fold unwraps it cleanly rather than nesting a quote.
    let mut u = model("> [!NOTE]- Title\n> body");
    u.run(EditorCommand::ToggleBlockquote).unwrap();
    assert_eq!(u.source(), "Title\n\nbody");
}
