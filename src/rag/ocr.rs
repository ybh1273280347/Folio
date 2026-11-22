//! PaddleOCR-VL async Jobs API (`paddleocr.aistudio-app.com`) and `.folio/pdf-cache/<sha256>/` persistence.

use super::{
    AccuratePdfMode, PdfPageMarkdown, PdfReadResult, RagConfig, RagError,
    chunker::{chunk_pdf_pages, chunk_text_document, extract_pdf_pages},
    client::Endpoint,
    sha256_hex,
    store::{PdfLibraryItem, RagStore},
};
use crate::http::{self, RequestOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const PADDLEOCR_HOST: &str = "paddleocr.aistudio-app.com";
const PADDLEOCR_JOBS_PATH: &str = "/api/v2/ocr/jobs";
const PADDLEOCR_MODEL: &str = "PaddleOCR-VL-1.6";
const POLL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug)]
pub enum OcrProgressEvent {
    Started {
        rel_path: String,
        total_pages: Option<usize>,
    },
    PageProgress {
        rel_path: String,
        extracted_pages: usize,
        total_pages: Option<usize>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PageAnchor {
    pub page_num: usize,
    pub snippet: String,
}

#[derive(Default)]
pub struct AccurateUpgradeStats {
    pub upgraded_pdfs: usize,
    pub failed_pdfs: Vec<String>,
}

pub fn cache_dir(root: &Path, content_hash: &str) -> PathBuf {
    root.join(".folio").join("pdf-cache").join(content_hash)
}

pub fn resolve_ocr_token(config: &RagConfig) -> String {
    let trimmed = config.paddleocr_token.trim();
    if !trimmed.is_empty() {
        return trimmed.to_owned();
    }
    for env_key in [
        "PADDLEOCR_ACCESS_TOKEN",
        "PADDLEOCR_MCP_AISTUDIO_ACCESS_TOKEN",
    ] {
        if let Ok(val) = std::env::var(env_key) {
            let val = val.trim();
            if !val.is_empty() {
                return val.to_owned();
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let mcp_path = home.join(".pi").join("agent").join("mcp.json");
        if let Ok(bytes) = fs::read(mcp_path)
            && let Ok(val) = serde_json::from_slice::<Value>(&bytes)
            && let Some(token) = val
                .pointer("/mcpServers/paddleocr/env/PADDLEOCR_MCP_AISTUDIO_ACCESS_TOKEN")
                .and_then(Value::as_str)
        {
            let token = token.trim();
            if !token.is_empty() {
                return token.to_owned();
            }
        }
    }
    String::new()
}

#[derive(Debug)]
struct CacheSummary {
    total_pages: usize,
    missing_pages: Vec<usize>,
    is_complete: bool,
    parse_tier: &'static str,
}

fn summarize_pages(pages: &[PdfPageMarkdown]) -> CacheSummary {
    let mut missing_pages: Vec<usize> = pages
        .iter()
        .filter(|p| p.needs_ocr)
        .map(|p| p.page)
        .collect();
    missing_pages.sort_unstable();
    missing_pages.dedup();

    let total_pages = pages.iter().map(|p| p.page).max().unwrap_or(pages.len());
    let is_complete = !pages.is_empty() && missing_pages.is_empty();
    let has_fast = pages
        .iter()
        .any(|p| !p.needs_ocr && p.source.as_deref() == Some("fast"));
    let has_ocr = pages
        .iter()
        .any(|p| !p.needs_ocr && p.source.as_deref() != Some("fast"));

    let parse_tier = if !is_complete {
        "fast_pdf"
    } else if has_fast && has_ocr {
        "hybrid"
    } else if has_ocr {
        "accurate"
    } else {
        "fast_pdf"
    };

    CacheSummary {
        total_pages,
        missing_pages,
        is_complete,
        parse_tier,
    }
}

fn join_pages_markdown(pages: &[PdfPageMarkdown]) -> String {
    let mut out = String::new();
    for p in pages {
        if p.needs_ocr {
            continue;
        }
        let trimmed = p.markdown.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(trimmed);
    }
    out
}

pub fn format_page_ranges(pages: &[usize]) -> String {
    let mut sorted: Vec<usize> = pages.iter().copied().filter(|p| *p > 0).collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    let mut start = sorted[0];
    let mut prev = sorted[0];
    for &p in &sorted[1..] {
        if p == prev + 1 {
            prev = p;
        } else {
            parts.push(if start == prev {
                start.to_string()
            } else {
                format!("{start}-{prev}")
            });
            start = p;
            prev = p;
        }
    }
    parts.push(if start == prev {
        start.to_string()
    } else {
        format!("{start}-{prev}")
    });
    parts.join(",")
}

fn build_anchors(pages: &[PdfPageMarkdown]) -> Vec<PageAnchor> {
    let mut anchors = Vec::new();
    for p in pages {
        if p.needs_ocr {
            continue;
        }
        for line in p.markdown.lines() {
            let norm = normalize_anchor(line);
            if norm.chars().count() >= 8 {
                anchors.push(PageAnchor {
                    page_num: p.page,
                    snippet: norm,
                });
            }
        }
    }
    anchors
}

pub fn write_pages_cache(
    out_dir: &Path,
    pages: &[PdfPageMarkdown],
    anchors: Option<&[PageAnchor]>,
) -> Result<(), String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;
    let pages_bytes = serde_json::to_vec(pages).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("pages_text.json"), pages_bytes).map_err(|e| e.to_string())?;

    let summary = summarize_pages(pages);
    let full_md_path = out_dir.join("full.md");
    if summary.is_complete {
        let full_md = join_pages_markdown(pages);
        if !full_md.trim().is_empty() {
            fs::write(&full_md_path, full_md).map_err(|e| e.to_string())?;
        }
    } else {
        let _ = fs::remove_file(&full_md_path);
    }

    if let Some(anchors) = anchors
        && !anchors.is_empty()
    {
        let json_bytes = serde_json::to_vec(anchors).map_err(|e| e.to_string())?;
        fs::write(out_dir.join("pages.json"), json_bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn read_cached_pages_file(dir: &Path) -> Option<Vec<PdfPageMarkdown>> {
    let raw = fs::read(dir.join("pages_text.json")).ok()?;
    let pages = serde_json::from_slice::<Vec<PdfPageMarkdown>>(&raw).ok()?;
    (!pages.is_empty()).then_some(pages)
}

fn ensure_fast_pages_cached(
    root: &Path,
    rel_path: &str,
    content_hash: &str,
    snapshot: Option<&[u8]>,
) -> Result<Vec<PdfPageMarkdown>, String> {
    let dir = cache_dir(root, content_hash);
    if let Some(pages) = read_cached_pages_file(&dir) {
        return Ok(pages);
    }
    let full_md_path = dir.join("full.md");
    if let Ok(full_md) = fs::read_to_string(&full_md_path)
        && !full_md.trim().is_empty()
    {
        return Ok(vec![PdfPageMarkdown {
            page: 1,
            markdown: full_md,
            source: Some("ocr".to_owned()),
            needs_ocr: false,
        }]);
    }
    // Callers with a verified snapshot can parse those exact bytes on a cache miss.
    let from_disk;
    let bytes = match snapshot {
        Some(bytes) => bytes,
        None => {
            let path = crate::files::project_path(root, rel_path).map_err(|e| e.to_string())?;
            from_disk = fs::read(path).map_err(|e| format!("读取 PDF 失败 ({rel_path}): {e}"))?;
            if sha256_hex(&from_disk) != content_hash {
                return Err(format!("PDF 已更改，请同步知识库后重试 ({rel_path})"));
            }
            &from_disk
        }
    };
    let pages = extract_pdf_pages(rel_path, bytes)?;
    write_pages_cache(&dir, &pages, None)?;
    Ok(pages)
}

/// Populate per-page completeness and page counts on `PdfLibraryItem` from disk cache.
pub fn hydrate_pdf_library_items(root: &Path, items: &mut [PdfLibraryItem]) {
    for item in items {
        let dir = cache_dir(root, &item.content_hash);
        if let Some(pages) = read_cached_pages_file(&dir) {
            let summary = summarize_pages(&pages);
            item.total_pages = Some(summary.total_pages);
            item.missing_pages = summary.missing_pages.len();
            item.is_complete = summary.is_complete;
            if summary.parse_tier != "fast_pdf" || item.parse_tier == "fast_pdf" {
                item.parse_tier = summary.parse_tier.to_owned();
            }
            item.status = if summary.is_complete {
                "indexed".to_owned()
            } else {
                "needs_ocr".to_owned()
            };
        } else if dir.join("full.md").exists() {
            item.missing_pages = 0;
            item.is_complete = true;
            item.status = "indexed".to_owned();
        } else if let Ok(pages) =
            ensure_fast_pages_cached(root, &item.rel_path, &item.content_hash, None)
        {
            let summary = summarize_pages(&pages);
            item.total_pages = Some(summary.total_pages);
            item.missing_pages = summary.missing_pages.len();
            item.is_complete = summary.is_complete;
            item.status = if summary.is_complete {
                "indexed".to_owned()
            } else {
                "needs_ocr".to_owned()
            };
        }
    }
}

pub fn load_cached_pdf(
    root: &Path,
    rel_path: &str,
    content_hash: &str,
) -> Option<super::chunker::ExtractedDocument> {
    let dir = cache_dir(root, content_hash);
    if let Some(pages) = read_cached_pages_file(&dir) {
        let summary = summarize_pages(&pages);
        return Some(chunk_pdf_pages(rel_path, &pages, summary.parse_tier));
    }

    let md_path = dir.join("full.md");
    let md = fs::read_to_string(&md_path).ok()?;
    if md.trim().is_empty() {
        return None;
    }

    let mut extracted = chunk_text_document(rel_path, "md", &md);
    extracted.parse_tier = "accurate";

    let anchors_path = dir.join("pages.json");
    if let Ok(bytes) = fs::read(&anchors_path)
        && let Ok(anchors) = serde_json::from_slice::<Vec<PageAnchor>>(&bytes)
        && !anchors.is_empty()
    {
        let mut last_page = 1usize;
        for chunk in &mut extracted.chunks {
            if let Some(p) = match_chunk_page(&chunk.chunk_text, &anchors, last_page) {
                last_page = p;
            }
            chunk.page_num = Some(last_page);
            chunk.line_start = None;
            chunk.line_end = None;
        }
    }

    Some(extracted)
}

pub fn upgrade_pdfs(
    root: &Path,
    store: &mut RagStore,
    config: &RagConfig,
    mode: AccuratePdfMode,
    paths: &[String],
    cancelled: &AtomicBool,
) -> Result<AccurateUpgradeStats, RagError> {
    let candidates = store
        .pdf_upgrade_candidates(mode, paths)
        .map_err(|e| RagError::new("store_error", e))?;
    if candidates.is_empty() {
        return Ok(AccurateUpgradeStats::default());
    }

    let mut stats = AccurateUpgradeStats::default();
    let mut to_upload: Vec<(String, String)> = Vec::new();

    for (rel_path, hash) in candidates {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RagError::cancelled());
        }
        if let Some(cached) = load_cached_pdf(root, &rel_path, &hash)
            && !cached.needs_ocr
            && matches!(cached.parse_tier, "accurate" | "hybrid")
        {
            store
                .upgrade_pdf_content(&hash, &cached.raw_text, cached.parse_tier, "indexed", &cached.chunks)
                .map_err(|e| RagError::new("store_error", e))?;
            stats.upgraded_pdfs += 1;
        } else {
            to_upload.push((rel_path, hash));
        }
    }

    if to_upload.is_empty() {
        return Ok(stats);
    }

    let token = resolve_ocr_token(config);
    if token.is_empty() {
        return Err(RagError::new(
            "accurate_pdf_unconfigured",
            "尚未配置 PaddleOCR Token，请在「设置 → API 服务」填写后重试。",
        ));
    }

    let fill_missing_only = matches!(mode, AccuratePdfMode::OnlyOcr);
    for (rel_path, hash) in to_upload {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RagError::cancelled());
        }
        match process_single_pdf(
            root,
            store,
            &token,
            &rel_path,
            &hash,
            fill_missing_only,
            cancelled,
            &mut |_| {},
        ) {
            Ok(()) => stats.upgraded_pdfs += 1,
            Err(err) if err.code == "cancelled" => return Err(err),
            Err(err) if err.code == "file_not_found" => {}
            Err(err) if err.code == "stale_pdf" => {
                stats.failed_pdfs.push(format!("{rel_path}: {}", err.message));
            }
            Err(err) => {
                let _ = store.upsert_ocr_job(&hash, &rel_path, None, "failed", Some(&err.message));
                stats.failed_pdfs.push(format!("{rel_path}: {}", err.message));
            }
        }
    }

    Ok(stats)
}

pub fn process_single_pdf_with_progress(
    root: &Path,
    store: &mut RagStore,
    config: &RagConfig,
    rel_path: &str,
    hash: &str,
    fill_missing_only: bool,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(OcrProgressEvent),
) -> Result<(), RagError> {
    let token = resolve_ocr_token(config);
    if token.is_empty() {
        return Err(RagError::new(
            "accurate_pdf_unconfigured",
            "尚未配置 PaddleOCR Token，请在「设置 → API 服务」填写后重试。",
        ));
    }
    match process_single_pdf(
        root,
        store,
        &token,
        rel_path,
        hash,
        fill_missing_only,
        cancelled,
        on_progress,
    ) {
        Ok(()) => Ok(()),
        Err(err) if matches!(err.code, "cancelled" | "file_not_found" | "stale_pdf") => Err(err),
        Err(err) => {
            let _ = store.upsert_ocr_job(hash, rel_path, None, "failed", Some(&err.message));
            Err(err)
        }
    }
}

fn pdf_total_pages(bytes: &[u8]) -> Option<usize> {
    gpui_pdf::parse(Arc::new(bytes.to_vec()))
        .ok()
        .map(|doc| doc.pages().len())
}

fn process_single_pdf(
    root: &Path,
    store: &mut RagStore,
    token: &str,
    rel_path: &str,
    hash: &str,
    fill_missing_only: bool,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(OcrProgressEvent),
) -> Result<(), RagError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(RagError::cancelled());
    }
    let bytes = crate::files::project_path(root, rel_path)
        .and_then(fs::read)
        .map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                "file_not_found"
            } else {
                "io_error"
            };
            RagError::new(code, format!("读取 PDF 失败 ({rel_path}): {e}"))
        })?;
    // The hash owns both the per-page cache and the indexed chunks. A queued job may start
    // after the source file has been replaced at the same path.
    if sha256_hex(&bytes) != hash {
        return Err(RagError::new(
            "stale_pdf",
            format!("PDF 已更改，请同步知识库后重试 ({rel_path})"),
        ));
    }

    let mut base_pages: Option<Vec<PdfPageMarkdown>> = None;
    let mut target_pages: Option<Vec<usize>> = None;
    let mut page_ranges_arg: Option<String> = None;

    if fill_missing_only {
        if let Ok(existing_pages) = ensure_fast_pages_cached(root, rel_path, hash, Some(&bytes)) {
            let summary = summarize_pages(&existing_pages);
            if summary.is_complete {
                let extracted = chunk_pdf_pages(rel_path, &existing_pages, summary.parse_tier);
                store
                    .upgrade_pdf_content(
                        hash,
                        &extracted.raw_text,
                        extracted.parse_tier,
                        "indexed",
                        &extracted.chunks,
                    )
                    .map_err(|e| RagError::new("store_error", e))?;
                store
                    .upsert_ocr_job(hash, rel_path, None, "done", None)
                    .map_err(|e| RagError::new("store_error", e))?;
                return Ok(());
            }
            if !summary.missing_pages.is_empty() && summary.missing_pages.len() < summary.total_pages
            {
                page_ranges_arg = Some(format_page_ranges(&summary.missing_pages));
                target_pages = Some(summary.missing_pages);
                base_pages = Some(existing_pages);
            }
        }
    }

    let expected_pages = target_pages
        .as_ref()
        .map(|p| p.len())
        .or_else(|| pdf_total_pages(&bytes));
    on_progress(OcrProgressEvent::Started {
        rel_path: rel_path.to_owned(),
        total_pages: expected_pages,
    });

    let file_name = Path::new(rel_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.pdf", &hash[..8.min(hash.len())]));

    let optional_payload = json!({
        "useDocOrientationClassify": false,
        "useDocUnwarping": false,
        "useChartRecognition": false,
    })
    .to_string();
    let boundary = format!("----FolioPaddleOcr{}", &hash[..12.min(hash.len())]);
    let body = build_multipart_body(
        &boundary,
        &optional_payload,
        page_ranges_arg.as_deref(),
        &file_name,
        &bytes,
    );
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let auth = format!("bearer {token}");

    let resp = http::send(RequestOptions {
        https: true,
        host: PADDLEOCR_HOST,
        port: 443,
        method: "POST",
        path: PADDLEOCR_JOBS_PATH,
        headers: &[
            ("Authorization", &auth),
            ("Content-Type", &content_type),
        ],
        body: &body,
        connect_timeout_ms: 10_000,
        receive_timeout_ms: 180_000,
        use_system_proxy: true,
        use_env_proxy: true,
        max_response_bytes: 4 * 1024 * 1024,
        label: "PaddleOCR",
    })
    .map_err(|e| RagError::new("ocr_network", e))?;

    if !(200..300).contains(&resp.status) {
        let preview = String::from_utf8_lossy(&resp.body);
        let snippet: String = preview.chars().take(200).collect();
        return Err(RagError::new(
            "ocr_http",
            format!("提交 OCR 任务失败 (HTTP {}): {snippet}", resp.status),
        ));
    }

    let parsed: SubmitJobResponse = serde_json::from_slice(&resp.body)
        .map_err(|e| RagError::new("ocr_parse", format!("解析 OCR 响应失败: {e}")))?;
    let job_id = parsed
        .data
        .map(|d| d.job_id)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            let msg = parsed
                .msg
                .unwrap_or_else(|| "OCR 响应缺少 jobId".to_owned());
            RagError::new("ocr_api", msg)
        })?;

    store
        .upsert_ocr_job(hash, rel_path, Some(&job_id), "polling", None)
        .map_err(|e| RagError::new("store_error", e))?;

    let jsonl_url = poll_job_until_done(&auth, &job_id, rel_path, expected_pages, cancelled, on_progress)?;
    let jsonl_text = download_jsonl(&jsonl_url)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(RagError::cancelled());
    }
    let out_dir = cache_dir(root, hash);
    match (base_pages.as_deref(), target_pages.as_deref()) {
        (Some(existing), Some(targets)) => {
            merge_paddleocr_jsonl_into_cache(&jsonl_text, existing, targets, &out_dir)
                .map_err(|e| RagError::new("ocr_parse", e))?;
        }
        _ => {
            parse_paddleocr_jsonl_to_cache_checked(&jsonl_text, expected_pages, &out_dir)
                .map_err(|e| RagError::new("ocr_parse", e))?;
        }
    }

    if let Some(extracted) = load_cached_pdf(root, rel_path, hash) {
        let status = if extracted.needs_ocr {
            "needs_ocr"
        } else {
            "indexed"
        };
        store
            .upgrade_pdf_content(
                hash,
                &extracted.raw_text,
                extracted.parse_tier,
                status,
                &extracted.chunks,
            )
            .map_err(|e| RagError::new("store_error", e))?;
    }
    store
        .upsert_ocr_job(hash, rel_path, Some(&job_id), "done", None)
        .map_err(|e| RagError::new("store_error", e))?;

    Ok(())
}

