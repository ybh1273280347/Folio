//! `markdown::parse` output (`BlockNode`) into the editor model.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::SharedString;

use super::{
    document::{AtomData, BlockContent, EditorDocument},
    inline::{InlineAtom, InlineText},
    kind::{BlockKindData, FoldSyntax},
    step::DetachedBlock,
    transaction::TransactionBuilder,
};
use gpui_kit::base::text::editor_host::parse_markdown;
use gpui_kit::base::text::{
    MarkdownExtensions,
    editor_host::{BlockNode, InlineNode, LinkMark, Paragraph, TextMark},
};

pub(crate) fn parse_document(
    source: &str,
    extensions: Arc<MarkdownExtensions>,
) -> Result<EditorDocument, SharedString> {
    let normalized = super::math::normalize_math(source);
    let source = normalized.as_str();
    let parsed = parse_markdown(source, extensions.clone())?;
    let mut doc = EditorDocument::empty(extensions.clone());
    let root = doc.root;
    // Drop the placeholder paragraph of the empty document.
    doc.detach(root, 0);
    let blocks = import_markdown_blocks(source, extensions.clone())?;
    for (ix, block) in blocks.into_iter().enumerate() {
        doc.attach(root, ix, block);
    }
    // Same structural rules as after every transaction (empty list items
    // get a paragraph, an empty document gets one leaf). The steps of this
    // pass are not history.
    let mut tx = TransactionBuilder::new(&mut doc);
    tx.normalize()
        .map_err(|e| SharedString::from(format!("{e:?}")))?;
    drop(tx);
    doc.structure_dirty = true;
    doc.refresh_order();
    // Markdown parsers omit final blank paragraphs. Reconstruct that editable
    // boundary as real document nodes, never a mutable raw-source shadow.
    // Primary AST spans distinguish exterior footer trivia from line endings
    // owned by an unterminated code fence/body.
    let exterior_start = parsed
        .last()
        .and_then(BlockNode::span)
        .map_or(0, |span| span.end);
    let exterior = source.get(exterior_start..).unwrap_or_default();
    let trimmed_spaces = exterior.trim_end_matches([' ', '\t']);
    let stripped = trimmed_spaces.trim_end_matches(['\r', '\n']);
    let line_ends = &trimmed_spaces[stripped.len()..];
    let ending_count = if line_ends.contains('\n') {
        line_ends.bytes().filter(|b| *b == b'\n').count()
    } else {
        line_ends.len()
    };
    for _ in 0..ending_count.div_ceil(2) {
        let index = doc.children(root).len();
        doc.attach(
            root,
            index,
            DetachedBlock::leaf(
                BlockKindData::Paragraph,
                BlockContent::Inline(InlineText::default()),
            ),
        );
    }
    doc.refresh_order();

    // CommonMark ignores terminal paragraph spaces, but in a live editing
    // buffer they separate the next typed/dictated word. Restore that trivia
    // into the document, not a second mutable source buffer. Code retains its
    // exact own body and must not inherit trivia after its closing fence.
    let trimmed = source.trim_end_matches([' ', '\t']);
    let mut tail = &source[trimmed.len()..];
    let last = *doc.leaves().last().unwrap();
    if (matches!(doc.kind(last), Some(BlockKindData::Heading { .. }))
        || doc.index_in_parent(last).is_some_and(|(parent, _)| {
            matches!(doc.kind(parent), Some(BlockKindData::ListItem { .. }))
        }))
        && doc.inline(last).is_some_and(|inline| inline.is_empty())
    {
        // One space is the empty heading's structural marker separator.
        tail = tail.strip_prefix(' ').unwrap_or(tail);
    }
    if !tail.is_empty()
        && let Some(entry) = doc.get_mut(last)
        && let BlockContent::Inline(inline) = &mut entry.content
    {
        inline.push(InlineText::plain(tail, TextMark::default()));
    }
    doc.revision = 0;
    Ok(doc)
}

#[derive(Clone, Copy)]
enum DetailsTag {
    Open(bool),
    Close,
    SummaryOpen,
    SummaryClose,
}

