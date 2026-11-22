//! Structure-aware chunking for Markdown, LaTeX, BibTeX, plain text, and digital PDFs.

use crate::annotations::AnnotationItem;
use std::sync::Arc;

const TARGET_CHUNK_CHARS: usize = 2200;
const OVERLAP_CHARS: usize = 320;
const HARD_MAX_CHARS: usize = 3200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkDraft {
    pub seq: usize,
    pub heading_path: String,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
    pub page_num: Option<usize>,
    pub chunk_text: String,
}

#[derive(Clone, Debug)]
pub struct ExtractedDocument {
    pub raw_text: String,
    pub parse_tier: &'static str,
    pub needs_ocr: bool,
    pub chunks: Vec<ChunkDraft>,
}

pub fn chunk_text_document(rel_path: &str, kind: &str, text: &str) -> ExtractedDocument {
    let chunks = match kind {
        "bib" => chunk_bibtex(rel_path, text),
        "tex" => chunk_structured_lines(rel_path, text, true),
        _ => chunk_structured_lines(rel_path, text, false),
    };
    ExtractedDocument {
        raw_text: text.to_owned(),
        parse_tier: "source",
        needs_ocr: false,
        chunks,
    }
}

pub fn extract_and_chunk_pdf(rel_path: &str, bytes: Vec<u8>) -> Result<ExtractedDocument, String> {
    if let Ok(inspected) = pdf_inspector::extract_pages_markdown_mem(&bytes, None) {
        return Ok(chunk_inspected_pdf(rel_path, inspected));
    }
    extract_and_chunk_pdf_gpui(rel_path, bytes)
}

fn chunk_inspected_pdf(
    rel_path: &str,
    inspected: pdf_inspector::PagesExtractionResult,
) -> ExtractedDocument {
    let page_count = inspected.pages.len();
    let mut non_ws_chars = 0usize;
    let mut full_raw = String::new();
    let mut chunks = Vec::new();

    for page in &inspected.pages {
        let page_num = (page.page as usize) + 1;
        let trimmed = page.markdown.trim();
        non_ws_chars += trimmed.chars().filter(|c| !c.is_whitespace()).count();
        if trimmed.is_empty() {
            continue;
        }
        if !full_raw.is_empty() {
            full_raw.push_str("\n\n");
        }
        full_raw.push_str(&format!("<!-- page {page_num} -->\n{trimmed}"));

        let heading = format!("{rel_path} > p.{page_num}");
        for piece in split_text_with_overlap(trimmed, TARGET_CHUNK_CHARS, OVERLAP_CHARS) {
            let seq = chunks.len();
            chunks.push(ChunkDraft {
                seq,
                heading_path: heading.clone(),
                line_start: None,
                line_end: None,
                page_num: Some(page_num),
                chunk_text: format!("[{heading}]\n{piece}"),
            });
        }
    }

    let has_content_ocr_reason = inspected.ocr_reasons_by_page.iter().any(|entry| {
        entry
            .reasons
            .iter()
            .any(|reason| reason != pdf_inspector::OCR_REASON_NO_TEXT)
    });
    let needs_ocr = has_content_ocr_reason || (page_count > 0 && non_ws_chars < 40);

    ExtractedDocument {
        raw_text: full_raw,
        parse_tier: "fast_pdf",
        needs_ocr,
        chunks,
    }
}

fn extract_and_chunk_pdf_gpui(rel_path: &str, bytes: Vec<u8>) -> Result<ExtractedDocument, String> {
    let document = gpui_pdf::parse(Arc::new(bytes))
        .map_err(|err| format!("PDF 解析失败 ({rel_path}): {err:?}"))?;
    let page_count = document.pages().len();
    let mut pages = Vec::with_capacity(page_count);
    let mut non_ws_chars = 0usize;
    let mut full_raw = String::new();

    for idx in 0..page_count {
        let text = gpui_pdf::extract_page_text(&document, idx)
            .map(|page| page.text())
            .unwrap_or_default();
        non_ws_chars += text.chars().filter(|c| !c.is_whitespace()).count();
        if !text.trim().is_empty() {
            if !full_raw.is_empty() {
                full_raw.push_str("\n\n");
            }
            full_raw.push_str(&format!("<!-- page {} -->\n{}", idx + 1, text.trim()));
        }
        pages.push((idx + 1, text));
    }

    let needs_ocr = page_count > 0 && non_ws_chars < 40;
    let mut chunks = Vec::new();
    for (page_num, page_text) in pages {
        if page_text.trim().is_empty() {
            continue;
        }
        let heading = format!("{rel_path} > p.{page_num}");
        for piece in split_text_with_overlap(&page_text, TARGET_CHUNK_CHARS, OVERLAP_CHARS) {
            let seq = chunks.len();
            chunks.push(ChunkDraft {
                seq,
                heading_path: heading.clone(),
                line_start: None,
                line_end: None,
                page_num: Some(page_num),
                chunk_text: format!("[{heading}]\n{piece}"),
            });
        }
    }

    Ok(ExtractedDocument {
        raw_text: full_raw,
        parse_tier: "fast_pdf",
        needs_ocr,
        chunks,
    })
}