pub fn build_multipart_body(
    boundary: &str,
    optional_payload_json: &str,
    page_ranges: Option<&str>,
    file_name: &str,
    file_bytes: &[u8],
) -> Vec<u8> {
    let safe_name = file_name.replace('"', "_");
    let mut out = Vec::with_capacity(file_bytes.len() + 512);
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{PADDLEOCR_MODEL}\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"optionalPayload\"\r\n\r\n{optional_payload_json}\r\n"
        )
        .as_bytes(),
    );
    if let Some(ranges) = page_ranges.filter(|r| !r.is_empty()) {
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"pageRanges\"\r\n\r\n{ranges}\r\n"
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe_name}\"\r\nContent-Type: application/pdf\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(file_bytes);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

fn poll_job_until_done(
    auth: &str,
    job_id: &str,
    rel_path: &str,
    expected_pages: Option<usize>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(OcrProgressEvent),
) -> Result<String, RagError> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    let poll_path = format!("{PADDLEOCR_JOBS_PATH}/{job_id}");

    while Instant::now() < deadline {
        sleep_cancellable(Duration::from_secs(5), cancelled)?;

        let resp = http::send(RequestOptions {
            https: true,
            host: PADDLEOCR_HOST,
            port: 443,
            method: "GET",
            path: &poll_path,
            headers: &[("Authorization", auth)],
            body: &[],
            connect_timeout_ms: 5_000,
            receive_timeout_ms: 30_000,
            use_system_proxy: true,
            use_env_proxy: true,
            max_response_bytes: 4 * 1024 * 1024,
            label: "PaddleOCR",
        })
        .map_err(|e| RagError::new("ocr_network", e))?;

        if !(200..300).contains(&resp.status) {
            continue;
        }
        let Ok(poll) = serde_json::from_slice::<PollJobResponse>(&resp.body) else {
            continue;
        };
        let Some(data) = poll.data else {
            continue;
        };

        match data.state.as_str() {
            "running" => {
                if let Some(progress) = data.extract_progress {
                    on_progress(OcrProgressEvent::PageProgress {
                        rel_path: rel_path.to_owned(),
                        extracted_pages: progress.extracted_pages.unwrap_or(0),
                        total_pages: progress.total_pages.or(expected_pages),
                    });
                }
            }
            "done" => {
                let json_url = data
                    .result_url
                    .and_then(|r| r.json_url)
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| RagError::new("ocr_parse", "OCR 任务已完成但缺少结果 URL"))?;
                return Ok(json_url);
            }
            "failed" => {
                let reason = data.error_msg.unwrap_or_else(|| "OCR 任务失败".to_owned());
                return Err(RagError::new("ocr_failed", reason));
            }
            _ => {}
        }
    }

    Err(RagError::new("ocr_timeout", "OCR 解析轮询超时。"))
}

