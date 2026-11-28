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
pub(crate) fn parse_entries(yaml: &str) -> Vec<FrontMatterEntry> {
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
                let min_indent = sub_lines
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.bytes().take_while(|b| *b == b' ' || *b == b'\t').count())
                    .min()
                    .unwrap_or(0);
                let raw = sub_lines
                    .into_iter()
                    .map(|l| {
                        let trimmed = l.trim_end();
                        if trimmed.len() >= min_indent {
                            &trimmed[min_indent..]
                        } else {
                            trimmed
                        }
                    })
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

/// Flip a top-level boolean field in-place inside a YAML Front Matter body,
/// preserving comments, ordering, and all other lines untouched.
pub(crate) fn toggle_bool_in_yaml(yaml: &str, target_key: &str, next: bool) -> String {
    let replacement = if next { "true" } else { "false" };
    let mut out = Vec::new();
    let mut updated = false;
    for line in yaml.lines() {
        if !updated && !line.starts_with([' ', '\t']) {
            if let Some((key, val)) = split_key_value(line)
                && key == target_key
                && (val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("false"))
                && let Some(colon) = line.find(':')
            {
                out.push(format!("{}: {replacement}", &line[..colon]));
                updated = true;
                continue;
            }
        }
        out.push(line.to_owned());
    }
    out.join("\n")
}

pub(crate) type TagColorLookup =
    std::sync::Arc<dyn Fn(&str, &gpui_kit::App) -> Option<(u32, u32)> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldType {
    Text,
    List,
    Bool,
}

fn find_key_line_span(lines: &[&str], target_key: &str) -> Option<(usize, usize, bool)> {
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if !line.starts_with([' ', '\t'])
            && let Some((key, val)) = split_key_value(line)
            && key == target_key
        {
            let start = i;
            let mut end = i + 1;
            let mut is_block_list = false;
            if val.is_empty() || matches!(val, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
                while end < lines.len() {
                    let next = lines[end];
                    if next.trim().is_empty() {
                        end += 1;
                        continue;
                    }
                    let indented = next.starts_with([' ', '\t']);
                    let dash = next.starts_with("- ") || next.trim() == "-";
                    if indented || dash {
                        if dash || next.trim_start().starts_with("- ") {
                            is_block_list = true;
                        }
                        end += 1;
                    } else {
                        break;
                    }
                }
                while end > start + 1 && lines[end - 1].trim().is_empty() {
                    end -= 1;
                }
            } else {
                while end < lines.len() {
                    let next = lines[end];
                    if next.starts_with([' ', '\t'])
                        && !next.trim().is_empty()
                        && !next.trim_start().starts_with('#')
                    {
                        end += 1;
                    } else {
                        break;
                    }
                }
            }
            return Some((start, end, is_block_list));
        }
        i += 1;
    }
    None
}

fn format_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("false")
        || parse_flow_list(trimmed).is_some()
    {
        return trimmed.to_owned();
    }
    let needs_quote = trimmed.starts_with(['-', '?', ':', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', '[', '{'])
        || trimmed.contains(": ")
        || trimmed.contains(" #");
    if needs_quote {
        let escaped = trimmed.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        trimmed.to_owned()
    }
}

pub(crate) fn set_text_value_in_yaml(yaml: &str, target_key: &str, new_val: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, end, _)) = find_key_line_span(&lines, target_key) else {
        return yaml.to_owned();
    };
    let key_prefix = lines[start].find(':').map_or(target_key, |idx| &lines[start][..idx]);
    let formatted = format_yaml_scalar(new_val);
    let replacement = if formatted.is_empty() {
        format!("{key_prefix}:")
    } else {
        format!("{key_prefix}: {formatted}")
    };
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.extend(lines[..start].iter().map(|s| (*s).to_owned()));
    out.push(replacement);
    out.extend(lines[end..].iter().map(|s| (*s).to_owned()));
    out.join("\n")
}

pub(crate) fn rename_key_in_yaml(yaml: &str, old_key: &str, new_key: &str) -> String {
    let new_key = new_key.trim();
    if new_key.is_empty() || new_key == old_key {
        return yaml.to_owned();
    }
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, _, _)) = find_key_line_span(&lines, old_key) else {
        return yaml.to_owned();
    };
    let rest = lines[start]
        .find(':')
        .map_or("", |idx| &lines[start][idx..]);
    let mut out: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
    out[start] = format!("{new_key}{rest}");
    out.join("\n")
}

pub(crate) fn remove_key_in_yaml(yaml: &str, target_key: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, end, _)) = find_key_line_span(&lines, target_key) else {
        return yaml.to_owned();
    };
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.extend(lines[..start].iter().map(|s| (*s).to_owned()));
    out.extend(lines[end..].iter().map(|s| (*s).to_owned()));
    out.join("\n")
}