pub fn chunk_annotations(target_rel_path: &str, items: &[AnnotationItem]) -> ExtractedDocument {
    let mut raw_text = String::new();
    let mut chunks = Vec::new();
    for (seq, item) in items.iter().enumerate() {
        let is_pdf = target_rel_path.to_ascii_lowercase().ends_with(".pdf");
        let page_num = is_pdf.then_some(item.page + 1);
        let heading = match page_num {
            Some(page) => format!("{target_rel_path} > 批注 p.{page}"),
            None => format!("{target_rel_path} > 批注"),
        };
        let mut body = String::new();
        if !item.quote.trim().is_empty() {
            body.push_str("引用: ");
            body.push_str(item.quote.trim());
        }
        if !item.comment.trim().is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("批注: ");
            body.push_str(item.comment.trim());
        }
        if body.trim().is_empty() {
            continue;
        }
        if !raw_text.is_empty() {
            raw_text.push_str("\n\n");
        }
        raw_text.push_str(&body);
        chunks.push(ChunkDraft {
            seq,
            heading_path: heading.clone(),
            line_start: None,
            line_end: None,
            page_num,
            chunk_text: format!("[{heading}]\n{body}"),
        });
    }
    ExtractedDocument {
        raw_text,
        parse_tier: "annotation",
        needs_ocr: false,
        chunks,
    }
}

fn chunk_structured_lines(rel_path: &str, text: &str, is_tex: bool) -> Vec<ChunkDraft> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut sections: Vec<(String, usize, usize)> = Vec::new();
    let mut section_start = 1usize;
    let mut current_heading = rel_path.to_owned();
    let mut in_fence = false;

    for (idx, raw_line) in lines.iter().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim();
        if !is_tex && trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        let heading = if !in_fence {
            if is_tex {
                parse_tex_heading(trimmed)
            } else {
                parse_markdown_heading(trimmed)
            }
        } else {
            None
        };

        if let Some((level, title)) = heading {
            if line_no > section_start {
                sections.push((current_heading.clone(), section_start, line_no - 1));
            }
            while stack.last().is_some_and(|(lvl, _)| *lvl >= level) {
                stack.pop();
            }
            stack.push((level, title));
            current_heading = format_breadcrumb(rel_path, &stack);
            section_start = line_no;
        }
    }
    if section_start <= lines.len() {
        sections.push((current_heading, section_start, lines.len()));
    }

    let mut chunks = Vec::new();
    for (heading_path, start, end) in sections {
        split_section_lines(
            &lines[start - 1..end],
            start,
            &heading_path,
            is_tex,
            &mut chunks,
        );
    }
    chunks
}

