//! Structure-aware chunking for Markdown, LaTeX, BibTeX, plain text, and digital PDFs.

use crate::annotations::AnnotationItem;
use flate2::read::GzDecoder;
use std::{
    io::Read,
    ops::Range,
    sync::{Arc, LazyLock},
};
use tokenizers::{Encoding, Tokenizer};

const TARGET_CHUNK_TOKENS: usize = 384;
const OVERLAP_TOKENS: usize = 56;
// The managed embedding server uses a 512-token microbatch. Leave room for its special tokens.
const HARD_MAX_TOKENS: usize = 480;
const MAX_HEADING_TOKENS: usize = 96;

static TOKENIZER: LazyLock<Tokenizer> = LazyLock::new(|| {
    let compressed = include_bytes!("../../resources/rag/qwen3-embedding-tokenizer.json.gz");
    let mut json = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut json)
        .expect("the bundled tokenizer gzip is valid");
    Tokenizer::from_bytes(json).expect("the bundled Qwen3-Embedding tokenizer is valid")
});

fn encode(text: &str) -> Encoding {
    TOKENIZER
        .encode(text, false)
        .expect("Qwen3 BPE encodes UTF-8 text")
}

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

        append_page_chunks(rel_path, page_num, trimmed, &mut chunks);
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
        append_page_chunks(rel_path, page_num, page_text.trim(), &mut chunks);
    }

    Ok(ExtractedDocument {
        raw_text: full_raw,
        parse_tier: "fast_pdf",
        needs_ocr,
        chunks,
    })
}

fn append_page_chunks(rel_path: &str, page: usize, body: &str, out: &mut Vec<ChunkDraft>) {
    let heading_path = format!("{rel_path} > p.{page}");
    let paragraphs: Vec<_> = body
        .match_indices("\n\n")
        .map(|(offset, _)| offset + 2)
        .collect();
    for (_, chunk_text) in split_token_chunks(&heading_path, body, &paragraphs, false) {
        out.push(ChunkDraft {
            seq: out.len(),
            heading_path: heading_path.clone(),
            line_start: None,
            line_end: None,
            page_num: Some(page),
            chunk_text,
        });
    }
}

