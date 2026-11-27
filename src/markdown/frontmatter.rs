//! Lossless YAML Front Matter extraction, display parsing, and reassembly.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrontMatterValue {
    Text(String),
    Bool(bool),
    List(Vec<String>),
    Raw(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrontMatterEntry {
    pub key: String,
    pub value: FrontMatterValue,
}

/// A parsed YAML Front Matter block at the very start of a Markdown document.
///
/// `raw_block` preserves the exact original bytes (BOM, opening `---`, comments,
/// indentation, closing `---`/`...`, and trailing separator newlines) so editing
/// the Markdown body never mutates the user's Front Matter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrontMatter {
    pub raw_block: String,
    pub yaml_body: String,
    pub opening: String,
    pub closing_and_sep: String,
    pub line_count: usize,
    pub entries: Vec<FrontMatterEntry>,
}

impl FrontMatter {
    /// Reassemble the full Markdown source from this Front Matter and a body string.
    pub fn join_body(&self, body: &str) -> String {
        let mut out = String::with_capacity(self.raw_block.len() + body.len());
        out.push_str(&self.raw_block);
        out.push_str(body);
        out
    }

    /// Replace only the inner YAML text while keeping the original fences and separator.
    pub fn with_yaml_body(&self, next_yaml: &str) -> Self {
        let newline = if self.opening.ends_with("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let normalized_yaml = if next_yaml.is_empty() || next_yaml.ends_with('\n') {
            next_yaml.to_owned()
        } else {
            format!("{next_yaml}{newline}")
        };
        let raw_block = format!("{}{normalized_yaml}{}", self.opening, self.closing_and_sep);
        let line_count = raw_block.bytes().filter(|&b| b == b'\n').count();
        let trimmed_yaml = normalized_yaml
            .strip_suffix("\r\n")
            .or_else(|| normalized_yaml.strip_suffix('\n'))
            .unwrap_or(&normalized_yaml)
            .to_owned();
        let entries = parse_entries(&trimmed_yaml);
        Self {
            raw_block,
            yaml_body: trimmed_yaml,
            opening: self.opening.clone(),
            closing_and_sep: self.closing_and_sep.clone(),
            line_count,
            entries,
        }
    }
}

/// Split optional leading YAML Front Matter from a Markdown document.
///
/// Returns `(Some(frontmatter), body)` when the document begins with a valid
/// `---` ... `---` (or `...`) YAML metadata block; otherwise `(None, source)`.
pub(crate) fn split(source: &str) -> (Option<FrontMatter>, &str) {
    let bom_len = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let rest = &source[bom_len..];
    let Some(first_nl) = rest.find('\n') else {
        return (None, source);
    };
    let first_line_with_nl = &rest[..=first_nl];
    let first_line = first_line_with_nl
        .strip_suffix("\r\n")
        .or_else(|| first_line_with_nl.strip_suffix('\n'))
        .unwrap_or(first_line_with_nl);
    if first_line.trim_end_matches([' ', '\t']) != "---" {
        return (None, source);
    }

    let opening_end = bom_len + first_nl + 1;
    let mut cursor = opening_end;
    let mut closing_start = None;
    let mut closing_end = None;

    while cursor < source.len() {
        let line_end = source[cursor..]
            .find('\n')
            .map_or(source.len(), |idx| cursor + idx + 1);
        let line = &source[cursor..line_end];
        let trimmed = line
            .strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
            .unwrap_or(line)
            .trim_end_matches([' ', '\t']);
        if trimmed == "---" || trimmed == "..." {
            closing_start = Some(cursor);
            closing_end = Some(line_end);
            break;
        }
        cursor = line_end;
    }

    let (Some(close_start), Some(mut block_end)) = (closing_start, closing_end) else {
        return (None, source);
    };

    let yaml_with_nl = &source[opening_end..close_start];
    let yaml_body = yaml_with_nl
        .strip_suffix("\r\n")
        .or_else(|| yaml_with_nl.strip_suffix('\n'))
        .unwrap_or(yaml_with_nl);

    // Guard against mistaking two horizontal rules around normal prose for YAML.
    let has_non_comment_content = yaml_body.lines().any(|line| {
        let t = line.trim();
        !t.is_empty() && !t.starts_with('#')
    });
    if has_non_comment_content && !yaml_body.lines().any(|line| line_has_mapping_key(line)) {
        return (None, source);
    }

    // Include trailing blank lines right after the closing fence in `raw_block`
    // so the visual body starts cleanly at the first real Markdown block and
    // round-trips without gaining or losing blank lines.
    while block_end < source.len() {
        let next_end = source[block_end..]
            .find('\n')
            .map_or(source.len(), |idx| block_end + idx + 1);
        let next_line = &source[block_end..next_end];
        if next_line.trim().is_empty() {
            block_end = next_end;
        } else {
            break;
        }
    }

    let raw_block = source[..block_end].to_owned();
    let opening = source[..opening_end].to_owned();
    let closing_and_sep = source[close_start..block_end].to_owned();
    let line_count = raw_block.bytes().filter(|&b| b == b'\n').count();
    let entries = parse_entries(yaml_body);

    (
        Some(FrontMatter {
            raw_block,
            yaml_body: yaml_body.to_owned(),
            opening,
            closing_and_sep,
            line_count,
            entries,
        }),
        &source[block_end..],
    )
}