fn download_jsonl(url: &str) -> Result<String, RagError> {
    let ep = Endpoint::parse(url).map_err(|e| RagError::new("ocr_url", e))?;
    let parsed = url::Url::parse(url).map_err(|e| RagError::new("ocr_url", e.to_string()))?;
    let path_and_query = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let resp = http::send(RequestOptions {
        https: ep.https,
        host: &ep.host,
        port: ep.port,
        method: "GET",
        path: &path_and_query,
        headers: &[],
        body: &[],
        connect_timeout_ms: 10_000,
        receive_timeout_ms: 180_000,
        use_system_proxy: true,
        use_env_proxy: true,
        max_response_bytes: 128 * 1024 * 1024,
        label: "PaddleOCR-Result",
    })
    .map_err(|e| RagError::new("ocr_download", e))?;

    if !(200..300).contains(&resp.status) {
        return Err(RagError::new(
            "ocr_download",
            format!("下载 OCR 结果失败 (HTTP {})", resp.status),
        ));
    }
    String::from_utf8(resp.body)
        .map_err(|e| RagError::new("ocr_parse", format!("OCR 结果编码异常: {e}")))
}

#[cfg(test)]
pub fn parse_paddleocr_jsonl_to_cache(jsonl_text: &str, out_dir: &Path) -> Result<(), String> {
    parse_paddleocr_jsonl_to_cache_checked(jsonl_text, None, out_dir)
}