// Only HTML nodes can contain delimiters. In particular, escaped text,
// fenced code and inline code do not become HTML nodes in markdown-rs.
fn details_tags(source: &str) -> Result<Vec<(usize, usize, DetailsTag)>, SharedString> {
    use ::markdown::mdast::Node;

    fn visit(node: &Node, ranges: &mut Vec<(usize, usize)>) {
        match node {
            Node::Html(_) => {
                if let Some(pos) = node.position() {
                    ranges.push((pos.start.offset, pos.end.offset));
                }
            }
            Node::Root(_) | Node::Blockquote(_) | Node::List(_) | Node::ListItem(_) => {
                if let Some(children) = node.children() {
                    for child in children {
                        visit(child, ranges);
                    }
                }
            }
            _ => {}
        }
    }

    fn has_open_attribute(attrs: &str) -> bool {
        let bytes = attrs.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' { i += 1; }
            if start == i { i += 1; continue; }
            if bytes[start..i].eq_ignore_ascii_case(b"open") { return true; }
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            if i < bytes.len() && bytes[i] == b'=' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
                if i < bytes.len() && (bytes[i] == b'\'' || bytes[i] == b'"') {
                    let quote = bytes[i];
                    i += 1;
                    while i < bytes.len() && bytes[i] != quote { i += 1; }
                    if i < bytes.len() { i += 1; }
                } else {
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() { i += 1; }
                }
            }
        }
        false
    }

    // An HTML flow node can swallow a fence opener and stop at a blank line.
    // Track fences across node boundaries before looking at HTML node ranges.
    fn fenced_ranges(source: &str) -> Vec<std::ops::Range<usize>> {
        let mut ranges = Vec::new();
        let mut fence: Option<(u8, usize, usize)> = None;
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            let mut content = line.trim_start();
            while let Some(rest) = content.strip_prefix('>') {
                content = rest.trim_start();
            }
            if let Some(rest) = content.strip_prefix("- ").or_else(|| content.strip_prefix("* ")).or_else(|| content.strip_prefix("+ ")) {
                content = rest.trim_start();
            }
            let digits = content.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && content.as_bytes().get(digits).is_some_and(|byte| matches!(*byte, b'.' | b')'))
                && content.as_bytes().get(digits + 1).is_some_and(u8::is_ascii_whitespace)
            {
                content = content[digits + 1..].trim_start();
            }
            if let Some(ch @ (b'`' | b'~')) = content.bytes().next() {
                let count = content.bytes().take_while(|byte| *byte == ch).count();
                if count >= 3 {
                    if let Some((open, length, start)) = fence {
                        if open == ch && count >= length && content[count..].trim().is_empty() {
                            ranges.push(start..offset + line.len());
                            fence = None;
                        }
                    } else {
                        fence = Some((ch, count, offset));
                    }
                }
            }
            offset += line.len();
        }
        if let Some((_, _, start)) = fence { ranges.push(start..source.len()); }
        ranges
    }

    let mut options = ::markdown::ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    let ast = ::markdown::to_mdast(source, &options)
        .map_err(|error| SharedString::from(error.to_string()))?;
    let mut ranges = Vec::new();
    visit(&ast, &mut ranges);
    let mut tags = Vec::new();
    let fenced = fenced_ranges(source);
    for (start, end) in ranges {
        let bytes = source.as_bytes();
        let mut i = start;
        let mut fence: Option<(u8, usize)> = None;
        let mut inline_ticks = 0;
        let mut comment = false;
        while i < end {
            if i == start || bytes[i - 1] == b'\n' {
                let line_end = source[i..end].find('\n').map_or(end, |n| i + n);
                let mut line = source[i..line_end].trim_start();
                while let Some(rest) = line.strip_prefix('>') {
                    line = rest.trim_start();
                }
                let marker = line.as_bytes().first().copied();
                if matches!(marker, Some(b'`' | b'~')) {
                    let ch = marker.unwrap();
                    let count = line.bytes().take_while(|b| *b == ch).count();
                    if count >= 3 {
                        if let Some((open, length)) = fence {
                            if ch == open && count >= length && line[count..].trim().is_empty() {
                                fence = None;
                            }
                        } else {
                            fence = Some((ch, count));
                        }
                        i = line_end;
                        continue;
                    }
                }
            }
            if fence.is_some() {
                i += 1;
                continue;
            }
            if comment {
                if bytes[i..end].starts_with(b"-->") {
                    comment = false;
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            if bytes[i..end].starts_with(b"<!--") {
                comment = true;
                i += 4;
                continue;
            }
            if bytes[i] == b'`' {
                let count = bytes[i..end].iter().take_while(|b| **b == b'`').count();
                if inline_ticks == 0 { inline_ticks = count; }
                else if inline_ticks == count { inline_ticks = 0; }
                i += count;
                continue;
            }
            if bytes[i] != b'<' || inline_ticks != 0 {
                i += 1;
                continue;
            }
            if fenced.iter().any(|range| range.contains(&i)) {
                i += 1;
                continue;
            }
            let escaped = bytes[start..i].iter().rev().take_while(|b| **b == b'\\').count() % 2 == 1;
            if escaped {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            let closing = j < end && bytes[j] == b'/';
            if closing { j += 1; }
            let name_start = j;
            while j < end && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'-' | b':')) { j += 1; }
            if j == name_start { i += 1; continue; }
            let name = &source[name_start..j];
            let attrs_start = j;
            let mut quote = None;
            while j < end {
                let ch = bytes[j];
                if let Some(open) = quote {
                    if ch == open { quote = None; }
                } else if ch == b'\'' || ch == b'"' {
                    quote = Some(ch);
                } else if ch == b'>' {
                    break;
                }
                j += 1;
            }
            if j == end { break; }
            if !(name.eq_ignore_ascii_case("details") || name.eq_ignore_ascii_case("summary"))
                || (attrs_start < j && !bytes[attrs_start].is_ascii_whitespace())
            {
                i = j + 1;
                continue;
            }
            let tag = if name.eq_ignore_ascii_case("summary") {
                if closing { DetailsTag::SummaryClose } else { DetailsTag::SummaryOpen }
            } else if closing {
                DetailsTag::Close
            } else {
                DetailsTag::Open(has_open_attribute(&source[attrs_start..j]))
            };
            tags.push((i, j + 1, tag));
            i = j + 1;
        }
    }
    Ok(tags)
}

