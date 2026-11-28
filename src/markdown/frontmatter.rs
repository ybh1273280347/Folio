//! Lossless YAML Front Matter extraction, display parsing, and reassembly.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui_kit::SharedString;
use markdown_editor::split_frontmatter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrontMatterValue {
    Text(String),
    Bool(bool),
    List(Arc<[String]>),
    Raw(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrontMatterEntry {
    pub key: String,
    pub value: FrontMatterValue,
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    if line.starts_with([' ', '\t', '#', '-']) {
        return None;
    }
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (idx, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_double => escaped = true,
            b'\'' if in_single && bytes.get(idx + 1) == Some(&b'\'') => escaped = true,
            b'\'' if in_single || idx == 0 => in_single = !in_single,
            b'"' if in_double || idx == 0 => in_double = !in_double,
            b':' if !in_single && !in_double => {
                let next = bytes.get(idx + 1).copied();
                if next.is_none() || matches!(next, Some(b' ' | b'\t')) {
                    let key = &line[..idx];
                    if !key.trim().is_empty() {
                        return Some((key, &line[idx + 1..]));
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

fn scalar_text(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with('"') && let Ok(value) = serde_json::from_str::<String>(raw) {
        return value;
    }
    if raw.starts_with('\'') && raw.ends_with('\'') {
        return unquote(raw).replace("''", "'");
    }
    unquote(raw).to_owned()
}

/// Split a trailing comment, keeping its preceding whitespace for source edits.
fn split_yaml_comment(raw: &str) -> (&str, &str) {
    let mut quote = None;
    let mut escaped = false;
    let mut previous = b' ';
    for (index, byte) in raw.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            } else if delimiter == b'"' && byte == b'\\' {
                escaped = true;
            }
        } else if byte == b'#' && previous.is_ascii_whitespace() {
            let start = raw[..index].trim_end_matches([' ', '\t']).len();
            return (raw[..start].trim(), &raw[start..]);
        } else if matches!(byte, b'\'' | b'"')
            && (previous.is_ascii_whitespace() || matches!(previous, b'[' | b','))
        {
            quote = Some(byte);
        }
        previous = byte;
    }
    (raw.trim(), "")
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
    let mut escaped = false;
    for (idx, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_double => escaped = true,
            '\'' if in_single && inner[idx + 1..].starts_with('\'') => escaped = true,
            '\'' if !in_double && (in_single || inner[start..idx].trim().is_empty()) => {
                in_single = !in_single;
            }
            '"' if !in_single && (in_double || inner[start..idx].trim().is_empty()) => {
                in_double = !in_double;
            }
            '[' | ']' | '{' | '}' if !in_single && !in_double => return None,
            ',' if !in_single && !in_double => {
                if !is_simple_list_item(inner[start..idx].trim()) {
                    return None;
                }
                let part = scalar_text(&inner[start..idx]);
                if !part.trim().is_empty() {
                    items.push(part);
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    if in_single || in_double || !is_simple_list_item(inner[start..].trim()) {
        return None;
    }
    let tail = scalar_text(&inner[start..]);
    if !tail.trim().is_empty() {
        items.push(tail);
    }
    Some(items)
}

fn is_simple_list_item(item: &str) -> bool {
    !item.starts_with(['[', '{', '*', '&', '!', '|', '>'])
        && split_key_value(item).is_none()
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
        let key = scalar_text(key);
        let (val, _) = split_yaml_comment(val);
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
                if next.trim().is_empty() || next.trim_start().starts_with('#') {
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
            } else if non_empty.iter().all(|line| {
                *line == "-"
                    || line.strip_prefix("- ").is_some_and(|item| {
                        is_simple_list_item(split_yaml_comment(item).0)
                    })
            })
            {
                let items: Vec<_> = non_empty
                    .into_iter()
                    .filter_map(|l| {
                        let item = scalar_text(split_yaml_comment(l.strip_prefix("- ").unwrap_or("")).0);
                        (!item.trim().is_empty()).then_some(item)
                    })
                    .collect();
                entries.push(FrontMatterEntry {
                    key,
                    value: FrontMatterValue::List(items.into()),
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
                value: FrontMatterValue::List(items.into()),
            });
            continue;
        }
        if val.starts_with(['[', '{', '*', '&', '!']) {
            entries.push(FrontMatterEntry {
                key,
                value: FrontMatterValue::Raw(val.to_owned()),
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
        let mut text = scalar_text(val);
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
    let mut offset = 0;
    for line in yaml.split_inclusive('\n') {
        if let Some((key, raw)) = split_key_value(line.trim_end_matches(['\r', '\n']))
            && scalar_text(key) == target_key
        {
            let (value, _) = split_yaml_comment(raw);
            if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
                let start = offset + key.len() + 1 + raw.len() - raw.trim_start().len();
                let mut updated = yaml.to_owned();
                updated.replace_range(start..start + value.len(), replacement);
                return updated;
            }
            break;
        }
        offset += line.len();
    }
    yaml.to_owned()
}

pub(crate) type TagColorLookup =
    Arc<dyn Fn(&str, &gpui_kit::App) -> Option<(u32, u32)> + Send + Sync>;

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
        if let Some((key, raw)) = split_key_value(line)
            && scalar_text(key) == target_key
        {
            let (val, _) = split_yaml_comment(raw);
            let start = i;
            let mut end = i + 1;
            let mut is_block_list = false;
            if val.is_empty() || matches!(val, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
                while end < lines.len() {
                    let next = lines[end];
                    if next.trim().is_empty()
                        || (val.is_empty() && next.trim_start().starts_with('#'))
                    {
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
                while end > start + 1
                    && (lines[end - 1].trim().is_empty()
                        || (val.is_empty() && lines[end - 1].trim_start().starts_with('#')))
                {
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

fn yaml_line_ending(yaml: &str) -> &'static str {
    if yaml
        .split_inclusive('\n')
        .next()
        .is_some_and(|line| line.ends_with("\r\n"))
    {
        "\r\n"
    } else {
        "\n"
    }
}

/// Splice only the edited field. Existing CRLF, blank lines and indentation
/// outside the selected line range remain byte-for-byte intact.
fn replace_yaml_lines(yaml: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut lines = yaml.split_inclusive('\n');
    let from: usize = lines.by_ref().take(start).map(str::len).sum();
    let to = from + lines.take(end - start).map(str::len).sum::<usize>();
    let removed = &yaml[from..to];
    let mut result = yaml.to_owned();
    if replacement.is_empty() {
        // Deleting the last field should not leave an extra empty line before the fence.
        let from = if to == yaml.len() && !removed.ends_with('\n') {
            let previous = &yaml[..from];
            previous
                .strip_suffix("\r\n")
                .or_else(|| previous.strip_suffix('\n'))
                .unwrap_or(previous)
                .len()
        } else {
            from
        };
        result.replace_range(from..to, "");
    } else {
        let removed = removed
            .strip_suffix("\r\n")
            .or_else(|| removed.strip_suffix('\n'))
            .unwrap_or(removed);
        result.replace_range(from..from + removed.len(), replacement);
    }
    result
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
        || trimmed.contains(" #")
        || trimmed.chars().any(char::is_control);
    if needs_quote {
        serde_json::to_string(trimmed).expect("string can be JSON-quoted")
    } else {
        trimmed.to_owned()
    }
}

pub(crate) fn set_text_value_in_yaml(yaml: &str, target_key: &str, new_val: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, end, _)) = find_key_line_span(&lines, target_key) else {
        return yaml.to_owned();
    };
    let (key_prefix, raw) = split_key_value(lines[start]).expect("field span begins at its key");
    let (_, comment) = split_yaml_comment(raw);
    let formatted = format_yaml_scalar(new_val);
    let replacement = if formatted.is_empty() {
        format!("{key_prefix}:{comment}")
    } else {
        format!("{key_prefix}: {formatted}{comment}")
    };
    replace_yaml_lines(yaml, start, end, &replacement)
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
    if find_key_line_span(&lines, new_key).is_some() {
        return yaml.to_owned();
    }
    let (key_prefix, raw) = split_key_value(lines[start]).expect("field span begins at its key");
    let spacing = &key_prefix[key_prefix.trim_end().len()..];
    let new_key = format_yaml_tag_item(new_key);
    replace_yaml_lines(yaml, start, start + 1, &format!("{new_key}{spacing}:{raw}"))
}

pub(crate) fn remove_key_in_yaml(yaml: &str, target_key: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, end, _)) = find_key_line_span(&lines, target_key) else {
        return yaml.to_owned();
    };
    replace_yaml_lines(yaml, start, end, "")
}

pub(crate) fn set_list_in_yaml(yaml: &str, target_key: &str, items: &[String]) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some((start, end, block_style)) = find_key_line_span(&lines, target_key) else {
        return yaml.to_owned();
    };
    let (key_prefix, raw) = split_key_value(lines[start]).expect("field span begins at its key");
    let (_, comment) = split_yaml_comment(raw);
    let newline = yaml_line_ending(yaml);
    let mut out = Vec::with_capacity(items.len() + 1);
    if items.is_empty() {
        out.push(format!("{key_prefix}: []{comment}"));
    } else if block_style {
        out.push(format!("{key_prefix}:{comment}"));
        for item in items {
            out.push(format!("  - {}", format_yaml_tag_item(item)));
        }
    } else {
        let formatted: Vec<String> = items.iter().map(|s| format_yaml_tag_item(s)).collect();
        out.push(format!("{key_prefix}: [{}]{comment}", formatted.join(", ")));
    }
    if block_style {
        // Keep standalone comments and any comments attached to replaced items.
        for line in &lines[start + 1..end] {
            let (value, comment) = split_yaml_comment(line);
            if value.is_empty() {
                out.push((*line).to_owned());
            } else if !comment.is_empty() {
                let indent = line.len() - line.trim_start().len();
                out.push(format!("{}{}", &line[..indent], comment.trim_start()));
            }
        }
    }
    replace_yaml_lines(yaml, start, end, &out.join(newline))
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
                Some(FrontMatterValue::List(items)) => items.to_vec(),
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
            let (key_prefix, raw) = split_key_value(lines[start]).expect("field span begins at its key");
            let (_, comment) = split_yaml_comment(raw);
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
            replace_yaml_lines(yaml, start, end, &format!("{replacement}{comment}"))
        }
    }
}

pub(crate) fn append_key_in_yaml(yaml: &str, key: &str, as_list: bool) -> String {
    let key = key.trim();
    if key.is_empty() || find_key_line_span(&yaml.lines().collect::<Vec<_>>(), key).is_some() {
        return yaml.to_owned();
    }
    let key = format_yaml_tag_item(key);
    let line = if as_list {
        format!("{key}: []")
    } else {
        format!("{key}: ")
    };
    if yaml.is_empty() {
        line
    } else if yaml.ends_with('\n') {
        format!("{yaml}{line}")
    } else {
        format!("{yaml}{}{line}", yaml_line_ending(yaml))
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
    yaml_snapshot: SharedString,
    parsed_yaml: Option<(SharedString, Arc<[FrontMatterEntry]>)>,
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
            yaml_snapshot: SharedString::default(),
            parsed_yaml: None,
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
                if next_yaml.as_str() != yaml.as_ref() {
                    self.yaml_snapshot = next_yaml.clone().into();
                    let block = self.block;
                    let _ = self.editor.update(cx, |ed, cx| {
                        ed.replace_plain_block_text(block, next_yaml, cx);
                    });
                }
            }
            InlineTarget::TextValue(key) => {
                let next_yaml = set_text_value_in_yaml(&yaml, &key, &raw);
                if next_yaml.as_str() != yaml.as_ref() {
                    self.yaml_snapshot = next_yaml.clone().into();
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
                            FrontMatterValue::List(v) => Some(v.to_vec()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    for part in trimmed.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        if !items.iter().any(|existing| existing.eq_ignore_ascii_case(part)) {
                            items.push(part.to_owned());
                        }
                    }
                    let next_yaml = set_list_in_yaml(&yaml, &key, &items);
                    self.yaml_snapshot = next_yaml.clone().into();
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
                    self.yaml_snapshot = next_yaml.clone().into();
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
    yaml: SharedString,
    editor: gpui_kit::WeakEntity<markdown_editor::BlockEditorState>,
    editor_font: &str,
    tag_colors: Option<TagColorLookup>,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    use gpui_kit::component::input::Input;
    use gpui_kit::{
        AnyElement, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
        StatefulInteractiveElement, Styled, div, px, relative,
    };
    use gpui_kit::assets::IconName;

    let inline_state = window.use_keyed_state(
        ("fm-inline-state", block.as_u64() as usize),
        cx,
        |window, cx| FrontMatterInlineState::new(block, editor.clone(), window, cx),
    );
    let entries = inline_state.update(cx, |state, _| {
        state.yaml_snapshot = yaml.clone();
        // Layout and hover changes reuse the same parsed property values.
        if let Some((source, entries)) = &state.parsed_yaml && *source == yaml {
            return entries.clone();
        }
        let entries: Arc<[FrontMatterEntry]> = parse_entries(&yaml).into();
        state.parsed_yaml = Some((yaml.clone(), entries.clone()));
        entries
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

    for (ix, entry) in entries.iter().enumerate() {
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

        let value_el: AnyElement = match &entry.value {
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
                        .child(text.clone())
                        .into_any_element()
                }
            }
            FrontMatterValue::Bool(val) => {
                let val = *val;
                let toggle_editor = editor.clone();
                let toggle_yaml = yaml.clone();
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
                    let remove_yaml = yaml.clone();
                    let remove_key = key_name.clone();
                    let remove_items = items.clone();
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
                                    .id(SharedString::from(format!(
                                        "fm-pill-rm-{}-{ix}-{item_ix}",
                                        block.as_u64(),
                                    )))
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
                                        // Copy the list only for the selected removal.
                                        let mut remaining = remove_items.to_vec();
                                        remaining.remove(item_ix);
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
                .child(raw.clone())
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
                let y = yaml.clone();
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
            let del_y = yaml.clone();
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
    let header = split_frontmatter(source)?;
    let entries = parse_entries(header.yaml);
    let mut matching = entries.iter().filter(|entry| entry.key.eq_ignore_ascii_case("tags"));
    let value = &matching.next()?.value;
    if matching.next().is_some() {
        return None;
    }
    tags_from_value(value)
}

fn tags_from_value(value: &FrontMatterValue) -> Option<Vec<String>> {
    let raw_items: Vec<&str> = match value {
        FrontMatterValue::List(items) => items.iter().map(String::as_str).collect(),
        FrontMatterValue::Text(text) => text.split(',').collect(),
        FrontMatterValue::Bool(_) | FrontMatterValue::Raw(_) => return None,
    };
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for item in raw_items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = crate::tags::name_key(trimmed);
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
        || tag.contains([',', '[', ']', '{', '}', '#', '\'', '"'])
        || tag.chars().any(char::is_control)
        || tag.contains(": ")
        || matches!(tag.to_ascii_lowercase().as_str(), "true" | "false" | "null" | "~");
    if needs_quote {
        serde_json::to_string(tag).expect("string can be JSON-quoted")
    } else {
        tag.to_owned()
    }
}

/// Apply a tag operation to an existing YAML header, keeping other draft tags.
pub(crate) fn update_tags_in_markdown(
    source: &str,
    before: &[String],
    after: &[String],
) -> Option<String> {
    let header = split_frontmatter(source)?;
    let entries = parse_entries(header.yaml);
    let mut matching = entries.iter().filter(|entry| entry.key.eq_ignore_ascii_case("tags"));
    let existing = matching.next();
    // Replacing an ambiguous or unsupported YAML value can discard user data.
    if matching.next().is_some() {
        return None;
    }
    let mut desired = match existing {
        Some(entry) => tags_from_value(&entry.value)?,
        None => Vec::new(),
    };
    let existing_keys: BTreeSet<_> =
        desired.iter().map(|name| crate::tags::name_key(name)).collect();
    let before_keys: BTreeSet<_> =
        before.iter().map(|name| crate::tags::name_key(name)).collect();
    let after_keys: BTreeSet<_> =
        after.iter().map(|name| crate::tags::name_key(name)).collect();
    // Drafts and on-disk edits can contain tags the database has not seen.
    desired.retain(|tag| {
        let key = crate::tags::name_key(tag);
        !before_keys.contains(&key) || after_keys.contains(&key)
    });
    let mut desired_keys: BTreeSet<_> =
        desired.iter().map(|name| crate::tags::name_key(name)).collect();
    desired.extend(
        after
            .iter()
            .filter(|name| {
                let key = crate::tags::name_key(name);
                (existing.is_none() || !before_keys.contains(&key)) && desired_keys.insert(key)
            })
            .cloned(),
    );
    if existing_keys == desired_keys {
        return None;
    }
    let newline = yaml_line_ending(header.opening);
    let yaml = if let Some(entry) = existing {
        set_list_in_yaml(header.yaml, &entry.key, &desired)
    } else {
        let items: Vec<_> = desired.iter().map(|tag| format_yaml_tag_item(tag)).collect();
        let line = format!("tags: [{}]", items.join(", "));
        if header.yaml.is_empty() {
            line
        } else {
            format!("{}{newline}{line}", header.yaml)
        }
    };
    let ending = if header.yaml_ending.is_empty() { newline } else { header.yaml_ending };
    Some(format!("{}{yaml}{ending}{}{}", header.opening, header.closing, header.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_skill_frontmatter_and_parses_properties() {
        let src = "---\nname: folio-intro\ndescription: 面向普通用户介绍 Folio 是什么。\ndisable-model-invocation: false\n---\n\n# 介绍 Folio\n\n正文\n";
        let header = split_frontmatter(src).expect("frontmatter detected");
        let entries = parse_entries(header.yaml);
        assert_eq!(header.body, "# 介绍 Folio\n\n正文\n");
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            FrontMatterEntry {
                key: "name".into(),
                value: FrontMatterValue::Text("folio-intro".into()),
            }
        );
        assert_eq!(
            entries[2],
            FrontMatterEntry {
                key: "disable-model-invocation".into(),
                value: FrontMatterValue::Bool(false),
            }
        );
    }

    #[test]
    fn parses_block_and_flow_lists_and_block_scalars() {
        let src = "---\ntags: [rust, \"gpui\", markdown]\nauthors:\n  - Alice\n  - Bob\nsummary: >-\n  First line\n  second line\n---\nBody";
        let header = split_frontmatter(src).expect("frontmatter detected");
        let entries = parse_entries(header.yaml);
        assert_eq!(header.body, "Body");
        assert_eq!(
            entries[0].value,
            FrontMatterValue::List(vec!["rust".into(), "gpui".into(), "markdown".into()].into())
        );
        assert_eq!(
            entries[1].value,
            FrontMatterValue::List(vec!["Alice".into(), "Bob".into()].into())
        );
        assert_eq!(
            entries[2].value,
            FrontMatterValue::Text("First line second line".into())
        );
    }

    #[test]
    fn ignores_horizontal_rules_without_yaml_mappings() {
        let src = "---\n\nHello world\n\n---\n\n# Heading\n";
        assert!(split_frontmatter(src).is_none());
    }

    #[test]
    fn toggle_quoted_key_preserves_crlf_and_comment() {
        let yaml = "\"出版: 正式\":  false  # 保留\r\ntitle: 明天\r\n";
        assert_eq!(
            toggle_bool_in_yaml(yaml, "出版: 正式", true),
            "\"出版: 正式\":  true  # 保留\r\ntitle: 明天\r\n"
        );
    }

    #[test]
    fn bidirectional_yaml_tags_conservative_sync() {
        // Plain note without frontmatter is never mutated in conservative mode.
        let plain = "# Just a note\n\nBody\n";
        assert_eq!(extract_tags(plain), None);
        assert_eq!(update_tags_in_markdown(plain, &[], &["Rust".into()]), None);

        // Flow-style tags update in place and no-op when set matches.
        let flow = "---\nname: demo\ntags: [Rust, GPUI]\n---\n\n# Body\n";
        assert_eq!(extract_tags(flow), Some(vec!["Rust".into(), "GPUI".into()]));
        assert_eq!(
            update_tags_in_markdown(flow, &["Rust".into(), "GPUI".into()], &["gpui".into(), "rust".into()]),
            None
        );
        let updated = update_tags_in_markdown(flow, &["Rust".into(), "GPUI".into()], &["Rust".into(), "Markdown".into()]).unwrap();
        assert_eq!(
            updated,
            "---\nname: demo\ntags: [Rust, Markdown]\n---\n\n# Body\n"
        );

        // Block-style tags preserve block style.
        let block = "---\nname: demo\ntags:\n  - Alice\n---\n\nBody";
        let updated = update_tags_in_markdown(block, &["Alice".into()], &["Alice".into(), "Bob".into()]).unwrap();
        assert_eq!(
            updated,
            "---\nname: demo\ntags:\n  - Alice\n  - Bob\n---\n\nBody"
        );

        // Existing frontmatter without tags key appends tags when non-empty.
        let no_tags = "---\nname: demo\n---\n\nBody";
        let updated = update_tags_in_markdown(no_tags, &[], &["Note".into()]).unwrap();
        assert_eq!(updated, "---\nname: demo\ntags: [Note]\n---\n\nBody");
    }

    #[test]
    fn yaml_tag_writeback_keeps_draft_tags_and_original_bytes() {
        let source = "\u{feff}--- \r\nname: demo\r\n\"tags\": # 分类\r\n  # 草稿\r\n  - draft\r\n  - Old # 备注\r\n\r\n... \r\n\r\n正文";
        let after = ["Rust's".into(), "quoted \"tag\", with comma".into()];
        let updated = update_tags_in_markdown(source, &["Old".into()], &after).unwrap();
        assert_eq!(
            updated,
            "\u{feff}--- \r\nname: demo\r\n\"tags\": # 分类\r\n  - draft\r\n  - \"Rust's\"\r\n  - \"quoted \\\"tag\\\", with comma\"\r\n  # 草稿\r\n  # 备注\r\n\r\n... \r\n\r\n正文"
        );
        assert_eq!(
            extract_tags(&updated),
            Some(vec!["draft".into(), after[0].clone(), after[1].clone()])
        );
    }

    #[test]
    fn yaml_tag_sync_skips_structured_values() {
        let source = "---\ntags: {primary: Rust}\n---\n";
        assert_eq!(extract_tags(source), None);
        assert_eq!(update_tags_in_markdown(source, &[], &["new".into()]), None);
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