fn split_section_lines(
    lines: &[&str],
    base_line: usize,
    heading_path: &str,
    is_tex: bool,
    out: &mut Vec<ChunkDraft>,
) {
    if lines.iter().all(|l| l.trim().is_empty()) {
        return;
    }

    let mut i = 0usize;
    while i < lines.len() {
        let chunk_start = i;
        let mut chars = 0usize;
        let mut in_fence = false;
        let mut in_display_math = false;
        let mut tex_env_depth = 0usize;
        let mut last_safe_end = i;

        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();
            chars += line.chars().count() + 1;

            if !is_tex && trimmed.starts_with("```") {
                in_fence = !in_fence;
            }
            if trimmed == "$$" || (trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() == 2) {
                in_display_math = !in_display_math;
            }
            if is_tex {
                if starts_protected_tex_env(trimmed) {
                    tex_env_depth += 1;
                }
                if ends_protected_tex_env(trimmed) && tex_env_depth > 0 {
                    tex_env_depth -= 1;
                }
            }

            let protected = in_fence || in_display_math || tex_env_depth > 0;
            if !protected && (trimmed.is_empty() || chars >= TARGET_CHUNK_CHARS) {
                last_safe_end = i + 1;
            }

            i += 1;
            if (!protected && chars >= TARGET_CHUNK_CHARS) || chars >= HARD_MAX_CHARS {
                if last_safe_end <= chunk_start {
                    last_safe_end = i;
                }
                break;
            }
        }

        let end = if i >= lines.len() && chars <= HARD_MAX_CHARS {
            lines.len()
        } else {
            last_safe_end.max(chunk_start + 1)
        };

        let body = lines[chunk_start..end].join("\n");
        if !body.trim().is_empty() {
            let seq = out.len();
            out.push(ChunkDraft {
                seq,
                heading_path: heading_path.to_owned(),
                line_start: Some(base_line + chunk_start),
                line_end: Some(base_line + end - 1),
                page_num: None,
                chunk_text: format!("[{heading_path}]\n{}", body.trim()),
            });
        }

        if end >= lines.len() {
            break;
        }

        // Step backward by ~OVERLAP_CHARS on whole-line boundaries for context continuity.
        let mut overlap = 0usize;
        let mut next_start = end;
        while next_start > chunk_start + 1 && overlap < OVERLAP_CHARS {
            next_start -= 1;
            overlap += lines[next_start].chars().count() + 1;
        }
        i = next_start.max(chunk_start + 1);
    }
}

fn chunk_bibtex(rel_path: &str, text: &str) -> Vec<ChunkDraft> {
    let lines: Vec<&str> = text.lines().collect();
    let mut entries: Vec<(String, usize, usize)> = Vec::new();
    let mut current: Option<(String, usize)> = None;

    for (idx, line) in lines.iter().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim();
        if trimmed.starts_with('@')
            && !trimmed.to_ascii_lowercase().starts_with("@comment")
            && !trimmed.to_ascii_lowercase().starts_with("@preamble")
        {
            if let Some((key, start)) = current.take() {
                entries.push((key, start, line_no - 1));
            }
            let key = trimmed
                .split_once('{')
                .and_then(|(_, rest)| rest.split(',').next())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("entry")
                .to_owned();
            current = Some((key, line_no));
        }
    }
    if let Some((key, start)) = current {
        entries.push((key, start, lines.len()));
    }

    if entries.is_empty() {
        return chunk_structured_lines(rel_path, text, false);
    }

    let mut out = Vec::new();
    for (key, start, end) in entries {
        let body = lines[start - 1..end].join("\n");
        if body.trim().is_empty() {
            continue;
        }
        let heading_path = format!("{rel_path} > @{key}");
        let seq = out.len();
        out.push(ChunkDraft {
            seq,
            heading_path: heading_path.clone(),
            line_start: Some(start),
            line_end: Some(end),
            page_num: None,
            chunk_text: format!("[{heading_path}]\n{}", body.trim()),
        });
    }
    out
}

fn split_text_with_overlap(text: &str, target_chars: usize, overlap_chars: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.chars().count() <= target_chars {
        return vec![trimmed.to_owned()];
    }

    let mut out = Vec::new();
    let mut start_byte = 0usize;
    while start_byte < trimmed.len() {
        let mut end_byte = advance_chars(trimmed, start_byte, target_chars);
        if end_byte < trimmed.len()
            && let Some(break_rel) = trimmed[start_byte..end_byte].rfind("\n\n")
            && break_rel > target_chars / 3
        {
            end_byte = start_byte + break_rel;
        }
        let piece = trimmed[start_byte..end_byte].trim();
        if !piece.is_empty() {
            out.push(piece.to_owned());
        }
        if end_byte >= trimmed.len() {
            break;
        }
        let next_start = rewind_chars(trimmed, end_byte, overlap_chars);
        start_byte = if next_start <= start_byte {
            end_byte
        } else {
            next_start
        };
    }
    out
}

fn advance_chars(s: &str, start: usize, count: usize) -> usize {
    s[start..]
        .char_indices()
        .nth(count)
        .map_or(s.len(), |(offset, _)| start + offset)
}