struct DetailsRange {
    start: usize,
    content: usize,
    end_content: usize,
    end: usize,
    open: bool,
    summary_start: Option<usize>,
    summary_end: Option<usize>,
    summary_tag_start: Option<usize>,
    summary_tag_end: Option<usize>,
}

fn without_container_prefix(source: &str, prefix: &str) -> String {
    let continuation: String = prefix.chars().map(|ch| if ch == '>' { '>' } else { ' ' }).collect();
    source.split('\n').enumerate().map(|(index, line)| {
        if index == 0 || continuation.is_empty() {
            line
        } else if line.trim_end() == continuation.trim_end() {
            ""
        } else {
            line.strip_prefix(&continuation).unwrap_or(line)
        }
    }).collect::<Vec<_>>().join("\n")
}

/// Import normalized source, including details whose HTML spans multiple AST nodes.
pub(crate) fn import_markdown_blocks(source: &str, extensions: Arc<MarkdownExtensions>) -> Result<Vec<DetachedBlock>, SharedString> {
    import_markdown_with_defs(source, extensions, &Defs::new())
}

fn import_markdown_with_defs(source: &str, extensions: Arc<MarkdownExtensions>, inherited: &Defs) -> Result<Vec<DetachedBlock>, SharedString> {
    let original = parse_markdown(source, extensions.clone())?;
    let mut defs = inherited.clone();
    collect_definitions(&original, &mut defs);
    let mut stack: Vec<DetailsRange> = Vec::new();
    let mut ranges = Vec::new();
    for (start, end, tag) in details_tags(source)? {
        match tag {
            DetailsTag::Open(open) => stack.push(DetailsRange {
                start, content: end, end_content: 0, end: 0, open,
                summary_start: None, summary_end: None,
                summary_tag_start: None, summary_tag_end: None,
            }),
            DetailsTag::SummaryOpen => {
                if let Some(detail) = stack.last_mut() && detail.summary_start.is_none() {
                    detail.summary_tag_start = Some(start);
                    detail.summary_start = Some(end);
                }
            }
            DetailsTag::SummaryClose => {
                if let Some(detail) = stack.last_mut() && detail.summary_start.is_some() && detail.summary_end.is_none() {
                    detail.summary_end = Some(start);
                    detail.summary_tag_end = Some(end);
                }
            }
            DetailsTag::Close => {
                if let Some(mut detail) = stack.pop() {
                    detail.end_content = start;
                    detail.end = end;
                    if stack.is_empty() { ranges.push(detail); }
                }
            }
        }
    }
    if ranges.is_empty() {
        let mut blocks = Vec::new();
        for node in &original {
            import_block_with_folds(node, &defs, &mut blocks, Some(source), None, Some(&extensions));
        }
        return Ok(blocks);
    }

    let mut transformed = String::new();
    let mut folds = HashMap::new();
    let mut cursor = 0;
    for detail in ranges {
        let line_start = source[..detail.start].rfind('\n').map_or(0, |index| index + 1);
        let prefix = &source[line_start..detail.start];
        let (title, body) = if let (Some(start), Some(end), Some(tag_start), Some(tag_end)) = (
            detail.summary_start, detail.summary_end, detail.summary_tag_start, detail.summary_tag_end,
        ) {
            let title = without_container_prefix(&source[start..end], prefix);
            let mut body = String::from(&source[detail.content..tag_start]);
            body.push_str(&source[tag_end..detail.end_content]);
            (title, body)
        } else {
            (String::new(), source[detail.content..detail.end_content].to_owned())
        };
        let title = title.trim();
        let parsed_title = parse_markdown(title, extensions.clone())?;
        let mut title_defs = defs.clone();
        collect_definitions(&parsed_title, &mut title_defs);
        let title = parsed_title.iter().find_map(|node| match node {
            BlockNode::Paragraph(paragraph) => Some(import_inline(paragraph, &title_defs)),
            _ => None,
        }).unwrap_or_else(|| InlineText::plain(title, TextMark::default()));
        let body = without_container_prefix(&body, prefix);
        let mut children = vec![DetachedBlock::paragraph(title)];
        if !body.trim().is_empty() {
            children.extend(import_markdown_with_defs(body.trim_matches(['\r', '\n']), extensions.clone(), &defs)?);
        }
        let block = DetachedBlock::container(BlockKindData::Fold {
            syntax: FoldSyntax::Details {
                opening: source[detail.start..detail.content].into(),
                summary_opening: match (detail.summary_tag_start, detail.summary_start) {
                    (Some(start), Some(end)) => source[start..end].into(),
                    _ => "<summary>".into(),
                },
            },
            initially_open: detail.open,
        }, children);
        let mut id = folds.len();
        let marker = loop {
            let marker = format!("<!--gpui-editor-fold-{id}-->");
            if !source.contains(&marker) && !folds.contains_key(&marker) { break marker; }
            id += 1;
        };
        transformed.push_str(&source[cursor..detail.start]);
        transformed.push_str(&marker);
        folds.insert(marker, block);
        cursor = detail.end;
    }
    transformed.push_str(&source[cursor..]);
    let parsed = parse_markdown(&transformed, extensions.clone())?;
    let mut defs = defs;
    collect_definitions(&parsed, &mut defs);
    let mut blocks = Vec::new();
    for node in &parsed {
        import_block_with_folds(node, &defs, &mut blocks, Some(&transformed), Some(&folds), Some(&extensions));
    }
    Ok(blocks)
}