pub(crate) fn body_of(source: &str) -> &str {
    split(source).1
}

fn line_has_mapping_key(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return false;
    }
    split_key_value(trimmed).is_some()
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b':' if !in_single && !in_double => {
                let next = bytes.get(idx + 1).copied();
                if next.is_none() || matches!(next, Some(b' ' | b'\t')) {
                    let key = line[..idx].trim();
                    if !key.is_empty() {
                        let val = line[idx + 1..].trim();
                        return Some((unquote(key), val));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn unquote(raw: &str) -> &str {
    let s = raw.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn parse_flow_list(raw: &str) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with('[') && trimmed.ends_with(']')) {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    let mut items = Vec::new();
    let mut start = 0;
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in inner.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ',' if !in_single && !in_double => {
                let part = unquote(&inner[start..idx]).trim();
                if !part.is_empty() {
                    items.push(part.to_owned());
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    let tail = unquote(&inner[start..]).trim();
    if !tail.is_empty() {
        items.push(tail.to_owned());
    }
    Some(items)
}

/// Lightweight display parser for top-level YAML Front Matter fields.
fn parse_entries(yaml: &str) -> Vec<FrontMatterEntry> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut entries = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        // Top-level keys start at column 0.
        if line.starts_with([' ', '\t']) {
            i += 1;
            continue;
        }
        let Some((key, val)) = split_key_value(line) else {
            i += 1;
            continue;
        };
        let key = key.to_owned();
        i += 1;

        // Block scalar (`|`, `>`, `|-`, `>-`, `|+`, `>+`)
        if matches!(val, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
            let folded = val.starts_with('>');
            let mut block_lines = Vec::new();
            let mut base_indent: Option<usize> = None;
            while i < lines.len() {
                let next = lines[i];
                if next.trim().is_empty() {
                    block_lines.push("");
                    i += 1;
                    continue;
                }
                let indent = next.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
                if indent == 0 {
                    break;
                }
                let min_indent = *base_indent.get_or_insert(indent);
                let cut = indent.min(min_indent);
                block_lines.push(&next[cut..]);
                i += 1;
            }
            while block_lines.last().is_some_and(|l| l.is_empty()) {
                block_lines.pop();
            }
            let joined = if folded {
                block_lines.join(" ").trim().to_owned()
            } else {
                block_lines.join("\n")
            };
            entries.push(FrontMatterEntry {
                key,
                value: FrontMatterValue::Text(joined),
            });
            continue;
        }

        // Empty inline value: may be followed by an indented block list or nested map.
        if val.is_empty() {
            let mut sub_lines = Vec::new();
            while i < lines.len() {
                let next = lines[i];
                if next.trim().is_empty() {
                    sub_lines.push(next);
                    i += 1;
                    continue;
                }
                let starts_indented = next.starts_with([' ', '\t']);
                let starts_dash = next.starts_with("- ") || next.trim() == "-";
                if !starts_indented && !starts_dash {
                    break;
                }
                sub_lines.push(next);
                i += 1;
            }
            let non_empty: Vec<&str> = sub_lines
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            if non_empty.is_empty() {
                entries.push(FrontMatterEntry {
                    key,
                    value: FrontMatterValue::Text(String::new()),
                });
            } else if non_empty
                .iter()
                .all(|l| l.starts_with("- ") || *l == "-")
            {
                let items = non_empty
                    .into_iter()
                    .filter_map(|l| {
                        let item = l.strip_prefix("- ").unwrap_or("").trim();
                        let item = unquote(item).trim();
                        (!item.is_empty()).then(|| item.to_owned())
                    })
                    .collect();
                entries.push(FrontMatterEntry {
                    key,
                    value: FrontMatterValue::List(items),
                });
            } else {
                let raw = sub_lines
                    .into_iter()
                    .map(|l| l.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim_matches('\n')
                    .to_owned();
                entries.push(FrontMatterEntry {
                    key,
                    value: FrontMatterValue::Raw(raw),
                });
            }
            continue;
        }

        if let Some(items) = parse_flow_list(val) {
            entries.push(FrontMatterEntry {
                key,
                value: FrontMatterValue::List(items),
            });
            continue;
        }

        if val.eq_ignore_ascii_case("true") {
            entries.push(FrontMatterEntry {
                key,
                value: FrontMatterValue::Bool(true),
            });
            continue;
        }
        if val.eq_ignore_ascii_case("false") {
            entries.push(FrontMatterEntry {
                key,
                value: FrontMatterValue::Bool(false),
            });
            continue;
        }

        // Plain or quoted scalar (collect any indented continuation lines)
        let mut text = unquote(val).to_owned();
        while i < lines.len() {
            let next = lines[i];
            if next.starts_with([' ', '\t']) && !next.trim().is_empty() && !next.trim_start().starts_with('#') {
                text.push(' ');
                text.push_str(next.trim());
                i += 1;
            } else {
                break;
            }
        }
        entries.push(FrontMatterEntry {
            key,
            value: FrontMatterValue::Text(text),
        });
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_skill_frontmatter_and_roundtrips_losslessly() {
        let src = "---\nname: folio-intro\ndescription: 面向普通用户介绍 Folio 是什么。\ndisable-model-invocation: false\n---\n\n# 介绍 Folio\n\n正文\n";
        let (fm, body) = split(src);
        let fm = fm.expect("frontmatter detected");
        assert_eq!(body, "# 介绍 Folio\n\n正文\n");
        assert_eq!(fm.line_count, 6);
        assert_eq!(fm.entries.len(), 3);
        assert_eq!(
            fm.entries[0],
            FrontMatterEntry {
                key: "name".into(),
                value: FrontMatterValue::Text("folio-intro".into()),
            }
        );
        assert_eq!(
            fm.entries[2],
            FrontMatterEntry {
                key: "disable-model-invocation".into(),
                value: FrontMatterValue::Bool(false),
            }
        );
        assert_eq!(fm.join_body(body), src);
    }

    #[test]
    fn parses_block_and_flow_lists_and_block_scalars() {
        let src = "---\ntags: [rust, \"gpui\", markdown]\nauthors:\n  - Alice\n  - Bob\nsummary: >-\n  First line\n  second line\n---\nBody";
        let (fm, body) = split(src);
        let fm = fm.expect("frontmatter detected");
        assert_eq!(body, "Body");
        assert_eq!(
            fm.entries[0].value,
            FrontMatterValue::List(vec!["rust".into(), "gpui".into(), "markdown".into()])
        );
        assert_eq!(
            fm.entries[1].value,
            FrontMatterValue::List(vec!["Alice".into(), "Bob".into()])
        );
        assert_eq!(
            fm.entries[2].value,
            FrontMatterValue::Text("First line second line".into())
        );
    }

    #[test]
    fn ignores_horizontal_rules_without_yaml_mappings() {
        let src = "---\n\nHello world\n\n---\n\n# Heading\n";
        let (fm, body) = split(src);
        assert!(fm.is_none());
        assert_eq!(body, src);
    }

    #[test]
    fn editing_yaml_body_preserves_fences_and_separator() {
        let src = "---\nname: old\n---\n\n# Body\n";
        let (fm, body) = split(src);
        let updated = fm.unwrap().with_yaml_body("name: new\ndraft: true");
        assert_eq!(updated.join_body(body), "---\nname: new\ndraft: true\n---\n\n# Body\n");
        assert_eq!(updated.entries.len(), 2);
        assert_eq!(updated.entries[1].value, FrontMatterValue::Bool(true));
    }
}