pub fn chunk_annotations(target_rel_path: &str, items: &[AnnotationItem]) -> ExtractedDocument {
    let mut raw_text = String::new();
    let mut chunks = Vec::new();
    for item in items {
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
        for (_, chunk_text) in split_token_chunks(&heading, &body, &[], false) {
            chunks.push(ChunkDraft {
                seq: chunks.len(),
                heading_path: heading.clone(),
                line_start: None,
                line_end: None,
                page_num,
                chunk_text,
            });
        }
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
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut boundaries = Vec::new();
    let mut byte = 0;
    let mut in_fence = false;
    let mut in_display_math = false;
    let mut tex_env_depth = 0usize;
    // Track protection across the section once so overlapped chunks cannot reinterpret delimiters.
    for line in lines {
        line_starts.push(byte);
        byte += line.len() + 1;
        let trimmed = line.trim();
        if !is_tex && trimmed.starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence && trimmed == "$$" {
            in_display_math = !in_display_math;
        }
        if is_tex {
            if starts_protected_tex_env(trimmed) {
                tex_env_depth += 1;
            }
            if ends_protected_tex_env(trimmed) {
                tex_env_depth = tex_env_depth.saturating_sub(1);
            }
        }

        if !in_fence && !in_display_math && tex_env_depth == 0 {
            boundaries.push(byte);
        }
    }

    let body = lines.join("\n");
    for (range, chunk_text) in split_token_chunks(heading_path, &body, &boundaries, true) {
        let first_line = line_starts.partition_point(|&offset| offset <= range.start);
        let last_line = if range.end == body.len() {
            lines.len()
        } else {
            line_starts.partition_point(|&offset| offset < range.end)
        };
        out.push(ChunkDraft {
            seq: out.len(),
            heading_path: heading_path.to_owned(),
            line_start: Some(base_line + first_line - 1),
            line_end: Some(base_line + last_line - 1),
            page_num: None,
            chunk_text,
        });
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
        let heading_path = format!("{rel_path} > @{key}");
        split_section_lines(
            &lines[start - 1..end],
            start,
            &heading_path,
            false,
            &mut out,
        );
    }
    out
}

/// Returns ranges in the original UTF-8 body and the corresponding embedding inputs.
fn split_token_chunks(
    heading: &str,
    body: &str,
    boundaries: &[usize],
    structured: bool,
) -> Vec<(Range<usize>, String)> {
    if body.trim().is_empty() {
        return Vec::new();
    }

    let heading_encoding = encode(heading);
    let heading = if heading_encoding.len() > MAX_HEADING_TOKENS {
        // Keep the full breadcrumb in metadata even if it would crowd out the embedding text.
        format!(
            "{}…",
            &heading[..heading_encoding.get_offsets()[MAX_HEADING_TOKENS - 1].1]
        )
    } else {
        heading.to_owned()
    };
    let prefix = format!("[{heading}]\n");
    let prefix_tokens = encode(&prefix).len();
    let target = TARGET_CHUNK_TOKENS - prefix_tokens;
    let hard_max = HARD_MAX_TOKENS - prefix_tokens;
    let encoding = encode(body);
    // NFC and byte-level tokens can share source spans. Track the rightmost byte covered.
    let token_ends: Vec<_> = encoding
        .get_offsets()
        .iter()
        .scan(0, |covered, &(_, end)| {
            *covered = (*covered).max(end);
            Some(*covered)
        })
        .collect();

    let mut chunks = Vec::new();
    let mut start = 0;
    let mut previous_end = 0;
    while start < body.len() {
        let first_token = token_ends.partition_point(|&end| end <= start);
        let target_end = token_ends
            .get(first_token + target - 1)
            .copied()
            .unwrap_or(body.len());
        let hard_end = token_ends
            .get(first_token + hard_max - 1)
            .copied()
            .unwrap_or(body.len());
        let mut end = body.len();
        if target_end < body.len() {
            let after_target = boundaries.partition_point(|&byte| byte <= target_end);
            let before = boundaries[..after_target]
                .last()
                .copied()
                .filter(|&byte| byte > previous_end);
            let after = boundaries
                .get(after_target)
                .copied()
                .filter(|&byte| byte <= hard_end);
            let midpoint = token_ends[first_token + target / 2];
            let boundary = before
                .filter(|&byte| byte >= midpoint)
                .or(after)
                .or(before.filter(|_| structured));
            if boundary.is_none()
                && structured
                && start < previous_end
                && boundaries.binary_search(&previous_end).is_ok()
            {
                // Drop optional overlap before splitting a block that may fit on its own.
                start = previous_end;
                continue;
            }
            end = boundary.unwrap_or(target_end);
        }

        // Re-encode the actual input because BPE can merge differently at a cut or after a prefix.
        let mut chunk_text = format!("{prefix}{}", body[start..end].trim());
        while encode(&chunk_text).len() > HARD_MAX_TOKENS {
            end = start
                + body[start..end]
                    .char_indices()
                    .next_back()
                    .expect("chunk has a body")
                    .0;
            chunk_text = format!("{prefix}{}", body[start..end].trim());
        }
        if !body[start..end].trim().is_empty() {
            chunks.push((start..end, chunk_text));
        }
        if end == body.len() {
            break;
        }

        let end_token = token_ends.partition_point(|&token_end| token_end <= end);
        let overlap_start = end_token.checked_sub(OVERLAP_TOKENS + 1)
            .map_or(0, |index| token_ends[index]);
        let next = if structured && boundaries.binary_search(&end).is_ok() {
            // A structured overlap starts between blocks, even if that leaves less overlap.
            boundaries[boundaries.partition_point(|&byte| byte < overlap_start)]
        } else {
            overlap_start
        };
        previous_end = end;
        start = if next > start { next } else { end };
    }
    chunks
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
        assert!(extracted.chunks.len() > 1);
        assert!(extracted.chunks.iter().any(|c| {
            c.chunk_text.contains("\\begin{theorem}") && c.chunk_text.contains("\\end{theorem}")
        }));
    }

    #[test]
    fn token_chunks_keep_long_mixed_utf8_lines_within_budget() {
        let text = format!(
            "开头 {} 结束",
            "混合检索 keeps source offsets 🧬 e\u{301}\u{327}; ".repeat(180)
        );
        let extracted = chunk_text_document("notes.txt", "txt", &text);
        let chunks = &extracted.chunks;
        assert!(chunks.len() > 1);
        assert!(
            chunks
                .first()
                .unwrap()
                .chunk_text
                .starts_with("[notes.txt]\n开头")
        );
        assert!(chunks.last().unwrap().chunk_text.ends_with("结束"));
        assert!(chunks.iter().all(|chunk| {
            let (_, body) = chunk.chunk_text.split_once('\n').unwrap();
            text.contains(body)
                && encode(&chunk.chunk_text).len() <= HARD_MAX_TOKENS
                && chunk.line_start == Some(1)
                && chunk.line_end == Some(1)
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