fn rewind_chars(s: &str, end: usize, count: usize) -> usize {
    s[..end]
        .char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(0, |(offset, _)| offset)
}

fn parse_markdown_heading(line: &str) -> Option<(usize, String)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
        let title = line[hashes..].trim().trim_end_matches('#').trim();
        if !title.is_empty() {
            return Some((hashes, title.to_owned()));
        }
    }
    None
}

fn parse_tex_heading(line: &str) -> Option<(usize, String)> {
    const PREFIXES: &[(&str, usize)] = &[
        ("\\part", 1),
        ("\\chapter", 2),
        ("\\section", 3),
        ("\\subsection", 4),
        ("\\subsubsection", 5),
    ];
    for &(prefix, level) in PREFIXES {
        if let Some(rest) = line.strip_prefix(prefix) {
            let rest = rest.strip_prefix('*').unwrap_or(rest).trim_start();
            if let Some(inner) = rest.strip_prefix('{')
                && let Some((title, _)) = inner.split_once('}')
            {
                let title = title.trim();
                if !title.is_empty() {
                    return Some((level, title.to_owned()));
                }
            }
        }
    }
    None
}

fn starts_protected_tex_env(line: &str) -> bool {
    const ENVS: &[&str] = &[
        "theorem", "lemma", "proposition", "corollary", "definition", "proof", "equation",
        "equation*", "align", "align*", "gather", "gather*", "multline", "multline*",
    ];
    line.strip_prefix("\\begin{")
        .and_then(|rest| rest.split_once('}'))
        .is_some_and(|(env, _)| ENVS.contains(&env))
}

fn ends_protected_tex_env(line: &str) -> bool {
    const ENVS: &[&str] = &[
        "theorem", "lemma", "proposition", "corollary", "definition", "proof", "equation",
        "equation*", "align", "align*", "gather", "gather*", "multline", "multline*",
    ];
    line.strip_prefix("\\end{")
        .and_then(|rest| rest.split_once('}'))
        .is_some_and(|(env, _)| ENVS.contains(&env))
}

fn format_breadcrumb(rel_path: &str, stack: &[(usize, String)]) -> String {
    if stack.is_empty() {
        return rel_path.to_owned();
    }
    let mut out = String::from(rel_path);
    for (_, title) in stack {
        out.push_str(" > ");
        out.push_str(title);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_chunks_preserve_headings_and_line_ranges() {
        let doc = "# Convex Opt\n\nIntro line.\n\n## KKT\n\nCondition 1.\nCondition 2.\n";
        let extracted = chunk_text_document("notes/opt.md", "md", doc);
        assert_eq!(extracted.chunks.len(), 2);
        assert_eq!(extracted.chunks[0].heading_path, "notes/opt.md > Convex Opt");
        assert_eq!(extracted.chunks[0].line_start, Some(1));
        assert_eq!(extracted.chunks[0].line_end, Some(4));
        assert_eq!(
            extracted.chunks[1].heading_path,
            "notes/opt.md > Convex Opt > KKT"
        );
        assert_eq!(extracted.chunks[1].line_start, Some(5));
        assert_eq!(extracted.chunks[1].line_end, Some(8));
    }

    #[test]
    fn tex_chunks_keep_protected_theorem_block_together() {
        let body = "a ".repeat(1200);
        let tex = format!(
            "\\section{{Theory}}\n{}\n\\begin{{theorem}}\nImportant bound.\n\\end{{theorem}}\nTail.\n",
            body
        );
        let extracted = chunk_text_document("paper.tex", "tex", &tex);
        assert!(extracted.chunks.iter().any(|c| {
            c.chunk_text.contains("\\begin{theorem}") && c.chunk_text.contains("\\end{theorem}")
        }));
    }

    #[test]
    fn bibtex_chunks_split_by_entry() {
        let bib = "@article{vaswani2017,\n  title={Attention Is All You Need}\n}\n\n@inproceedings{krizhevsky2012,\n  title={ImageNet}\n}\n";
        let extracted = chunk_text_document("refs.bib", "bib", bib);
        assert_eq!(extracted.chunks.len(), 2);
        assert_eq!(extracted.chunks[0].heading_path, "refs.bib > @vaswani2017");
        assert_eq!(extracted.chunks[1].heading_path, "refs.bib > @krizhevsky2012");
    }
}