fn extract_jsonl_page_markdowns(jsonl_text: &str) -> Result<Vec<String>, String> {
    let mut page_texts = Vec::new();
    for raw_line in jsonl_text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let val: Value =
            serde_json::from_str(trimmed).map_err(|e| format!("解析 OCR JSONL 失败: {e}"))?;
        let Some(pages) = val
            .pointer("/result/layoutParsingResults")
            .and_then(Value::as_array)
        else {
            continue;
        };

        for page in pages {
            let md_text = page
                .pointer("/markdown/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            page_texts.push(md_text.to_owned());
        }
    }
    Ok(page_texts)
}

pub fn parse_paddleocr_jsonl_to_cache_checked(
    jsonl_text: &str,
    expected_total_pages: Option<usize>,
    out_dir: &Path,
) -> Result<(), String> {
    let raw_pages = extract_jsonl_page_markdowns(jsonl_text)?;
    let page_idx = raw_pages.len();

    if let Some(expected) = expected_total_pages
        && page_idx != expected
    {
        return Err(format!(
            "OCR 返回页数与原文不一致 ({page_idx}/{expected})，保留原快速索引状态"
        ));
    }

    let page_texts: Vec<PdfPageMarkdown> = raw_pages
        .into_iter()
        .enumerate()
        .map(|(idx, markdown)| PdfPageMarkdown {
            page: idx + 1,
            markdown,
            source: Some("ocr".to_owned()),
            needs_ocr: false,
        })
        .collect();

    if join_pages_markdown(&page_texts).trim().is_empty() {
        return Err("OCR 结果中未提取到有效文本".to_owned());
    }

    let anchors = build_anchors(&page_texts);
    write_pages_cache(out_dir, &page_texts, Some(&anchors))
}