fn leaf(kind: BlockKindData, content: BlockContent) -> DetachedBlock {
    DetachedBlock::leaf(kind, content)
}

fn import_children(nodes: &[BlockNode], defs: &Defs, source: Option<&str>, folds: Option<&HashMap<String, DetachedBlock>>, extensions: Option<&Arc<MarkdownExtensions>>) -> Vec<DetachedBlock> {
    let mut out = Vec::new();
    for node in nodes {
        import_block_with_folds(node, defs, &mut out, source, folds, extensions);
    }
    out
}

pub(crate) fn import_block(node: &BlockNode, defs: &Defs, out: &mut Vec<DetachedBlock>) {
    import_block_source(node, defs, out, None);
}

pub(crate) fn import_block_source(node: &BlockNode, defs: &Defs, out: &mut Vec<DetachedBlock>, source: Option<&str>) {
    import_block_with_folds(node, defs, out, source, None, None);
}

fn import_block_with_folds(node: &BlockNode, defs: &Defs, out: &mut Vec<DetachedBlock>, source: Option<&str>, folds: Option<&HashMap<String, DetachedBlock>>, extensions: Option<&Arc<MarkdownExtensions>>) {
    let block = match node {
        // An HTML container: splice its children in place.
        BlockNode::Root { children, .. } => {
            for child in children {
                import_block_with_folds(child, defs, out, source, folds, extensions);
            }
            return;
        }
        BlockNode::Paragraph(paragraph) => leaf(
            BlockKindData::Paragraph,
            BlockContent::Inline(import_inline(paragraph, defs)),
        ),
        BlockNode::Heading {
            level, children, ..
        } => leaf(
            BlockKindData::Heading { level: *level },
            BlockContent::Inline(import_inline(children, defs)),
        ),
        BlockNode::Blockquote { children, .. } => {
            if let Some(BlockNode::Paragraph(paragraph)) = children.first() {
                let inline = import_inline(paragraph, defs);
                let text = inline.text();
                if let Some(rest) = text.strip_prefix("[!")
                    && let Some(end) = rest.find(']')
                    && end > 0
                    && let Some(sign) = rest.as_bytes().get(end + 1)
                    && (*sign == b'+' || *sign == b'-')
                    && source.and_then(|s| children.first().and_then(BlockNode::span).and_then(|span| s.get(span.start..span.end)))
                        .is_some_and(|raw| raw.starts_with(&text[..2 + end + 2]))
                    && rest.get(end + 2..).is_some_and(|s| s.chars().next().is_none_or(|ch| matches!(ch, ' ' | '\t' | '\n')))
                {
                    let name = rest[..end].into();
                    let start = 2 + end + 2;
                    let title_start = start + text[start..].len() - text[start..].trim_start_matches([' ', '\t']).len();
                    let raw = source.and_then(|s| children.first().and_then(BlockNode::span).and_then(|span| s.get(span.start..span.end))).unwrap();
                    let title_end = if let Some((first_line, _)) = raw.split_once('\n') {
                        parse_markdown(first_line, extensions.unwrap().clone())
                            .expect("fold title must parse")
                            .into_iter().find_map(|node| match node {
                                BlockNode::Paragraph(p) => Some(import_inline(&p, defs).len()),
                                _ => None,
                            }).expect("fold title must be a paragraph")
                    } else {
                        text.len()
                    };
                    let mut blocks = vec![DetachedBlock::paragraph(inline.slice(title_start..title_end))];
                    if title_end < text.len() {
                        blocks.push(DetachedBlock::paragraph(inline.slice(title_end + 1..text.len())));
                    }
                    blocks.extend(import_children(&children[1..], defs, source, folds, extensions));
                    DetachedBlock::container(BlockKindData::Fold {
                        syntax: FoldSyntax::Alert { name },
                        initially_open: *sign == b'+',
                    }, blocks)
                } else {
                    DetachedBlock::container(BlockKindData::Blockquote, import_children(children, defs, source, folds, extensions))
                }
            } else {
                DetachedBlock::container(BlockKindData::Blockquote, import_children(children, defs, source, folds, extensions))
            }
        }
        BlockNode::Surface { name, children, .. } => {
            let mut children = import_children(children, defs, source, folds, extensions);
            if children.is_empty() {
                children.push(leaf(BlockKindData::Paragraph, BlockContent::Inline(InlineText::new())));
            }
            DetachedBlock::container(BlockKindData::Alert { name: name.clone() }, children)
        }
        BlockNode::List {
            children,
            ordered,
            start,
            ..
        } => DetachedBlock::container(
            BlockKindData::List {
                ordered: *ordered,
                start: *start,
            },
            import_children(children, defs, source, folds, extensions),
        ),
        BlockNode::ListItem {
            children,
            spread,
            checked,
            ..
        } => {
            let mut children = import_children(children, defs, source, folds, extensions);
            // A parsed bare marker is an editable empty item, not an empty
            // container to prune during normalization.
            if children.is_empty() {
                children.push(DetachedBlock::paragraph(InlineText::new()));
            }
            DetachedBlock::container(
                BlockKindData::ListItem {
                    checked: *checked,
                    spread: *spread,
                },
                children,
            )
        }
        BlockNode::CodeBlock(code) => {
            let text = code.code().to_string();
            leaf(
                BlockKindData::CodeBlock {
                    lang: code.lang(),
                    meta: code.meta(),
                },
                BlockContent::Plain(text),
            )
        }
        BlockNode::Custom { node, .. } => {
            if let Some(block) = folds.and_then(|folds| folds.get(node.as_markdown())) {
                block.clone()
            } else {
                leaf(BlockKindData::Custom, BlockContent::Atom(AtomData::Custom(node.clone())))
            }
        },
        BlockNode::Table(table) => {
            let rows = table
                .rows()
                .map(|row| {
                    DetachedBlock::container(
                        BlockKindData::TableRow,
                        row.map(|cell| {
                            leaf(
                                BlockKindData::TableCell,
                                BlockContent::Inline(import_inline(cell, defs)),
                            )
                        })
                        .collect(),
                    )
                })
                .collect();
            DetachedBlock::container(
                BlockKindData::Table {
                    aligns: table.column_aligns().to_vec(),
                },
                rows,
            )
        }
        BlockNode::Break { html, .. } => leaf(
            BlockKindData::Break { html: *html },
            BlockContent::Atom(AtomData::None),
        ),
        BlockNode::HorizontalRule { .. } => leaf(
            BlockKindData::HorizontalRule,
            BlockContent::Atom(AtomData::None),
        ),
        BlockNode::Definition {
            identifier,
            url,
            title,
            ..
        } => leaf(
            BlockKindData::Definition {
                identifier: identifier.clone(),
                url: url.clone(),
                title: title.clone(),
            },
            BlockContent::Atom(AtomData::None),
        ),
        BlockNode::Unknown | _ => leaf(BlockKindData::Unknown, BlockContent::Atom(AtomData::None)),
    };
    out.push(block);
}

