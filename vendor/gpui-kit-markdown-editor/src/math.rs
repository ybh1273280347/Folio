//! Recognize TeX before Markdown escapes it; importing and live typing share fences.
use gpui::SharedString;
use gpui_kit::base::text::{MarkdownExtensions, editor_host::BlockNode};
use std::sync::Arc;

/// Accept one explicitly escaped clipboard layer as well as ordinary TeX fences.
fn formula_at(source: &str, start: usize) -> Option<(usize, String, bool)> {
    if escaped(source, start) {
        return None;
    }
    let tail = &source[start..];
    let (open, close, display, copied) = [
        ("$$", "$$", true, false),
        ("\\\\[", "\\\\]", true, true),
        ("\\\\(", "\\\\)", false, true),
        ("\\[", "\\]", true, false),
        ("\\(", "\\)", false, false),
        ("$", "$", false, false),
    ]
    .into_iter()
    .find(|(open, _, _, _)| tail.starts_with(open))?;
    let body_start = start + open.len();
    let end = source[body_start..]
        .match_indices(close)
        .map(|(offset, _)| body_start + offset)
        .find(|offset| !escaped(source, *offset))?;
    let body = &source[body_start..end];
    if body.trim().is_empty() {
        return None;
    }
    // JSON-style copied TeX doubles control slashes. Ordinary aligned/matrix
    // formulas keep their real `\\` row separators, including during live input.
    let mut single_command = false;
    let mut double_command = false;
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            continue;
        }
        let mut count = 1;
        while chars.peek() == Some(&'\\') {
            chars.next();
            count += 1;
        }
        if chars.peek().is_some_and(char::is_ascii_alphabetic) {
            if count == 1 {
                single_command = true;
            } else if count == 2 {
                double_command = true;
            }
        }
    }
    let body = if copied || (double_command && !single_command) {
        body.replace("\\\\", "\\")
    } else {
        body.to_owned()
    };
    Some((end + close.len(), body, display))
}

pub(crate) fn math_range_at(source: &str, caret: usize) -> Option<(usize, usize, String, bool)> {
    let mut cursor = 0;
    while cursor < source.len() {
        if let Some((end, body, display)) = formula_at(source, cursor) {
            if caret >= cursor && caret <= end {
                return Some((cursor, end, body, display));
            }
            cursor = end;
        } else {
            cursor += source[cursor..]
                .chars()
                .next()
                .expect("source character")
                .len_utf8();
        }
    }
    None
}

pub fn normalize_math(source: &str) -> String {
    fn code_ranges(node: &markdown::mdast::Node, ranges: &mut Vec<std::ops::Range<usize>>) {
        if matches!(
            node,
            markdown::mdast::Node::Code(_) | markdown::mdast::Node::InlineCode(_)
        ) {
            let position = node.position().expect("parsed code has source positions");
            ranges.push(position.start.offset..position.end.offset);
        } else if let Some(children) = node.children() {
            for child in children {
                code_ranges(child, ranges);
            }
        }
    }
    let mut code = Vec::new();
    let ast = markdown::to_mdast(source, &markdown::ParseOptions::gfm())
        .expect("GFM without MDX accepts editor source");
    code_ranges(&ast, &mut code);
    let mut code = code.into_iter().peekable();
    let mut result = String::with_capacity(source.len());
    let mut cursor = 0;
    while cursor < source.len() {
        if let Some(range) = code.peek().filter(|range| range.start == cursor) {
            result.push_str(&source[range.clone()]);
            cursor = range.end;
            code.next();
            continue;
        }
        if let Some((end, body, display)) = formula_at(source, cursor)
            .filter(|(end, _, _)| code.peek().is_none_or(|range| range.start >= *end))
        {
            if display {
                // markdown-rs only emits flow math when fences occupy separate lines.
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str("$$\n");
                result.push_str(body.trim());
                result.push_str("\n$$");
                if end < source.len() && !source[end..].starts_with(['\r', '\n']) {
                    result.push('\n');
                }
            } else {
                result.push('$');
                result.push_str(&body);
                result.push('$');
            }
            cursor = end;
        } else {
            let ch = source[cursor..]
                .chars()
                .next()
                .expect("cursor is inside source");
            result.push(ch);
            cursor += ch.len_utf8();
        }
    }
    result
}

fn escaped(source: &str, index: usize) -> bool {
    source[..index]
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count()
        % 2
        == 1
}

pub(crate) fn parse_markdown(
    source: &str,
    extensions: Arc<MarkdownExtensions>,
) -> Result<Vec<BlockNode>, SharedString> {
    gpui_kit::base::text::editor_host::parse_markdown(&normalize_math(source), extensions)
}
