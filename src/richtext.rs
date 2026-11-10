//! Editable Markdown projection: syntax is stored at text boundaries, while the
//! native editor receives visible text and style ranges. Only the edited block is serialized.
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use gpui_kit::{FontStyle, FontWeight, HighlightStyle, component::input::TextDecoration, rgb};
use std::ops::Range;

struct Marker {
    at: usize,
    text: String,
    after: bool,
}
pub struct RichText {
    pub text: String,
    markers: Vec<Marker>,
    styles: Vec<(Range<usize>, HighlightStyle)>,
}

impl RichText {
    pub fn new(source: &str) -> Self {
        let arena = Arena::new();
        let mut options = Options::default();
        options.extension.table = true;
        options.extension.tasklist = true;
        options.extension.strikethrough = true;
        options.extension.math_dollars = true;
        let normalized = crate::markdown::normalize_math(source);
        let root = parse_document(&arena, &normalized, &options);
        let lines: Vec<_> = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let range = |position: comrak::nodes::Sourcepos| {
            let start = lines[position.start.line - 1] + position.start.column - 1;
            let end = (lines[position.end.line - 1] + position.end.column).min(source.len());
            start..end
        };
        let mut visible = vec![false; source.len()];
        let mut styles = Vec::new();
        for node in root.descendants().skip(1) {
            let data = node.data.borrow();
            let mut r = range(data.sourcepos);
            match &data.value {
                NodeValue::Text(_) => visible[r].fill(true),
                NodeValue::Code(code) => {
                    if let Some(index) = source[r.clone()].find(&code.literal) {
                        r = r.start + index..r.start + index + code.literal.len();
                        visible[r.clone()].fill(true);
                        styles.push((
                            r,
                            HighlightStyle {
                                color: Some(rgb(0xb45753).into()),
                                ..Default::default()
                            },
                        ));
                    }
                }
                NodeValue::Math(math) => {
                    let delimiter = if source[r.clone()].starts_with("\\(")
                        || source[r.clone()].starts_with("\\[")
                        || math.display_math
                    {
                        2
                    } else {
                        1
                    };
                    r = r.start + delimiter..r.end.saturating_sub(delimiter);
                    visible[r].fill(true);
                }
                NodeValue::CodeBlock(code) => {
                    // Editing keeps the code content visible, not the fenced Markdown wrapper.
                    let literal = code.literal.trim_end_matches('\n');
                    if let Some(offset) = source[r.clone()].find(literal) {
                        visible[r.start + offset..r.start + offset + literal.len()].fill(true);
                    }
                }
                NodeValue::HtmlBlock(_) => visible[r].fill(true),
                NodeValue::Strong => styles.push((
                    r,
                    HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                )),
                NodeValue::Emph => styles.push((
                    r,
                    HighlightStyle {
                        font_style: Some(FontStyle::Italic),
                        ..Default::default()
                    },
                )),
                NodeValue::Heading(_) => styles.push((
                    r,
                    HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                )),
                NodeValue::Link(_) => styles.push((
                    r,
                    HighlightStyle {
                        color: Some(rgb(0x47759e).into()),
                        ..Default::default()
                    },
                )),
                _ => {}
            }
        }
        // Keep line breaks editable, including those separating list items and table rows.
        for (offset, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                visible[offset] = true;
            }
        }
        let mut text = String::new();
        let mut markers = Vec::new();
        let mut mapping = vec![0; source.len() + 1];
        let mut hidden = String::new();
        let mut hidden_at = 0;
        for (offset, character) in source.char_indices() {
            mapping[offset] = text.len();
            if visible[offset] {
                if !hidden.is_empty() {
                    markers.push(Marker {
                        at: hidden_at,
                        text: std::mem::take(&mut hidden),
                        after: false,
                    });
                }
                text.push(character);
            } else {
                if hidden.is_empty() {
                    hidden_at = text.len();
                }
                hidden.push(character);
            }
            for i in 1..=character.len_utf8() {
                mapping[offset + i] = text.len();
            }
        }
        if !hidden.is_empty() {
            markers.push(Marker {
                at: hidden_at,
                text: hidden,
                after: true,
            });
        }
        let styles = styles
            .into_iter()
            .map(|(r, style)| (mapping[r.start]..mapping[r.end], style))
            .collect();
        Self {
            text,
            markers,
            styles,
        }
    }

    pub fn change(&mut self, text: String) {
        // UTF-8 byte offsets match the GPUI editor. Compute a single splice so
        // syntax outside the edited range retains its original ordering.
        let start = self
            .text
            .char_indices()
            .zip(text.char_indices())
            .take_while(|((_, a), (_, b))| a == b)
            .map(|((i, c), _)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        let suffix = self.text[start..]
            .chars()
            .rev()
            .zip(text[start..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let old_end = self.text.len() - suffix;
        let new_end = text.len() - suffix;
        let delta = new_end as isize - old_end as isize;
        for marker in &mut self.markers {
            marker.at = if marker.at > old_end || (marker.at == old_end && marker.after) {
                marker
                    .at
                    .checked_add_signed(delta)
                    .expect("splice stays inside text")
            } else if marker.at > start {
                new_end
            } else {
                marker.at
            };
        }
        for (r, _) in &mut self.styles {
            if r.start > old_end {
                r.start = r.start.checked_add_signed(delta).unwrap();
            } else if r.start > start {
                r.start = start;
            }
            if r.end >= old_end {
                r.end = r.end.checked_add_signed(delta).unwrap();
            } else if r.end > start {
                r.end = new_end;
            }
        }
        self.text = text;
    }

    pub fn format(&mut self, range: Range<usize>, prefix: &str, suffix: &str) {
        self.markers.push(Marker {
            at: range.start,
            text: prefix.into(),
            after: false,
        });
        self.markers.push(Marker {
            at: range.end,
            text: suffix.into(),
            after: true,
        });
        self.markers.sort_by_key(|marker| marker.at);
    }

    pub fn markdown(&self) -> String {
        let mut source = String::new();
        let mut start = 0;
        for marker in &self.markers {
            source.push_str(&self.text[start..marker.at]);
            source.push_str(&marker.text);
            start = marker.at;
        }
        source.push_str(&self.text[start..]);
        source
    }

    pub fn decorations(&self) -> Vec<TextDecoration> {
        self.styles
            .iter()
            .map(|(range, style)| TextDecoration::new(range.clone(), *style))
            .collect()
    }
}