/// Convert a paragraph's inline nodes into normalized runs. `InlineNode.marks`
/// may overlap (`**b*c*d**` gives bold 0..3 and italic 1..2), so split at
/// every boundary and merge the marks covering each piece.
///
/// Reference links (`[text][id]`) arrive with an empty url and an
/// identifier; they are resolved against the document's definitions so the
/// canonical output is an inline link. Unresolved references (and footnote
/// links) lose their link mark: documented lossiness.
pub(crate) fn import_inline(paragraph: &Paragraph, defs: &Defs) -> InlineText {
    let mut out = InlineText::new();
    for node in paragraph.children() {
        out.push(import_inline_node(node));
    }
    let len = out.len();
    out.map_marks(0..len, |mark| match &mark.link {
        Some(link) if link.url.is_empty() => {
            let link = link
                .identifier
                .as_ref()
                .and_then(|id| defs.get(&SharedString::from(id.to_lowercase())))
                .map(|(url, title)| LinkMark {
                    url: url.clone(),
                    identifier: None,
                    title: title.clone(),
                });
            TextMark {
                link,
                ..mark.clone()
            }
        }
        _ => mark.clone(),
    });
    out
}

pub(crate) type Defs = HashMap<SharedString, (SharedString, Option<SharedString>)>;

pub(crate) fn collect_definitions(nodes: &[BlockNode], defs: &mut Defs) {
    for node in nodes {
        match node {
            BlockNode::Definition {
                identifier,
                url,
                title,
                ..
            } => {
                defs.insert(
                    identifier.to_lowercase().into(),
                    (url.clone(), title.clone()),
                );
            }
            BlockNode::Root { children, .. }
            | BlockNode::Blockquote { children, .. }
            | BlockNode::List { children, .. }
            | BlockNode::ListItem { children, .. } => collect_definitions(children, defs),
            _ => {}
        }
    }
}