pub fn merge_paddleocr_jsonl_into_cache(
    jsonl_text: &str,
    existing_pages: &[PdfPageMarkdown],
    target_pages: &[usize],
    out_dir: &Path,
) -> Result<(), String> {
    let ocr_pages = extract_jsonl_page_markdowns(jsonl_text)?;
    // OCR rows are positional. Extra rows are as ambiguous as missing rows.
    if ocr_pages.len() != target_pages.len() {
        return Err(format!(
            "OCR 返回页数与请求范围不一致 ({}/{})，保留当前索引状态",
            ocr_pages.len(),
            target_pages.len()
        ));
    }

    let mut merged = existing_pages.to_vec();
    for (&page_num, md_text) in target_pages.iter().zip(ocr_pages.into_iter()) {
        if let Some(slot) = merged.iter_mut().find(|p| p.page == page_num) {
            slot.markdown = md_text;
            slot.source = Some("ocr".to_owned());
            slot.needs_ocr = false;
        } else {
            merged.push(PdfPageMarkdown {
                page: page_num,
                markdown: md_text,
                source: Some("ocr".to_owned()),
                needs_ocr: false,
            });
        }
    }
    merged.sort_by_key(|p| p.page);

    if join_pages_markdown(&merged).trim().is_empty() {
        return Err("OCR 结果中未提取到有效文本".to_owned());
    }

    let anchors = build_anchors(&merged);
    write_pages_cache(out_dir, &merged, Some(&anchors))
}

