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

#[test]
fn display_math_stays_nested_inside_callouts_and_round_trips() {
    use gpui_kit::base::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin};
    use markdown::mdast::Node;

    struct TestMathPlugin {
        display: bool,
    }
    impl MarkdownPlugin for TestMathPlugin {
        fn is_block(&self) -> bool {
            self.display
        }
        fn name(&self) -> &str {
            if self.display { "test-display-math" } else { "test-inline-math" }
        }
        fn parse(&self, node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
            let source = match node {
                Node::Math(math) if self.display => format!("$$\n{}\n$$", math.value),
                Node::InlineMath(_) if !self.display => cx.node_source(node)?.to_owned(),
                _ => return None,
            };
            Some(
                MarkdownNode::new(self.name(), ())
                    .text(source.clone())
                    .markdown(source),
            )
        }
    }

    let extensions = MarkdownExtensions::default()
        .plugin(TestMathPlugin { display: false })
        .plugin(TestMathPlugin { display: true })
        .block_surface("note", |_, _| gpui_kit::div());

    let multiline = "> [!NOTE]+ 贯穿全节的量：装填因子 $\\alpha$\n> 装填因子度量散列表\"被填得多满\"：\n> $$\n> \\alpha = \\frac{n}{s \\cdot m}\n> $$\n> $n$ 是已存表项数。";
    let m = BlockEditorModel::from_markdown(multiline, extensions.clone()).unwrap();
    assert_eq!(
        m.source(),
        "> [!NOTE]+ 贯穿全节的量：装填因子 $\\alpha$\n> 装填因子度量散列表\"被填得多满\"：\n>\n> $$\n> \\alpha = \\frac{n}{s \\cdot m}\n> $$\n>\n> $n$ 是已存表项数。"
    );
    assert_eq!(m.document().root().children().count(), 1);

    let single_line = "> [!NOTE]\n> 公式：\n> $$\\alpha = \\frac{n}{s \\cdot m}$$\n> 后续段落。";
    let m2 = BlockEditorModel::from_markdown(single_line, extensions.clone()).unwrap();
    assert_eq!(
        m2.source(),
        "> [!NOTE]\n> 公式：\n>\n> $$\n> \\alpha = \\frac{n}{s \\cdot m}\n> $$\n>\n> 后续段落。"
    );
    assert_eq!(m2.document().root().children().count(), 1);

    let inline_split = "> [!NOTE]+ 标题\n> 公式 $$a = b$$ 后续。";
    let m3 = BlockEditorModel::from_markdown(inline_split, extensions.clone()).unwrap();
    assert_eq!(
        m3.source(),
        "> [!NOTE]+ 标题\n> 公式\n>\n> $$\n> a = b\n> $$\n>\n> 后续。"
    );
    assert_eq!(m3.document().root().children().count(), 1);

    // Interactive insertion/paste of display math inside a callout stays inside the callout.
    let mut interactive = BlockEditorModel::from_markdown("> [!NOTE]+\n> 引导语", extensions).unwrap();
    interactive.select_end().unwrap();
    interactive.split_block().unwrap();
    assert!(interactive.paste_blocks("$$\nx = y\n$$").unwrap());
    assert_eq!(interactive.document().root().children().count(), 1);
    assert_eq!(
        interactive.source(),
        "> [!NOTE]+\n> 引导语\n>\n> $$\n> x = y\n> $$\n>"
    );

    // Direct normalize_math edge cases (AI chat and parser share this path).
    let nested = "> > \\[x = 1\\]\n> > tail";
    assert_eq!(
        markdown_editor::normalize_math(nested),
        "> > $$\n> > x = 1\n> > $$\n> > tail"
    );
    let gt_inside = "> $$\n> x > 0\n> > 1\n> $$";
    assert_eq!(markdown_editor::normalize_math(gt_inside), gt_inside);
    let empty_quoted = "> $$\n> $$";
    assert_eq!(markdown_editor::normalize_math(empty_quoted), empty_quoted);
    let list_indented = "- item\n  $$a + b$$";
    assert_eq!(
        markdown_editor::normalize_math(list_indented),
        "- item\n  $$\n  a + b\n  $$"
    );
}
