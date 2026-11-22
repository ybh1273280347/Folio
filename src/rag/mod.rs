//! Personal knowledge-base RAG engine backed by SQLite (FTS5 + sqlite-vec) and local llama-server.

pub mod chunker;
pub mod client;
pub mod mineru;
pub mod pipeline;
pub mod store;

pub use crate::model::RagConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Hybrid,
    Keyword,
    Semantic,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub lex_queries: Vec<String>,
    #[serde(default)]
    pub vec_queries: Vec<String>,
    #[serde(default)]
    pub mode: SearchMode,
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

#[derive(Clone, Debug)]
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dirty_buffer: bool,
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
pub struct StatusReport {
    pub db_path: String,
    pub embed_model: String,
    pub embed_dim: usize,
    pub embed_server_ready: bool,
    pub rerank_server_ready: bool,
    pub mineru_configured: bool,
    pub mineru_upgraded_pdfs: usize,
    pub mineru_pending_pdfs: usize,
    pub indexed_files: usize,
    pub total_chunks: usize,
    pub vector_chunks: usize,
    pub pending_vectors: usize,
    pub needs_ocr_files: Vec<String>,
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
    pub embed_server_ready: bool,
    pub mineru_upgraded_pdfs: usize,
    pub mineru_failed_pdfs: Vec<String>,
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

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

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

/// Collects project-relative paths eligible for RAG indexing, skipping PDFs compiled from a sibling `.tex`.
pub fn collect_eligible_paths(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let all: Vec<String> = paths.into_iter().collect();
    let tex_stems: HashSet<String> = all
        .iter()
        .filter_map(|p| {
            let lower = p.to_ascii_lowercase();
            lower
                .strip_suffix(".tex")
                .or_else(|| lower.strip_suffix(".ltx"))
                .map(str::to_owned)
        })
        .collect();

    all.into_iter()
        .filter(|path| {
            let Some(kind) = doc_kind(path) else {
                return false;
            };
            if kind == "pdf"
                && let Some(stem) = path.to_ascii_lowercase().strip_suffix(".pdf")
                && tex_stems.contains(stem)
            {
                return false;
            }
            true
        })
        .collect()
}