pub fn export_pdf_markdown_to_sibling(
    root: &Path,
    rel_path: &str,
    content_hash: &str,
) -> Result<String, String> {
    let dir = cache_dir(root, content_hash);
    let pages = ensure_fast_pages_cached(root, rel_path, content_hash, None)?;
    let summary = summarize_pages(&pages);
    if !summary.is_complete {
        return Err(format!(
            "文档尚有 {} 页未完成 OCR 补全 ({rel_path})，请先补全后再导出",
            summary.missing_pages.len()
        ));
    }

    let markdown = match fs::read_to_string(dir.join("full.md")) {
        Ok(md) if !md.trim().is_empty() => md,
        _ => join_pages_markdown(&pages),
    };
    if markdown.trim().is_empty() {
        return Err(format!("解析缓存为空 ({rel_path})"));
    }
    let normalized = rel_path.replace('\\', "/");
    let target_rel = match normalized.rsplit_once('/') {
        Some((dir, file_name)) => {
            let stem = file_name
                .strip_suffix(".pdf")
                .or_else(|| file_name.strip_suffix(".PDF"))
                .unwrap_or(file_name);
            format!("{dir}/{stem}_folio-parse.md")
        }
        None => {
            let stem = normalized
                .strip_suffix(".pdf")
                .or_else(|| normalized.strip_suffix(".PDF"))
                .unwrap_or(&normalized);
            format!("{stem}_folio-parse.md")
        }
    };
    let dest = root.join(target_rel.replace('/', "\\"));
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败: {e}"))?;
    }
    fs::write(&dest, markdown).map_err(|e| format!("写入 Markdown 失败: {e}"))?;
    Ok(target_rel)
}