pub(crate) fn set_list_in_yaml(yaml: &str, target_key: &str, items: &[String]) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, end, block_style)) = find_key_line_span(&lines, target_key) else {
        return yaml.to_owned();
    };
    let key_prefix = lines[start].find(':').map_or(target_key, |idx| &lines[start][..idx]);
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + items.len());
    out.extend(lines[..start].iter().map(|s| (*s).to_owned()));
    if items.is_empty() {
        out.push(format!("{key_prefix}: []"));
    } else if block_style {
        out.push(format!("{key_prefix}:"));
        for item in items {
            out.push(format!("  - {}", format_yaml_tag_item(item)));
        }
    } else {
        let formatted: Vec<String> = items.iter().map(|s| format_yaml_tag_item(s)).collect();
        out.push(format!("{key_prefix}: [{}]", formatted.join(", ")));
    }
    out.extend(lines[end..].iter().map(|s| (*s).to_owned()));
    out.join("\n")
}

pub(crate) fn convert_field_type_in_yaml(
    yaml: &str,
    target_key: &str,
    target: FieldType,
) -> String {
    let current = parse_entries(yaml)
        .into_iter()
        .find(|e| e.key == target_key)
        .map(|e| e.value);
    match target {
        FieldType::Bool => {
            let next = match current {
                Some(FrontMatterValue::Bool(b)) => b,
                Some(FrontMatterValue::Text(t)) => t.eq_ignore_ascii_case("true"),
                _ => false,
            };
            set_text_value_in_yaml(yaml, target_key, if next { "true" } else { "false" })
        }
        FieldType::List => {
            let items = match current {
                Some(FrontMatterValue::List(items)) => items,
                Some(FrontMatterValue::Text(t)) if !t.trim().is_empty() => t
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect(),
                Some(FrontMatterValue::Bool(b)) => vec![b.to_string()],
                _ => Vec::new(),
            };
            set_list_in_yaml(yaml, target_key, &items)
        }
        FieldType::Text => {
            let text = match current {
                Some(FrontMatterValue::Text(t)) => t,
                Some(FrontMatterValue::List(items)) => items.join(", "),
                Some(FrontMatterValue::Bool(b)) => b.to_string(),
                Some(FrontMatterValue::Raw(r)) => r.replace('\n', " "),
                None => String::new(),
            };
            let lines: Vec<&str> = yaml.lines().collect();
            let Some((start, end, _)) = find_key_line_span(&lines, target_key) else {
                return yaml.to_owned();
            };
            let key_prefix = lines[start].find(':').map_or(target_key, |idx| &lines[start][..idx]);
            let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
            let replacement = if text.is_empty() {
                format!("{key_prefix}:")
            } else if text.eq_ignore_ascii_case("true")
                || text.eq_ignore_ascii_case("false")
                || parse_flow_list(&text).is_some()
            {
                format!("{key_prefix}: \"{escaped}\"")
            } else {
                format!("{key_prefix}: {}", format_yaml_scalar(&text))
            };
            let mut out: Vec<String> = Vec::with_capacity(lines.len());
            out.extend(lines[..start].iter().map(|s| (*s).to_owned()));
            out.push(replacement);
            out.extend(lines[end..].iter().map(|s| (*s).to_owned()));
            out.join("\n")
        }
    }
}