fn import_inline_node(node: &InlineNode) -> InlineText {
    let merged_marks = || {
        let mut mark = TextMark::default();
        for (_, layer) in node.mark_runs() {
            mark.merge(layer.clone());
        }
        mark
    };
    if let Some(custom) = node.as_custom() {
        return InlineText::atom(InlineAtom::Custom(custom.clone()), merged_marks());
    }
    if let Some(image) = node.as_image() {
        return InlineText::atom(InlineAtom::Image(image.clone()), merged_marks());
    }
    let text: &str = node.text();
    let mut cuts = vec![0, text.len()];
    for (range, _) in node.mark_runs() {
        cuts.push(range.start.min(text.len()));
        cuts.push(range.end.min(text.len()));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = InlineText::new();
    for pair in cuts.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        if lo >= hi || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
            continue;
        }
        let mut mark = TextMark::default();
        for (range, layer) in node.mark_runs() {
            if range.start <= lo && hi <= range.end {
                mark.merge(layer.clone());
            }
        }
        out.push(InlineText::plain(&text[lo..hi], mark));
    }
    out
}

/// Footnotes and unsupported HTML are outside the editable block vocabulary. Keep their original
/// source in opaque, selectable atoms rather than accepting the display
/// parser's decorated-prose projection. Uses the same AST/parser pipeline as
/// every other editor node; no second parser or mutable source mirror.
pub(crate) fn preserve_source_extensions(extensions: MarkdownExtensions) -> MarkdownExtensions {
    use ::markdown::mdast::Node;
    use gpui_kit::base::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin};

    struct SourceAtom {
        block: bool,
    }
    impl MarkdownPlugin for SourceAtom {
        fn name(&self) -> &str {
            if self.block {
                "editor-source-block"
            } else {
                "editor-source-inline"
            }
        }
        fn is_block(&self) -> bool {
            self.block
        }
        fn parse(&self, node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
            let known_html = if let Node::Html(html) = node {
                let lower = html.value.to_ascii_lowercase();
                let tag = lower
                    .trim_start_matches('<')
                    .trim_start_matches('/')
                    .split(|ch: char| !ch.is_ascii_alphabetic())
                    .next()
                    .unwrap_or_default();
                matches!(tag, "u" | "mark" | "em" | "strong" | "s" | "del" | "br")
            } else {
                false
            };
            let accepts = if self.block {
                matches!(node, Node::FootnoteDefinition(_))
                    || (matches!(node, Node::Html(_)) && !known_html)
            } else {
                matches!(node, Node::FootnoteReference(_))
                    || (matches!(node, Node::Html(_)) && !known_html)
            };
            if !accepts {
                return None;
            }
            let source = cx.node_source(node)?;
            // Block spans may include separator line endings. The root owns
            // those boundaries; retaining them here adds a blank line on every
            // export/import cycle. Internal content remains verbatim.
            let source = if self.block {
                source.trim_end_matches(['\r', '\n'])
            } else {
                source
            };
            let source = if self.block {
                // The AST span starts after the first container marker, but
                // later source lines still include it. Strip only matching
                // continuation prefixes, preserving body indentation and lazy
                // lines. The root serializer will add the containers once.
                let start = node.position()?.start.offset;
                let line_start = cx.source()[..start].rfind('\n').map_or(0, |ix| ix + 1);
                let prefix = &cx.source()[line_start..start];
                let continuation = prefix
                    .chars()
                    .map(|ch| if ch == '>' { '>' } else { ' ' })
                    .collect::<String>();
                source
                    .split('\n')
                    .enumerate()
                    .map(|(ix, line)| {
                        if ix == 0 || continuation.is_empty() {
                            line
                        } else if line.trim_end() == continuation.trim_end() {
                            ""
                        } else {
                            line.strip_prefix(&continuation).unwrap_or(line)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                source.to_owned()
            };
            Some(
                MarkdownNode::new(self.name(), ())
                    .text(source.clone())
                    .markdown(source),
            )
        }
    }
    extensions
        .plugin(SourceAtom { block: true })
        .plugin(SourceAtom { block: false })
}