pub fn read_pdf_markdown(
    root: &Path,
    rel_path: &str,
    start_page: Option<usize>,
    end_page: Option<usize>,
) -> Result<PdfReadResult, RagError> {
    let abs = crate::files::project_path(root, rel_path)
        .map_err(|e| RagError::new("invalid_path", e.to_string()))?;
    let bytes = fs::read(&abs)
        .map_err(|e| RagError::new("io_error", format!("读取 PDF 失败 ({rel_path}): {e}")))?;
    let norm_rel = rel_path.trim().replace('\\', "/");
    let content_hash = sha256_hex(&bytes);
    let cache = cache_dir(root, &content_hash);
    let full_md_path = cache.join("full.md");
    let req_start = start_page.unwrap_or(1).max(1);
    let req_end = end_page
        .unwrap_or_else(|| req_start.saturating_add(4))
        .max(req_start)
        .min(req_start.saturating_add(19));

    if read_cached_pages_file(&cache).is_none()
        && full_md_path.exists()
        && let Ok(full_md) = fs::read_to_string(&full_md_path)
        && !full_md.trim().is_empty()
    {
        let total_pages = pdf_total_pages(&bytes).unwrap_or(1).max(1);
        return Ok(PdfReadResult {
            path: norm_rel,
            source: "paddleocr_cache",
            parse_tier: "accurate",
            markdown_cache: Some(format!(".folio/pdf-cache/{content_hash}/full.md")),
            total_pages,
            start_page: 1,
            end_page: total_pages,
            needs_ocr: false,
            pages: vec![PdfPageMarkdown {
                page: 1,
                markdown: full_md,
                source: Some("ocr".to_owned()),
                needs_ocr: false,
            }],
        });
    }

    let cached_pages = ensure_fast_pages_cached(root, &norm_rel, &content_hash, Some(&bytes))
        .map_err(|e| RagError::new("pdf_parse", e))?;
    let summary = summarize_pages(&cached_pages);
    let total_pages = summary.total_pages.max(1);
    let start = req_start.min(total_pages);
    let end = req_end.min(total_pages);
    let source = match summary.parse_tier {
        "accurate" => "paddleocr_cache",
        "hybrid" => "hybrid_cache",
        _ => "pdf_inspector",
    };
    let pages = cached_pages
        .into_iter()
        .filter(|p| p.page >= start && p.page <= end)
        .map(|mut p| {
            if p.needs_ocr {
                p.markdown = if p.markdown.trim().is_empty() {
                    format!("[第 {} 页尚未 OCR 补全]", p.page)
                } else {
                    format!("[第 {} 页存在异常或扫描内容，尚未 OCR 补全]\n{}", p.page, p.markdown)
                };
            }
            p
        })
        .collect();

    Ok(PdfReadResult {
        path: norm_rel,
        source,
        parse_tier: summary.parse_tier,
        markdown_cache: full_md_path
            .exists()
            .then(|| format!(".folio/pdf-cache/{content_hash}/full.md")),
        total_pages,
        start_page: start,
        end_page: end,
        needs_ocr: !summary.is_complete,
        pages,
    })
}

fn normalize_anchor(s: &str) -> String {
    s.trim()
        .trim_start_matches('#')
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ")
}

fn match_chunk_page(chunk_text: &str, anchors: &[PageAnchor], min_page: usize) -> Option<usize> {
    let body = chunk_text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n").map(|(_, after)| after))
        .unwrap_or(chunk_text);
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    anchors
        .iter()
        .filter(|a| a.page_num >= min_page)
        .find(|a| compact.contains(&a.snippet))
        .or_else(|| anchors.iter().find(|a| compact.contains(&a.snippet)))
        .map(|a| a.page_num)
}