pub(crate) fn append_key_in_yaml(yaml: &str, key: &str, as_list: bool) -> String {
    let key = key.trim();
    if key.is_empty() {
        return yaml.to_owned();
    }
    let line = if as_list {
        format!("{key}: []")
    } else {
        format!("{key}: ")
    };
    if yaml.trim().is_empty() {
        line
    } else {
        format!("{}\n{line}", yaml.trim_end_matches('\n'))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InlineTarget {
    Key(String),
    TextValue(String),
    AddListItem(String),
    NewRowKey,
}

struct FrontMatterInlineState {
    editing: Option<InlineTarget>,
    type_menu_key: Option<String>,
    input: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    yaml_snapshot: String,
    block: markdown_editor::BlockId,
    editor: gpui_kit::WeakEntity<markdown_editor::BlockEditorState>,
    _sub: gpui_kit::Subscription,
}

impl FrontMatterInlineState {
    fn new(
        block: markdown_editor::BlockId,
        editor: gpui_kit::WeakEntity<markdown_editor::BlockEditorState>,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Self {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::input::{InputEvent, InputState};
        let input = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
            match event {
                InputEvent::PressEnter { .. } => {
                    let val = state.read(cx).value().to_string();
                    this.commit_value(val, true, window, cx);
                }
                InputEvent::Blur => {
                    if this.editing.is_some() {
                        let val = state.read(cx).value().to_string();
                        this.commit_value(val, false, window, cx);
                    }
                }
                _ => {}
            }
        });
        Self {
            editing: None,
            type_menu_key: None,
            input,
            yaml_snapshot: String::new(),
            block,
            editor,
            _sub: sub,
        }
    }

    fn start(
        &mut self,
        target: InlineTarget,
        initial: String,
        placeholder: &'static str,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        self.type_menu_key = None;
        self.editing = Some(target);
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value(initial, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut gpui_kit::Context<Self>) {
        self.editing = None;
        cx.notify();
    }

    fn commit_value(
        &mut self,
        raw: String,
        from_enter: bool,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let Some(target) = self.editing.take() else {
            return;
        };
        let yaml = self.yaml_snapshot.clone();
        match target {
            InlineTarget::Key(old_key) => {
                let next_yaml = rename_key_in_yaml(&yaml, &old_key, &raw);
                if next_yaml != yaml {
                    self.yaml_snapshot = next_yaml.clone();
                    let block = self.block;
                    let _ = self.editor.update(cx, |ed, cx| {
                        ed.replace_plain_block_text(block, next_yaml, cx);
                    });
                }
            }
            InlineTarget::TextValue(key) => {
                let next_yaml = set_text_value_in_yaml(&yaml, &key, &raw);
                if next_yaml != yaml {
                    self.yaml_snapshot = next_yaml.clone();
                    let block = self.block;
                    let _ = self.editor.update(cx, |ed, cx| {
                        ed.replace_plain_block_text(block, next_yaml, cx);
                    });
                }
            }
            InlineTarget::AddListItem(key) => {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    let mut items = parse_entries(&yaml)
                        .into_iter()
                        .find(|e| e.key == key)
                        .and_then(|e| match e.value {
                            FrontMatterValue::List(v) => Some(v),
                            _ => None,
                        })
                        .unwrap_or_default();
                    for part in trimmed.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        if !items.iter().any(|existing| existing.eq_ignore_ascii_case(part)) {
                            items.push(part.to_owned());
                        }
                    }
                    let next_yaml = set_list_in_yaml(&yaml, &key, &items);
                    self.yaml_snapshot = next_yaml.clone();
                    let block = self.block;
                    let _ = self.editor.update(cx, |ed, cx| {
                        ed.replace_plain_block_text(block, next_yaml, cx);
                    });
                    if from_enter {
                        // Keep input open for rapid multi-item entry.
                        self.start(
                            InlineTarget::AddListItem(key),
                            String::new(),
                            crate::i18n::markdown::FM_ADD_ITEM_PLACEHOLDER.get(),
                            window,
                            cx,
                        );
                        return;
                    }
                }
            }
            InlineTarget::NewRowKey => {
                let key = raw.trim().to_owned();
                if !key.is_empty() {
                    let as_list = key.eq_ignore_ascii_case("tags")
                        || key.eq_ignore_ascii_case("aliases");
                    let next_yaml = append_key_in_yaml(&yaml, &key, as_list);
                    self.yaml_snapshot = next_yaml.clone();
                    let block = self.block;
                    let _ = self.editor.update(cx, |ed, cx| {
                        ed.replace_plain_block_text(block, next_yaml, cx);
                    });
                    if from_enter {
                        if as_list {
                            self.start(
                                InlineTarget::AddListItem(key),
                                String::new(),
                                crate::i18n::markdown::FM_ADD_ITEM_PLACEHOLDER.get(),
                                window,
                                cx,
                            );
                        } else {
                            self.start(
                                InlineTarget::TextValue(key),
                                String::new(),
                                "",
                                window,
                                cx,
                            );
                        }
                        return;
                    }
                }
            }
        }
        cx.notify();
    }
}

