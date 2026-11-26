//! Inline quotes travel in the Pi message body. Historical prefix quotes still load separately.

use super::{InlineToken, InputContent};

const INLINE_QUOTE_OPEN: &str = "<folio-quote>";
const INLINE_QUOTE_CLOSE: &str = "</folio-quote>";
const QUOTE_TOKEN_PREFIX: &str = "quote:";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectionQuote {
    pub source: String,
    pub text: String,
}

impl SelectionQuote {
    pub(super) fn token(&self) -> InlineToken {
        let excerpt = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut chars = excerpt.chars().filter(|ch| !ch.is_control());
        let mut preview = format!("❝ {}", chars.by_ref().take(32).collect::<String>());
        if chars.next().is_some() {
            preview.push('…');
        }
        // The token owns the full quote, so undo and redo restore the source and text together.
        let data = serde_json::to_string(self).expect("quote strings serialize");
        // Invisible separators keep the token atomic even next to combining characters.
        InlineToken::new(format!("{QUOTE_TOKEN_PREFIX}{data}"), format!("\u{2063}{preview}\u{2063}"))
            .with_label(preview)
    }

    pub(super) fn from_token(token: &InlineToken) -> Option<Self> {
        let data = token.id().strip_prefix(QUOTE_TOKEN_PREFIX)?;
        Some(serde_json::from_str(data).expect("Folio quote token holds quote JSON"))
    }

    pub(super) fn tooltip(&self) -> String {
        format!("{}\n{}", self.source, self.text)
    }
}

/// Both new prompts and edited prompts use the same positional encoding.
pub(super) fn encode_prompt_body(content: &InputContent) -> (String, Vec<String>) {
    let text = content.text();
    let mut wire = String::with_capacity(text.len());
    let mut skills = Vec::new();
    let mut cursor = 0;
    for span in content.tokens() {
        let range = span.range();
        let token = span.token();
        wire.push_str(&text[cursor..range.start]);
        if let Some(quote) = token.id().strip_prefix(QUOTE_TOKEN_PREFIX) {
            wire.push_str(INLINE_QUOTE_OPEN);
            // A quote on the first line cannot be mistaken for a bare @file or /skill directive.
            wire.push(' ');
            // A literal closing tag in a source excerpt must stay inside the JSON string.
            wire.push_str(&quote.replace('<', "\\u003c"));
            wire.push_str(INLINE_QUOTE_CLOSE);
        } else if let Some(skill) = token.id().strip_prefix("skill:") {
            let full = format!("skill:{skill}");
            if !skills.contains(&full) {
                skills.push(full);
            }
            wire.push_str(token.label());
        } else {
            wire.push_str(&text[range.clone()]);
        }
        cursor = range.end;
    }
    wire.push_str(&text[cursor..]);
    (wire.trim().to_owned(), skills)
}

/// Decode only complete quote markers. Plain text outside them keeps its original position.
pub(super) fn decode_prompt_body(wire: &str) -> InputContent {
    let mut text = String::with_capacity(wire.len());
    let mut spans = Vec::new();
    let mut cursor = 0;
    for (start, _) in wire.match_indices(INLINE_QUOTE_OPEN) {
        let data_start = start + INLINE_QUOTE_OPEN.len();
        let Some(close) = wire[data_start..].find(INLINE_QUOTE_CLOSE) else {
            break;
        };
        let data_end = data_start + close;
        let Ok(quote) = serde_json::from_str::<SelectionQuote>(&wire[data_start..data_end]) else {
            continue;
        };
        let token = quote.token();
        text.push_str(&wire[cursor..start]);
        let start = text.len();
        text.push_str(token.text());
        spans.push((start..text.len(), token));
        cursor = data_end + INLINE_QUOTE_CLOSE.len();
    }
    text.push_str(&wire[cursor..]);
    spans.into_iter().fold(InputContent::new(text), |body, (range, token)| {
        body.with_token(range, token).expect("quote preview preserves token boundaries")
    })
}

#[cfg(test)]
pub(super) fn append_quotes(payload: &mut String, quotes: &[SelectionQuote]) {
    for quote in quotes {
        payload.push_str("> 引用自 ");
        payload.push_str(&quote.source);
        payload.push('\n');
        for line in quote.text.lines() {
            payload.push_str("> ");
            payload.push_str(line);
            payload.push('\n');
        }
        payload.push('\n');
    }
}

