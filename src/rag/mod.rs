//! Personal knowledge-base RAG engine backed by SQLite (FTS5 + sqlite-vec) and local llama-server.

pub mod chunker;
pub mod client;
#[cfg(windows)]
pub(crate) mod cuda_engine;
#[cfg(windows)]
mod cuda_ptx;
pub mod ocr;
pub mod pipeline;
pub mod store;

pub use crate::model::RagConfig;
use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, ops::Range};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubSearchType {
    Lex,
    Vec,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubSearch {
    #[serde(rename = "type")]
    pub kind: SubSearchType,
    pub query: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccuratePdfMode {
    OnlyOcr,
    All,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub searches: Vec<SubSearch>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub rerank: Option<bool>,
    #[serde(default)]
    pub extensions: Vec<String>,
    pub under_directory: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    8
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyDocument {
    pub rel_path: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub docid: String,
    pub path: String,
    pub kind: String,
    pub parse_tier: String,
    pub heading: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    pub score: f64,
    pub snippet: String,
    /// UTF-8 byte range of the first highlighted term in the displayed snippet.
    #[serde(skip)]
    pub(crate) keyword: Option<Range<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_cache: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dirty_buffer: bool,
}

impl SearchHit {
    /// Resolve the snippet's keyword against the live document, scoped to the indexed chunk.
    pub(crate) fn source_keyword(&self, text: &str) -> Option<Range<usize>> {
        self.keyword.as_ref()?;
        let first_line = self.line_start?;
        let last_line = self.line_end.unwrap_or(first_line);
        let mut line_offsets = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1));
        let start = line_offsets.nth(first_line - 1)?;
        let end = line_offsets.nth(last_line - first_line).unwrap_or(text.len());
        let mut line_start = start;
        text[start..end]
            .split_inclusive('\n')
            .flat_map(|line| {
                let offset = line_start;
                line_start += line.len();
                let raw = self.keyword_occurrences(line);
                let cleaned = if raw.is_empty() {
                    Vec::new()
                } else {
                    self.keyword_occurrences(&pipeline::clean_inline_markdown_line(line.trim()))
                };
                // Formatting may surround the term. Use cleaned context only when the
                // projection preserves each occurrence, so the raw byte range stays valid.
                let same_occurrences = raw.len() == cleaned.len();
                raw.into_iter().enumerate().map(move |(index, (range, mut score))| {
                    if same_occurrences {
                        score = score.max(cleaned[index].1);
                    }
                    (offset + range.start..offset + range.end, score)
                })
            })
            .max_by_key(|(range, score)| (*score, Reverse(range.start)))
            .map(|(range, _)| range)
    }

    /// Compare the text adjacent to each occurrence with the excerpt. Markdown
    /// syntax can interrupt either side, so each side contributes independently.
    pub(crate) fn keyword_occurrences(&self, text: &str) -> Vec<(Range<usize>, usize)> {
        let Some(keyword) = &self.keyword else {
            return Vec::new();
        };
        let before = self.snippet[..keyword.start].trim_start_matches('…');
        let after = self.snippet[keyword.end..].trim_end_matches('…');
        let term = self.snippet[keyword.clone()].to_ascii_lowercase();
        text.to_ascii_lowercase()
            .match_indices(&term)
            .map(|(start, _)| {
                let end = start + term.len();
                let left = text[..start]
                    .chars()
                    .rev()
                    .zip(before.chars().rev())
                    .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
                    .count();
                let right = text[end..]
                    .chars()
                    .zip(after.chars())
                    .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
                    .count();
                (start..end, left + right)
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub mode_used: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub indexed_files: usize,
    pub pending_vectors: usize,
    pub results: Vec<SearchHit>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPdfInfo {
    pub path: String,
    pub parse_tier: String,
    pub markdown_cache: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PdfPageMarkdown {
    pub page: usize,
    pub markdown: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_ocr: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfReadResult {
    pub path: String,
    pub source: &'static str,
    pub parse_tier: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_cache: Option<String>,
    pub total_pages: usize,
    pub start_page: usize,
    pub end_page: usize,
    pub needs_ocr: bool,
    pub pages: Vec<PdfPageMarkdown>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub db_path: String,
    pub embed_model: String,
    pub embed_dim: usize,
    pub embed_server_ready: bool,
    pub rerank_server_ready: bool,
    pub accurate_configured: bool,
    pub accurate_upgraded_pdfs: usize,
    pub accurate_pending_pdfs: usize,
    pub indexed_files: usize,
    pub total_chunks: usize,
    pub vector_chunks: usize,
    pub pending_vectors: usize,
    pub needs_ocr_files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cached_pdf_markdowns: Vec<CachedPdfInfo>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReindexReport {
    pub scanned_files: usize,
    pub updated_files: usize,
    pub skipped_unchanged: usize,
    pub removed_files: usize,
    pub embedded_chunks: usize,
    pub pending_vectors: usize,
    /// Whether this sync prepared a backend for pending vectors. Live health is reported by status.
    pub embed_server_ready: bool,
    pub accurate_upgraded_pdfs: usize,
    pub accurate_failed_pdfs: Vec<String>,
    pub needs_ocr_files: Vec<String>,
}

#[derive(Debug)]
pub struct RagError {
    pub code: &'static str,
    pub message: String,
}

impl RagError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new("cancelled", "知识库检索或索引已取消。")
    }
}

impl std::fmt::Display for RagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RagError {}

pub fn doc_kind(rel_path: &str) -> Option<&'static str> {
    let ext = rel_path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "md" => Some("md"),
        "txt" => Some("txt"),
        "tex" | "ltx" => Some("tex"),
        "bib" => Some("bib"),
        "pdf" => Some("pdf"),
        _ => None,
    }
}

/// Collects project-relative paths eligible for RAG indexing by extension.
/// Derived PDFs are filtered later in `sync_project` strictly by protocol fingerprints.
pub fn collect_eligible_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| doc_kind(path).is_some())
        .map(str::to_owned)
        .collect()
}