/// Render a minimal Obsidian-style Properties view inside the block editor.
/// All sizes scale proportionally with the document's body font size, tags reuse
/// the file-tree color palette, and every field supports independent inline editing.
pub(crate) fn render_preview(
    block: markdown_editor::BlockId,
    yaml: &str,
    editor: gpui_kit::WeakEntity<markdown_editor::BlockEditorState>,
    editor_font: &str,
    tag_colors: Option<TagColorLookup>,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    use gpui_kit::component::input::Input;
    use gpui_kit::{
        AnyElement, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
        SharedString, StatefulInteractiveElement, Styled, div, px, relative,
    };
    use gpui_kit::assets::IconName;

    let inline_state = window.use_keyed_state(
        ("fm-inline-state", block.as_u64() as usize),
        cx,
        |window, cx| FrontMatterInlineState::new(block, editor.clone(), window, cx),
    );
    inline_state.update(cx, |state, _| {
        state.yaml_snapshot = yaml.to_owned();
        state.block = block;
        state.editor = editor.clone();
    });

    let editing_target = inline_state.read(cx).editing.clone();
    let type_menu_key = inline_state.read(cx).type_menu_key.clone();
    let input_entity = inline_state.read(cx).input.clone();

    let palette = crate::theme::palette();
    let dark = palette.is_dark();
    let muted = gpui_kit::rgb(palette.muted);
    let text_color = gpui_kit::rgb(palette.text);
    let accent = gpui_kit::rgb(palette.accent);
    let row_hover = gpui_kit::rgb(palette.text).opacity(if dark { 0.04 } else { 0.025 });
    let subtle_tint = gpui_kit::rgb(palette.text).opacity(if dark { 0.045 } else { 0.022 });
    let mono_font: SharedString = if editor_font.is_empty() {
        SharedString::from("Consolas")
    } else {
        editor_font.to_owned().into()
    };

    let body_px = f32::from(window.text_style().font_size.to_pixels(window.rem_size())).max(12.0);
    let key_px = (body_px * 0.82).max(11.0);
    let val_px = (body_px * 0.88).max(11.5);
    let pill_px = (body_px * 0.78).max(10.5);
    let icon_px = (body_px * 0.80).max(11.0);
    let check_px = (body_px * 0.88).max(13.0);

    let entries = parse_entries(yaml);
    let max_key_chars = entries
        .iter()
        .map(|e| e.key.chars().count())
        .max()
        .unwrap_or(6);
    let key_col_w = ((max_key_chars as f32) * key_px * 0.56 + body_px * 2.1)
        .clamp(body_px * 6.5, body_px * 12.5);

    let mut rows = div()
        .group("fm-block")
        .w_full()
        .pb(px(body_px * 0.4))
        .mb(px(body_px * 0.15))
        .flex()
        .flex_col()
        .gap(px(2.));

    let yaml_owned = yaml.to_owned();
    let esc_state = inline_state.clone();
    rows = rows.on_key_down(move |ev: &KeyDownEvent, _, cx| {
        if ev.keystroke.key == "escape" {
            esc_state.update(cx, |s, cx| {
                s.type_menu_key = None;
                s.cancel(cx);
            });
            cx.stop_propagation();
        }
    });

    for (ix, entry) in entries.into_iter().enumerate() {
        let key_name = entry.key.clone();
        let is_tags_field = key_name.eq_ignore_ascii_case("tags");
        let key_icon = match &entry.value {
            FrontMatterValue::Text(_) => IconName::TextAlignStart,
            FrontMatterValue::Bool(_) => IconName::CircleDot,
            FrontMatterValue::List(_) => IconName::List,
            FrontMatterValue::Raw(_) => IconName::Code,
        };

        let is_editing_key = editing_target == Some(InlineTarget::Key(key_name.clone()));
        let is_editing_text = editing_target == Some(InlineTarget::TextValue(key_name.clone()));
        let is_adding_item = editing_target == Some(InlineTarget::AddListItem(key_name.clone()));
        let show_type_menu = type_menu_key.as_deref() == Some(key_name.as_str());

        let value_el: AnyElement = match entry.value {
            FrontMatterValue::Text(text) => {
                if is_editing_text {
                    div()
                        .flex_1()
                        .min_w_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Input::new(&input_entity)
                                .appearance(false)
                                .bordered(false)
                                .px_0()
                                .py_0()
                                .h(px(val_px * 1.5))
                                .text_size(px(val_px))
                                .text_color(text_color),
                        )
                        .into_any_element()
                } else {
                    let click_state = inline_state.clone();
                    let click_key = key_name.clone();
                    let click_text = text.clone();
                    div()
                        .id(("fm-val", (block.as_u64() as usize).wrapping_mul(31).wrapping_add(ix)))
                        .flex_1()
                        .min_w_0()
                        .min_h(px(val_px * 1.45))
                        .cursor_text()
                        .text_size(px(val_px))
                        .line_height(relative(1.5))
                        .text_color(text_color)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            click_state.update(cx, |s, cx| {
                                s.start(
                                    InlineTarget::TextValue(click_key.clone()),
                                    click_text.clone(),
                                    "",
                                    window,
                                    cx,
                                );
                            });
                        })
                        .child(text)
                        .into_any_element()
                }
            }
            FrontMatterValue::Bool(val) => {
                let toggle_editor = editor.clone();
                let toggle_yaml = yaml_owned.clone();
                let toggle_key = key_name.clone();
                let next_val = !val;
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .pt(px(2.))
                    .child(
                        div()
                            .id(("fm-bool", (block.as_u64() as usize).wrapping_add(ix + 1)))
                            .size(px(check_px))
                            .rounded(px(3.5))
                            .border_1()
                            .border_color(if val { accent } else { muted.opacity(0.45) })
                            .bg(if val { accent } else { accent.opacity(0.0) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let next_yaml =
                                    toggle_bool_in_yaml(&toggle_yaml, &toggle_key, next_val);
                                let _ = toggle_editor.update(cx, |editor, cx| {
                                    editor.replace_plain_block_text(block, next_yaml, cx);
                                });
                            })
                            .children(val.then(|| {
                                crate::ui::icon(IconName::Check)
                                    .size(px(check_px * 0.72))
                                    .text_color(gpui_kit::white())
                            })),
                    )
                    .into_any_element()
            }
            FrontMatterValue::List(items) => {
                let mut list_row = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.));

                for (item_ix, item) in items.iter().enumerate() {
                    let tag_palette = if is_tags_field {
                        tag_colors.as_ref().and_then(|lookup| lookup(item, cx))
                    } else {
                        None
                    };
                    let (item_bg, item_fg) = if let Some((bg, fg)) = tag_palette {
                        let bg_col = gpui_kit::rgb(bg).opacity(if dark { 0.22 } else { 0.72 });
                        let fg_col = gpui_kit::rgb(if dark { bg } else { fg });
                        (bg_col, fg_col)
                    } else {
                        (subtle_tint, text_color.opacity(0.9))
                    };

                    let remove_editor = editor.clone();
                    let remove_yaml = yaml_owned.clone();
                    let remove_key = key_name.clone();
                    let remaining: Vec<String> = items
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != item_ix)
                        .map(|(_, v)| v.clone())
                        .collect();
                    let pill_group = SharedString::from(format!("fm-pill-{ix}-{item_ix}"));

                    list_row = list_row.child(
                        div()
                            .group(pill_group.clone())
                            .relative()
                            .h(px(pill_px * 1.75))
                            .px(px(pill_px * 0.85))
                            .rounded_full()
                            .bg(item_bg)
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .text_size(px(pill_px))
                            .text_color(item_fg)
                            .children(is_tags_field.then(|| {
                                div()
                                    .text_color(item_fg.opacity(0.5))
                                    .child("#")
                            }))
                            .child(item.clone())
                            .child(
                                div()
                                    .id((
                                        "fm-pill-rm",
                                        (block.as_u64() as usize)
                                            .wrapping_mul(131)
                                            .wrapping_add(ix * 37 + item_ix),
                                    ))
                                    .absolute()
                                    .right(px(2.))
                                    .top(px((pill_px * 1.75 - pill_px * 1.2) * 0.5))
                                    .size(px(pill_px * 1.2))
                                    .rounded_full()
                                    .bg(item_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .opacity(0.)
                                    .group_hover(pill_group, |s| s.opacity(0.95))
                                    .hover(|s| s.opacity(1.0))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        let next_yaml =
                                            set_list_in_yaml(&remove_yaml, &remove_key, &remaining);
                                        let _ = remove_editor.update(cx, |ed, cx| {
                                            ed.replace_plain_block_text(block, next_yaml, cx);
                                        });
                                    })
                                    .child(crate::ui::icon(IconName::Close).size(px(pill_px * 0.78))),
                            ),
                    );
                }

                if is_adding_item {
                    list_row = list_row.child(
                        div()
                            .w(px(body_px * 6.5))
                            .h(px(pill_px * 1.65))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Input::new(&input_entity)
                                    .appearance(false)
                                    .bordered(false)
                                    .px_0()
                                    .py_0()
                                    .h(px(pill_px * 1.45))
                                    .text_size(px(pill_px))
                                    .text_color(text_color),
                            ),
                    );
                } else {
                    let add_state = inline_state.clone();
                    let add_key = key_name.clone();
                    list_row = list_row.child(
                        div()
                            .id(("fm-list-add", (block.as_u64() as usize).wrapping_mul(17).wrapping_add(ix)))
                            .size(px(pill_px * 1.6))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(muted.opacity(0.6))
                            .opacity(if items.is_empty() { 0.8 } else { 0.0 })
                            .group_hover("fm-block", |s| s.opacity(0.85))
                            .hover(move |s| s.bg(row_hover).text_color(text_color))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                add_state.update(cx, |s, cx| {
                                    s.start(
                                        InlineTarget::AddListItem(add_key.clone()),
                                        String::new(),
                                        crate::i18n::markdown::FM_ADD_ITEM_PLACEHOLDER.get(),
                                        window,
                                        cx,
                                    );
                                });
                            })
                            .child(crate::ui::icon(IconName::Plus).size(px(pill_px * 0.9))),
                    );
                }
                list_row.into_any_element()
            }
            FrontMatterValue::Raw(raw) => div()
                .flex_1()
                .min_w_0()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(4.))
                .bg(subtle_tint)
                .font_family(mono_font.clone())
                .text_size(px(pill_px))
                .line_height(relative(1.45))
                .text_color(text_color.opacity(0.9))
                .child(raw)
                .into_any_element(),
        };

        let icon_state = inline_state.clone();
        let icon_key = key_name.clone();
        let key_el: AnyElement = if is_editing_key {
            div()
                .flex_1()
                .min_w_0()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Input::new(&input_entity)
                        .appearance(false)
                        .bordered(false)
                        .px_0()
                        .py_0()
                        .h(px(key_px * 1.45))
                        .text_size(px(key_px))
                        .text_color(text_color),
                )
                .into_any_element()
        } else {
            let rename_state = inline_state.clone();
            let rename_key = key_name.clone();
            div()
                .id(("fm-key", (block.as_u64() as usize).wrapping_mul(29).wrapping_add(ix)))
                .flex_1()
                .min_w_0()
                .cursor_text()
                .text_size(px(key_px))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    rename_state.update(cx, |s, cx| {
                        s.start(
                            InlineTarget::Key(rename_key.clone()),
                            rename_key.clone(),
                            crate::i18n::markdown::FM_KEY_PLACEHOLDER.get(),
                            window,
                            cx,
                        );
                    });
                })
                .child(key_name.clone())
                .into_any_element()
        };

        let mut row_wrapper = div().w_full().flex().flex_col();
        row_wrapper = row_wrapper.child(
            div()
                .w_full()
                .px(px(4.))
                .py(px(body_px * 0.22))
                .rounded(px(5.))
                .hover(move |s| s.bg(row_hover))
                .flex()
                .items_start()
                .gap(px(body_px * 0.75))
                .child(
                    div()
                        .w(px(key_col_w))
                        .flex_shrink_0()
                        .pt(px(body_px * 0.12))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_color(muted)
                        .child(
                            div()
                                .id(("fm-type-btn", (block.as_u64() as usize).wrapping_mul(19).wrapping_add(ix)))
                                .size(px(icon_px * 1.4))
                                .rounded(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(move |s| s.bg(row_hover).text_color(text_color))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    icon_state.update(cx, |s, cx| {
                                        s.editing = None;
                                        if s.type_menu_key.as_deref() == Some(icon_key.as_str()) {
                                            s.type_menu_key = None;
                                        } else {
                                            s.type_menu_key = Some(icon_key.clone());
                                        }
                                        cx.notify();
                                    });
                                })
                                .child(
                                    crate::ui::icon(key_icon)
                                        .size(px(icon_px))
                                        .text_color(muted.opacity(0.6)),
                                ),
                        )
                        .child(key_el),
                )
                .child(value_el),
        );

        if show_type_menu {
            let menu_btn = |id_tag: &'static str,
                            label: &'static str,
                            icon: IconName,
                            danger: bool,
                            on_pick: Box<dyn Fn(&mut gpui_kit::App) + 'static>| {
                div()
                    .id((id_tag, (block.as_u64() as usize).wrapping_mul(43).wrapping_add(ix)))
                    .h(px(22.))
                    .px(px(7.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .text_size(px(pill_px))
                    .text_color(if danger {
                        gpui_kit::rgb(0xcf222e)
                    } else {
                        muted
                    })
                    .hover(move |s| {
                        s.bg(row_hover).text_color(if danger {
                            gpui_kit::rgb(0xcf222e)
                        } else {
                            text_color
                        })
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        on_pick(cx);
                    })
                    .child(crate::ui::icon(icon).size(px(pill_px)))
                    .child(label)
            };

            let make_convert = |target: FieldType| -> Box<dyn Fn(&mut gpui_kit::App)> {
                let ed = editor.clone();
                let st = inline_state.clone();
                let y = yaml_owned.clone();
                let k = key_name.clone();
                Box::new(move |cx| {
                    st.update(cx, |s, cx| {
                        s.type_menu_key = None;
                        cx.notify();
                    });
                    let next_yaml = convert_field_type_in_yaml(&y, &k, target);
                    let _ = ed.update(cx, |ed, cx| {
                        ed.replace_plain_block_text(block, next_yaml, cx);
                    });
                })
            };
            let del_ed = editor.clone();
            let del_st = inline_state.clone();
            let del_y = yaml_owned.clone();
            let del_k = key_name.clone();
            let on_delete: Box<dyn Fn(&mut gpui_kit::App)> = Box::new(move |cx| {
                del_st.update(cx, |s, cx| {
                    s.type_menu_key = None;
                    cx.notify();
                });
                let next_yaml = remove_key_in_yaml(&del_y, &del_k);
                let _ = del_ed.update(cx, |ed, cx| {
                    ed.replace_plain_block_text(block, next_yaml, cx);
                });
            });

            row_wrapper = row_wrapper.child(
                div()
                    .pl(px(24.))
                    .py(px(3.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(menu_btn(
                        "fm-to-text",
                        crate::i18n::markdown::FM_TYPE_TEXT.get(),
                        IconName::TextAlignStart,
                        false,
                        make_convert(FieldType::Text),
                    ))
                    .child(menu_btn(
                        "fm-to-list",
                        crate::i18n::markdown::FM_TYPE_LIST.get(),
                        IconName::List,
                        false,
                        make_convert(FieldType::List),
                    ))
                    .child(menu_btn(
                        "fm-to-bool",
                        crate::i18n::markdown::FM_TYPE_BOOL.get(),
                        IconName::CircleDot,
                        false,
                        make_convert(FieldType::Bool),
                    ))
                    .child(menu_btn(
                        "fm-del-row",
                        crate::i18n::markdown::FM_DELETE_ROW.get(),
                        IconName::Trash,
                        true,
                        on_delete,
                    )),
            );
        }

        rows = rows.child(row_wrapper);
    }

    // Bottom "+ Add property" trigger (revealed subtly on hover, or inline key input when active)
    if editing_target == Some(InlineTarget::NewRowKey) {
        rows = rows.child(
            div()
                .w_full()
                .px(px(4.))
                .py(px(body_px * 0.22))
                .flex()
                .items_center()
                .gap(px(5.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    crate::ui::icon(IconName::Plus)
                        .size(px(icon_px))
                        .text_color(muted.opacity(0.6)),
                )
                .child(
                    div().w(px(key_col_w)).child(
                        Input::new(&input_entity)
                            .appearance(false)
                            .bordered(false)
                            .px_0()
                            .py_0()
                            .h(px(key_px * 1.45))
                            .text_size(px(key_px))
                            .text_color(text_color),
                    ),
                ),
        );
    } else {
        let new_row_state = inline_state.clone();
        rows = rows.child(
            div()
                .id(("fm-add-prop", block.as_u64() as usize))
                .px(px(4.))
                .py(px(2.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap(px(5.))
                .cursor_pointer()
                .text_size(px(pill_px))
                .text_color(muted.opacity(0.65))
                .opacity(0.)
                .group_hover("fm-block", |s| s.opacity(0.85))
                .hover(move |s| s.bg(row_hover).text_color(text_color))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    new_row_state.update(cx, |s, cx| {
                        s.start(
                            InlineTarget::NewRowKey,
                            String::new(),
                            crate::i18n::markdown::FM_KEY_PLACEHOLDER.get(),
                            window,
                            cx,
                        );
                    });
                })
                .child(crate::ui::icon(IconName::Plus).size(px(pill_px)))
                .child(crate::i18n::markdown::FM_ADD_PROPERTY.get()),
        );
    }

    rows.into_any_element()
}

/// Extract the `tags` field from a Markdown document's leading YAML Front Matter.
///
/// Returns `None` if the document has no Front Matter or no `tags` key.
/// Returns `Some(vec)` (possibly empty for `tags: []`) when a `tags` key is present.
pub(crate) fn extract_tags(source: &str) -> Option<Vec<String>> {
    let fm = split(source).0?;
    let entry = fm
        .entries
        .into_iter()
        .find(|entry| entry.key.eq_ignore_ascii_case("tags"))?;
    let raw_items = match entry.value {
        FrontMatterValue::List(items) => items,
        FrontMatterValue::Text(text) => {
            if text.trim().is_empty() {
                Vec::new()
            } else {
                text.split(',')
                    .map(|part| part.trim().to_owned())
                    .filter(|part| !part.is_empty())
                    .collect()
            }
        }
        FrontMatterValue::Bool(_) | FrontMatterValue::Raw(_) => return None,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for item in raw_items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            result.push(trimmed.to_owned());
        }
    }
    Some(result)
}

fn format_yaml_tag_item(tag: &str) -> String {
    let needs_quote = tag.is_empty()
        || tag.starts_with([' ', '\t', '-', '?', ':', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', '[', '{'])
        || tag.ends_with([' ', '\t'])
        || tag.contains([',', '[', ']', '{', '}', '#'])
        || tag.contains(": ")
        || matches!(tag.to_ascii_lowercase().as_str(), "true" | "false" | "null" | "~");
    if needs_quote {
        let escaped = tag.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        tag.to_owned()
    }
}

/// Update the `tags` field inside an existing YAML Front Matter block (conservative mode).
///
/// Returns `None` if:
/// - the document has no Front Matter (never injects `---` into plain notes), or
/// - the normalized tag set already matches `desired_tags` (prevents sync loops).
pub(crate) fn update_tags_in_markdown(source: &str, desired_tags: &[String]) -> Option<String> {
    let (fm, body) = split(source);
    let fm = fm?;

    let mut desired_clean = Vec::new();
    let mut desired_keys = std::collections::BTreeSet::new();
    for tag in desired_tags {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if desired_keys.insert(key) {
            desired_clean.push(trimmed.to_owned());
        }
    }

    let existing_keys: std::collections::BTreeSet<String> = extract_tags(source)
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .collect();
    if existing_keys == desired_keys {
        return None;
    }

    let lines: Vec<&str> = fm.yaml_body.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + desired_clean.len() + 1);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let line = lines[i];
        if !replaced
            && !line.starts_with([' ', '\t'])
            && let Some((key, val)) = split_key_value(line)
            && key.eq_ignore_ascii_case("tags")
        {
            replaced = true;
            let key_prefix = line.find(':').map_or("tags", |idx| &line[..idx]);
            let mut block_style = false;
            i += 1;
            if val.is_empty() {
                while i < lines.len() {
                    let next = lines[i];
                    if next.trim().is_empty() {
                        i += 1;
                        continue;
                    }
                    let starts_indented = next.starts_with([' ', '\t']);
                    let starts_dash = next.starts_with("- ") || next.trim() == "-";
                    if starts_indented || starts_dash {
                        block_style = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            if desired_clean.is_empty() {
                out.push(format!("{key_prefix}: []"));
            } else if block_style {
                out.push(format!("{key_prefix}:"));
                for tag in &desired_clean {
                    out.push(format!("  - {}", format_yaml_tag_item(tag)));
                }
            } else {
                let items: Vec<String> = desired_clean
                    .iter()
                    .map(|tag| format_yaml_tag_item(tag))
                    .collect();
                out.push(format!("{key_prefix}: [{}]", items.join(", ")));
            }
            continue;
        }
        out.push(line.to_owned());
        i += 1;
    }

    if !replaced {
        if desired_clean.is_empty() {
            return None;
        }
        let items: Vec<String> = desired_clean
            .iter()
            .map(|tag| format_yaml_tag_item(tag))
            .collect();
        out.push(format!("tags: [{}]", items.join(", ")));
    }

    let updated = fm.with_yaml_body(&out.join("\n"));
    Some(updated.join_body(body))
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

    #[test]
    fn bidirectional_yaml_tags_conservative_sync() {
        // Plain note without frontmatter is never mutated in conservative mode.
        let plain = "# Just a note\n\nBody\n";
        assert_eq!(extract_tags(plain), None);
        assert_eq!(update_tags_in_markdown(plain, &["Rust".into()]), None);

        // Flow-style tags update in place and no-op when set matches.
        let flow = "---\nname: demo\ntags: [Rust, GPUI]\n---\n\n# Body\n";
        assert_eq!(extract_tags(flow), Some(vec!["Rust".into(), "GPUI".into()]));
        assert_eq!(
            update_tags_in_markdown(flow, &["gpui".into(), "rust".into()]),
            None
        );
        let updated = update_tags_in_markdown(flow, &["Rust".into(), "Markdown".into()]).unwrap();
        assert_eq!(
            updated,
            "---\nname: demo\ntags: [Rust, Markdown]\n---\n\n# Body\n"
        );

        // Block-style tags preserve block style.
        let block = "---\nname: demo\ntags:\n  - Alice\n---\n\nBody";
        let updated = update_tags_in_markdown(block, &["Alice".into(), "Bob".into()]).unwrap();
        assert_eq!(
            updated,
            "---\nname: demo\ntags:\n  - Alice\n  - Bob\n---\n\nBody"
        );

        // Existing frontmatter without tags key appends tags when non-empty.
        let no_tags = "---\nname: demo\n---\n\nBody";
        let updated = update_tags_in_markdown(no_tags, &["Note".into()]).unwrap();
        assert_eq!(updated, "---\nname: demo\ntags: [Note]\n---\n\nBody");
    }

    #[test]
    fn inline_yaml_row_mutations_preserve_other_fields() {
        let yaml = "name: folio\npublished: false\ntags: [Rust, GPUI]";
        assert_eq!(
            set_text_value_in_yaml(yaml, "name", "folio-next"),
            "name: folio-next\npublished: false\ntags: [Rust, GPUI]"
        );
        assert_eq!(
            rename_key_in_yaml(yaml, "name", "title"),
            "title: folio\npublished: false\ntags: [Rust, GPUI]"
        );
        assert_eq!(
            convert_field_type_in_yaml(yaml, "published", FieldType::Text),
            "name: folio\npublished: \"false\"\ntags: [Rust, GPUI]"
        );
        assert_eq!(
            remove_key_in_yaml(yaml, "published"),
            "name: folio\ntags: [Rust, GPUI]"
        );
        assert_eq!(
            append_key_in_yaml(yaml, "aliases", true),
            "name: folio\npublished: false\ntags: [Rust, GPUI]\naliases: []"
        );
    }
}