pub(super) fn split_injected_prompt(
    raw: &str,
) -> (Option<String>, Vec<String>, Vec<SelectionQuote>, String) {
    let mut lines = raw.lines().peekable();
    let mut skills = Vec::new();
    while let Some(first) = lines.peek() {
        if let Some(sk) = first.strip_prefix("/skill:") {
            let trimmed = sk.trim();
            if !trimmed.is_empty() && !trimmed.contains(' ') {
                skills.push(format!("skill:{trimmed}"));
                lines.next();
                continue;
            }
        }
        break;
    }
    while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
        lines.next();
    }
    let mut attached_file = None;
    if let Some(first) = lines.peek()
        && let Some(rest) = first.strip_prefix('@')
        && !rest.trim().is_empty()
        && !rest.contains(' ')
    {
        attached_file = Some(rest.trim().to_owned());
        lines.next();
        while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
            lines.next();
        }
    }
    let mut quotes = Vec::new();
    // Blank lines separate quote blocks from one another and from the user's own question.
    while let Some(first) = lines.peek()
        && let Some(src) = first.strip_prefix("> 引用自 ")
    {
        let source = src.trim().to_owned();
        lines.next();
        let mut q_lines = Vec::new();
        while let Some(l) = lines.peek() {
            if let Some(body) = l.strip_prefix("> ") {
                q_lines.push(body.to_owned());
                lines.next();
            } else if *l == ">" {
                q_lines.push(String::new());
                lines.next();
            } else {
                break;
            }
        }
        while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
            lines.next();
        }
        quotes.push(SelectionQuote { source, text: q_lines.join("\n") });
    }
    (attached_file, skills, quotes, lines.collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_quotes_survive_pi_history_and_edit_with_file_and_skill_context() {
        let selected = SelectionQuote {
            source: "笔记.md".into(),
            text: "第一行\r\n第二行：\n> </folio-quote> \"原文\"".into(),
        };
        let quote = selected.token();
        let skill = InlineToken::new("skill:read-markdown", "/skill:read-markdown").with_label("read-markdown");
        let quote_start = "/skill:read-markdown 甲🙂".len();
        let quote_end = quote_start + quote.text().len();
        let repeat_start = quote_end + "乙再说".len();
        let draft = InputContent::new(format!("/skill:read-markdown 甲🙂{}乙再说{}丙", quote.text(), quote.text()))
            .with_token(0..skill.text().len(), skill).unwrap()
            .with_token(quote_start..quote_end, quote.clone()).unwrap()
            .with_token(repeat_start..repeat_start + quote.text().len(), quote.clone()).unwrap();
        let (wire, skills) = encode_prompt_body(&draft);
        let payload = format!("/skill:read-markdown \n@笔记.md\n\n{wire}");
        let (file, restored_skills, legacy, stored_body) = split_injected_prompt(&payload);
        let restored = decode_prompt_body(&stored_body);

        assert_eq!(file.as_deref(), Some("笔记.md"));
        assert_eq!(skills, vec!["skill:read-markdown"]);
        assert_eq!(restored_skills, skills);
        assert!(legacy.is_empty());
        assert_eq!(restored.text().as_ref(), format!("read-markdown 甲🙂{}乙再说{}丙", quote.text(), quote.text()));
        assert_eq!(restored.tokens().len(), 2);
        assert_eq!(SelectionQuote::from_token(restored.tokens()[0].token()), Some(selected.clone()));
        assert_eq!(SelectionQuote::from_token(restored.tokens()[1].token()), Some(selected));
        assert_eq!(encode_prompt_body(&restored).0, wire);
        assert!(wire.starts_with("read-markdown 甲🙂<folio-quote>"));
        assert!(wire.ends_with("</folio-quote>丙"));
    }

    #[test]
    fn multiple_quotes_round_trip_with_file_skill_and_question() {
        let quotes = vec![
            SelectionQuote { source: "AI 回答".into(), text: "第一段\n\n> 原有引用".into() },
            SelectionQuote { source: "笔记.md".into(), text: "const 对象只能调用常量成员函数".into() },
        ];
        let mut payload = String::from("/skill:read-markdown \n\n@notes.md\n\n");
        append_quotes(&mut payload, &quotes);
        payload.push_str("请对照解释\n第二行问题");
        assert_eq!(split_injected_prompt(&payload), (
            Some("notes.md".into()), vec!["skill:read-markdown".into()], quotes,
            "请对照解释\n第二行问题".into(),
        ));
    }

    #[test]
    fn legacy_single_quote_still_loads() {
        assert_eq!(split_injected_prompt("> 引用自 old.pdf\n> 第一行\n> 第二行\n\n解释一下"), (
            None, vec![], vec![SelectionQuote {
                source: "old.pdf".into(), text: "第一行\n第二行".into(),
            }], "解释一下".into(),
        ));
    }

    #[test]
    fn plain_message_keeps_ordinary_markdown_quotes() {
        assert_eq!(split_injected_prompt("说明\n\n> 普通引用\n\n继续"), (
            None, vec![], vec![], "说明\n\n> 普通引用\n\n继续".into(),
        ));
    }
}
