//! Ordered quote attachments share the existing pi prompt format with legacy single quotes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionQuote {
    pub source: String,
    pub text: String,
    /// Draft-only answer identity for duplicate suppression; never injected into model context.
    pub answer: Option<usize>,
}

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
        quotes.push(SelectionQuote { source, text: q_lines.join("\n"), answer: None });
    }
    (attached_file, skills, quotes, lines.collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_quotes_round_trip_with_file_skill_and_question() {
        let quotes = vec![
            SelectionQuote { source: "AI 回答".into(), text: "第一段\n\n> 原有引用".into(), answer: None },
            SelectionQuote { source: "笔记.md".into(), text: "const 对象只能调用常量成员函数".into(), answer: None },
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
                source: "old.pdf".into(), text: "第一行\n第二行".into(), answer: None,
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
