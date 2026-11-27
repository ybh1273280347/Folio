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

fn blockquote_prefix(line: &str) -> (&str, usize) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut depth = 0;
    let mut after_last_marker = 0;
    while i < bytes.len() {
        let mut j = i;
        let mut spaces = 0;
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
            spaces += 1;
            if spaces > 3 {
                break;
            }
            j += 1;
        }
        if spaces <= 3 && j < bytes.len() && bytes[j] == b'>' {
            j += 1;
            depth += 1;
            if j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            after_last_marker = j;
            i = j;
        } else {
            break;
        }
    }
    (&line[..after_last_marker], depth)
}

fn strip_quote_prefix(mut line: &str, depth: usize) -> &str {
    for _ in 0..depth {
        let bytes = line.as_bytes();
        let mut j = 0;
        while j < bytes.len() && j < 3 && matches!(bytes[j], b' ' | b'\t') {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'>' {
            j += 1;
            if j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            line = &line[j..];
        } else {
            break;
        }
    }
    line
}

pub fn normalize_math(source: &str) -> String {
    // With no possible TeX opener, there is nothing to rewrite or protect in code spans.
    if !source.contains('$') && !source.contains("\\(") && !source.contains("\\[") {
        return source.to_owned();
    }

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
                let line_head = result
                    .rsplit_once('\n')
                    .map_or(result.as_str(), |(_, tail)| tail)
                    .to_owned();
                let (quote_prefix, quote_depth) = blockquote_prefix(&line_head);
                let only_prefix = line_head[quote_prefix.len()..]
                    .trim_matches([' ', '\t'])
                    .is_empty();
                let clean_body = if quote_depth > 0 && body.contains('\n') {
                    let mut lines = body.split('\n');
                    let mut out = String::from(
                        lines.next().unwrap_or_default().trim_end_matches('\r'),
                    );
                    for line in lines {
                        out.push('\n');
                        out.push_str(strip_quote_prefix(
                            line.trim_end_matches('\r'),
                            quote_depth,
                        ));
                    }
                    out
                } else {
                    body
                };
                let trimmed = clean_body.trim();
                if trimmed.is_empty() {
                    let ch = source[cursor..]
                        .chars()
                        .next()
                        .expect("cursor is inside source");
                    result.push(ch);
                    cursor += ch.len_utf8();
                    continue;
                }
                let continuation = if quote_depth > 0 {
                    if only_prefix {
                        if line_head.ends_with([' ', '\t']) {
                            line_head.clone()
                        } else {
                            format!("{line_head} ")
                        }
                    } else if quote_prefix.ends_with([' ', '\t']) {
                        quote_prefix.to_owned()
                    } else {
                        format!("{quote_prefix} ")
                    }
                } else if only_prefix {
                    line_head.clone()
                } else {
                    String::new()
                };
                // markdown-rs only emits flow math when fences occupy separate lines,
                // and preserving container prefixes keeps display math inside callouts/quotes.
                if quote_depth > 0 {
                    if !only_prefix {
                        result.push('\n');
                        result.push_str(&continuation);
                    } else if !line_head.ends_with([' ', '\t']) {
                        result.push(' ');
                    }
                } else if !result.is_empty() && !result.ends_with('\n') && !only_prefix {
                    result.push('\n');
                }
                result.push_str("$$\n");
                for line in trimmed.split('\n') {
                    if quote_depth > 0 && line.is_empty() {
                        result.push_str(continuation.trim_end_matches([' ', '\t']));
                    } else {
                        result.push_str(&continuation);
                    }
                    result.push_str(line);
                    result.push('\n');
                }
                result.push_str(&continuation);
                result.push_str("$$");
                if end < source.len() && !source[end..].starts_with(['\r', '\n']) {
                    result.push('\n');
                    result.push_str(&continuation);
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

#[cfg(test)]
mod tests {
    use super::normalize_math;

    #[test]
    fn normalize_math_preserves_blockquote_and_callout_prefixes() {
        let quoted = "> [!NOTE]+ Title\n> $$\n> \\alpha = \\frac{n}{s \\cdot m}\n> $$\n> after";
        assert_eq!(normalize_math(quoted), quoted);

        let single = "> [!NOTE]\n> $$a + b$$\n> tail";
        assert_eq!(
            normalize_math(single),
            "> [!NOTE]\n> $$\n> a + b\n> $$\n> tail"
        );

        let nested = "> > \\[x = 1\\]\n> > tail";
        assert_eq!(
            normalize_math(nested),
            "> > $$\n> > x = 1\n> > $$\n> > tail"
        );

        let gt_inside = "> $$\n> x > 0\n> > 1\n> $$";
        assert_eq!(normalize_math(gt_inside), gt_inside);

        let empty_quoted = "> $$\n> $$";
        assert_eq!(normalize_math(empty_quoted), empty_quoted);

        let list_indented = "- item\n  $$a + b$$";
        assert_eq!(normalize_math(list_indented), "- item\n  $$\n  a + b\n  $$");
    }
}
