//! Personal knowledge-base RAG engine backed by SQLite (FTS5 + sqlite-vec) and local llama-server.

pub mod chunker;
pub mod client;
pub mod ocr;
pub mod pipeline;
pub mod store;

pub use crate::model::RagConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_cache: Option<String>,
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

/// Collects project-relative paths eligible for RAG indexing by extension.
/// Derived PDFs are filtered later in `sync_project` strictly by protocol fingerprints.
pub fn collect_eligible_paths(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| doc_kind(path).is_some())
        .collect()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DerivedPdfEntry {
    pub pdf_hash: String,
    pub source_path: String,
    pub pdf_path: String,
    pub origin: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DerivedMarkdownEntry {
    pub source_pdf_path: String,
    pub source_pdf_hash: String,
    pub markdown_hash: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DerivedPdfManifest {
    #[serde(default)]
    entries: Vec<DerivedPdfEntry>,
}

fn derived_manifest_path(project_root: &Path) -> PathBuf {
    project_root.join(".folio").join("derived-pdfs.json")
}

fn project_rel_slash(project_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(project_root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() {
        None
    } else {
        Some(rel_str)
    }
}

pub fn record_derived_pdf(
    project_root: &Path,
    source_path: &Path,
    pdf_path: &Path,
    pdf_bytes: &[u8],
    origin: &str,
) -> std::io::Result<()> {
    let Some(pdf_rel) = project_rel_slash(project_root, pdf_path) else {
        return Ok(());
    };
    let source_rel = project_rel_slash(project_root, source_path).unwrap_or_default();
    let pdf_hash = sha256_hex(pdf_bytes);
    let manifest_path = derived_manifest_path(project_root);

    let mut manifest: DerivedPdfManifest = fs::read(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();

    manifest.entries.retain(|e| e.pdf_hash != pdf_hash);
    manifest.entries.push(DerivedPdfEntry {
        pdf_hash,
        source_path: source_rel,
        pdf_path: pdf_rel,
        origin: origin.to_owned(),
    });

    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?;
    fs::write(&manifest_path, json)
}

pub fn load_derived_pdf_hashes(project_root: &Path) -> HashSet<String> {
    let manifest_path = derived_manifest_path(project_root);
    let Ok(raw) = fs::read(&manifest_path) else {
        return HashSet::new();
    };
    let Ok(manifest) = serde_json::from_slice::<DerivedPdfManifest>(&raw) else {
        return HashSet::new();
    };
    manifest.entries.into_iter().map(|e| e.pdf_hash).collect()
}

fn derived_markdown_manifest_path(project_root: &Path) -> PathBuf {
    project_root.join(".folio").join("derived-markdowns.json")
}

/// Record the PDF source and exact exported text so later indexing can distinguish
/// an untouched convenience export from a Markdown file the user edited.
pub fn record_derived_markdown(
    project_root: &Path,
    markdown_path: &Path,
    source_pdf_path: &Path,
    source_pdf_hash: &str,
    markdown: &str,
) -> std::io::Result<()> {
    let Some(path) = project_rel_slash(project_root, markdown_path) else {
        return Ok(());
    };
    let manifest_path = derived_markdown_manifest_path(project_root);
    let mut entries: HashMap<String, DerivedMarkdownEntry> = fs::read(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    entries.insert(
        path,
        DerivedMarkdownEntry {
            source_pdf_path: project_rel_slash(project_root, source_pdf_path).unwrap_or_default(),
            source_pdf_hash: source_pdf_hash.to_owned(),
            markdown_hash: sha256_hex(markdown.as_bytes()),
        },
    );
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(&entries).map_err(std::io::Error::other)?;
    fs::write(manifest_path, json)
}

pub fn load_derived_markdown_entries(
    project_root: &Path,
) -> HashMap<String, DerivedMarkdownEntry> {
    fs::read(derived_markdown_manifest_path(project_root))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

pub fn has_folio_pdf_fingerprint(bytes: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"/Keywords (folio-derived)",
        b"/Keywords(folio-derived)",
        b"<pdf:Keywords>folio-derived</pdf:Keywords>",
    ];
    MARKERS
        .iter()
        .any(|needle| bytes.windows(needle.len()).any(|w| w == *needle))
}