fn sleep_cancellable(total: Duration, cancelled: &AtomicBool) -> Result<(), RagError> {
    let start = Instant::now();
    while start.elapsed() < total {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RagError::cancelled());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

#[derive(Deserialize)]
struct SubmitJobResponse {
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<SubmitJobData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitJobData {
    #[serde(default)]
    job_id: String,
}

#[derive(Deserialize)]
struct PollJobResponse {
    #[serde(default)]
    data: Option<PollJobData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollJobData {
    state: String,
    #[serde(default)]
    extract_progress: Option<PollExtractProgress>,
    #[serde(default)]
    result_url: Option<PollResultUrl>,
    #[serde(default)]
    error_msg: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollExtractProgress {
    #[serde(default)]
    total_pages: Option<usize>,
    #[serde(default)]
    extracted_pages: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollResultUrl {
    #[serde(default)]
    json_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_paddleocr_jsonl_caches_markdown_and_maps_page_numbers() {
        let p1 = json!({
            "result": {
                "layoutParsingResults": [{
                    "markdown": { "text": "# Introduction\n\nGradient descent converges linearly under strong convexity." }
                }]
            }
        });
        let p2 = json!({
            "result": {
                "layoutParsingResults": [{
                    "markdown": { "text": "## KKT Conditions\n\n$$\\nabla f(x^*) + \\lambda \\nabla g(x^*) = 0$$" }
                }]
            }
        });
        let jsonl = format!("{p1}\n{p2}\n");

        let dir = tempdir().unwrap();
        let hash = "deadbeef12345678";
        let out_dir = cache_dir(dir.path(), hash);
        parse_paddleocr_jsonl_to_cache(&jsonl, &out_dir).unwrap();

        let extracted = load_cached_pdf(dir.path(), "papers/opt.pdf", hash).unwrap();
        assert_eq!(extracted.parse_tier, "accurate");
        assert_eq!(extracted.chunks.len(), 2);
        assert_eq!(extracted.chunks[0].page_num, Some(1));
        assert_eq!(extracted.chunks[1].page_num, Some(2));

        assert!(
            parse_paddleocr_jsonl_to_cache_checked(&jsonl, Some(3), &cache_dir(dir.path(), "incomplete"))
                .is_err()
        );

        let pdf_dir = dir.path().join("papers");
        fs::create_dir_all(&pdf_dir).unwrap();
        let dummy_pdf = b"%PDF-1.4\n% page1 page2";
        fs::write(pdf_dir.join("opt.pdf"), dummy_pdf).unwrap();
        let real_hash = sha256_hex(dummy_pdf);
        parse_paddleocr_jsonl_to_cache_checked(&jsonl, Some(2), &cache_dir(dir.path(), &real_hash)).unwrap();

        let exported_rel = export_pdf_markdown_to_sibling(dir.path(), "papers/opt.pdf", &real_hash).unwrap();
        assert_eq!(exported_rel, "papers/opt_folio-parse.md");
        let exported = fs::read_to_string(dir.path().join("papers/opt_folio-parse.md")).unwrap();
        assert!(exported.contains("Gradient descent"));

        let read_res = read_pdf_markdown(dir.path(), "papers/opt.pdf", Some(2), Some(2)).unwrap();
        assert_eq!(read_res.source, "paddleocr_cache");
        assert_eq!(read_res.total_pages, 2);
        assert_eq!(read_res.pages.len(), 1);
        assert_eq!(read_res.pages[0].page, 2);
        assert!(read_res.pages[0].markdown.contains("KKT Conditions"));
    }

    #[test]
    fn multipart_body_contains_model_payload_and_pdf_bytes() {
        let body = build_multipart_body("B123", r#"{"useDocUnwarping":false}"#, Some("2,4-6"), "scan.pdf", b"%PDF-1.4");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"model\"\r\n\r\nPaddleOCR-VL-1.6"));
        assert!(text.contains("name=\"optionalPayload\"\r\n\r\n{\"useDocUnwarping\":false}"));
        assert!(text.contains("name=\"pageRanges\"\r\n\r\n2,4-6"));
        assert!(text.contains("filename=\"scan.pdf\""));
        assert!(text.contains("%PDF-1.4"));
    }

    #[test]
    fn format_page_ranges_merges_contiguous_runs() {
        assert_eq!(format_page_ranges(&[]), "");
        assert_eq!(format_page_ranges(&[5, 2, 4, 3, 2, 9]), "2-5,9");
        assert_eq!(format_page_ranges(&[1, 3, 5]), "1,3,5");
    }

    #[test]
    fn fast_and_hybrid_caches_support_export_and_partial_ai_read() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("papers")).unwrap();
        let pdf_bytes = b"%PDF-1.4 dummy";
        fs::write(root.join("papers/notes.pdf"), pdf_bytes).unwrap();
        let hash = sha256_hex(pdf_bytes);
        let out_dir = cache_dir(root, &hash);

        // 1. Complete Fast cache -> exportable & readable without OCR.
        let fast_complete = vec![
            PdfPageMarkdown {
                page: 1,
                markdown: "# Chapter 1\n\nDigital text.".to_owned(),
                source: Some("fast".to_owned()),
                needs_ocr: false,
            },
            PdfPageMarkdown {
                page: 2,
                markdown: "Second page.".to_owned(),
                source: Some("fast".to_owned()),
                needs_ocr: false,
            },
        ];
        write_pages_cache(&out_dir, &fast_complete, None).unwrap();
        let exported_rel = export_pdf_markdown_to_sibling(root, "papers/notes.pdf", &hash).unwrap();
        assert_eq!(exported_rel, "papers/notes_folio-parse.md");
        let read_fast = read_pdf_markdown(root, "papers/notes.pdf", Some(1), Some(2)).unwrap();
        assert_eq!(read_fast.parse_tier, "fast_pdf");
        assert!(!read_fast.needs_ocr);

        // 2. Partial Fast cache (page 2 needs OCR) -> export blocked, partial AI read marks page 2.
        let partial = vec![
            PdfPageMarkdown {
                page: 1,
                markdown: "Page one ok.".to_owned(),
                source: Some("fast".to_owned()),
                needs_ocr: false,
            },
            PdfPageMarkdown {
                page: 2,
                markdown: String::new(),
                source: None,
                needs_ocr: true,
            },
        ];
        write_pages_cache(&out_dir, &partial, None).unwrap();
        assert!(export_pdf_markdown_to_sibling(root, "papers/notes.pdf", &hash).is_err());
        let read_partial = read_pdf_markdown(root, "papers/notes.pdf", Some(1), Some(2)).unwrap();
        assert!(read_partial.needs_ocr);
        assert!(read_partial.pages[1].needs_ocr);
        assert!(read_partial.pages[1].markdown.contains("尚未 OCR 补全"));

        // 3. Merge missing page 2 from PaddleOCR JSONL -> hybrid tier, complete & exportable.
        let p2 = json!({
            "result": {
                "layoutParsingResults": [{
                    "markdown": { "text": "Scanned page two recovered." }
                }]
            }
        });
        assert!(
            merge_paddleocr_jsonl_into_cache(&format!("{p2}\n{p2}"), &partial, &[2], &out_dir)
                .is_err()
        );
        assert!(
            read_pdf_markdown(root, "papers/notes.pdf", Some(2), Some(2)).unwrap().needs_ocr
        );
        merge_paddleocr_jsonl_into_cache(&p2.to_string(), &partial, &[2], &out_dir).unwrap();
        let read_hybrid = read_pdf_markdown(root, "papers/notes.pdf", Some(1), Some(2)).unwrap();
        assert_eq!(read_hybrid.parse_tier, "hybrid");
        assert!(!read_hybrid.needs_ocr);
        assert_eq!(read_hybrid.pages[1].source.as_deref(), Some("ocr"));
        assert!(export_pdf_markdown_to_sibling(root, "papers/notes.pdf", &hash).is_ok());
    }
}
