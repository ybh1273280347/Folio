//! PaddleOCR-VL async Jobs API (`paddleocr.aistudio-app.com`) and `.folio/pdf-cache/<sha256>/` persistence.

use super::{
    AccuratePdfMode, RagConfig, RagError,
    chunker::{ExtractedDocument, chunk_text_document},
    client::Endpoint,
    store::RagStore,
};
use crate::http::{self, RequestOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const PADDLEOCR_HOST: &str = "paddleocr.aistudio-app.com";
const PADDLEOCR_JOBS_PATH: &str = "/api/v2/ocr/jobs";
const PADDLEOCR_MODEL: &str = "PaddleOCR-VL-1.6";
const POLL_TIMEOUT: Duration = Duration::from_secs(600);

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

pub fn load_cached_pdf(
    root: &Path,
    rel_path: &str,
    content_hash: &str,
) -> Option<ExtractedDocument> {
    let dir = cache_dir(root, content_hash);
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
        if let Some(cached) = load_cached_pdf(root, &rel_path, &hash) {
            store
                .upgrade_pdf_content(&hash, &cached.raw_text, &cached.chunks)
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

    for (rel_path, hash) in to_upload {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RagError::cancelled());
        }
        match process_single_pdf(root, store, &token, &rel_path, &hash, cancelled) {
            Ok(()) => stats.upgraded_pdfs += 1,
            Err(err) if err.code == "cancelled" => return Err(err),
            Err(err) => {
                let _ = store.upsert_ocr_job(&hash, &rel_path, None, "failed", Some(&err.message));
                stats.failed_pdfs.push(format!("{rel_path}: {}", err.message));
            }
        }
    }

    Ok(stats)
}

fn process_single_pdf(
    root: &Path,
    store: &mut RagStore,
    token: &str,
    rel_path: &str,
    hash: &str,
    cancelled: &AtomicBool,
) -> Result<(), RagError> {
    let abs = root.join(rel_path.replace('/', "\\"));
    let bytes = fs::read(&abs)
        .map_err(|e| RagError::new("io_error", format!("读取 PDF 失败 ({rel_path}): {e}")))?;
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
    let body = build_multipart_body(&boundary, &optional_payload, &file_name, &bytes);
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

    let jsonl_url = poll_job_until_done(&auth, &job_id, cancelled)?;
    let jsonl_text = download_jsonl(&jsonl_url)?;
    let out_dir = cache_dir(root, hash);
    parse_paddleocr_jsonl_to_cache(&jsonl_text, &out_dir)
        .map_err(|e| RagError::new("ocr_parse", e))?;

    if let Some(extracted) = load_cached_pdf(root, rel_path, hash) {
        store
            .upgrade_pdf_content(hash, &extracted.raw_text, &extracted.chunks)
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
    cancelled: &AtomicBool,
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

pub fn parse_paddleocr_jsonl_to_cache(jsonl_text: &str, out_dir: &Path) -> Result<(), String> {
    let mut full_md = String::new();
    let mut anchors: Vec<PageAnchor> = Vec::new();
    let mut page_idx = 0usize;

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
            let page_num = page_idx + 1;
            page_idx += 1;
            let md_text = page
                .pointer("/markdown/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if md_text.is_empty() {
                continue;
            }
            if !full_md.is_empty() {
                full_md.push_str("\n\n");
            }
            full_md.push_str(md_text);

            for line in md_text.lines() {
                let norm = normalize_anchor(line);
                if norm.chars().count() >= 8 {
                    anchors.push(PageAnchor {
                        page_num,
                        snippet: norm,
                    });
                }
            }
        }
    }

    if full_md.trim().is_empty() {
        return Err("OCR 结果中未提取到有效文本".to_owned());
    }

    fs::create_dir_all(out_dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;
    fs::write(out_dir.join("full.md"), full_md).map_err(|e| e.to_string())?;
    if !anchors.is_empty() {
        let json_bytes = serde_json::to_vec(&anchors).map_err(|e| e.to_string())?;
        fs::write(out_dir.join("pages.json"), json_bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
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
    result_url: Option<PollResultUrl>,
    #[serde(default)]
    error_msg: Option<String>,
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
        parse_paddleocr_jsonl_to_cache(&jsonl, &cache_dir(dir.path(), hash)).unwrap();

        let extracted = load_cached_pdf(dir.path(), "papers/opt.pdf", hash).unwrap();
        assert_eq!(extracted.parse_tier, "accurate");
        assert_eq!(extracted.chunks.len(), 2);
        assert_eq!(extracted.chunks[0].page_num, Some(1));
        assert_eq!(extracted.chunks[1].page_num, Some(2));
    }

    #[test]
    fn multipart_body_contains_model_payload_and_pdf_bytes() {
        let body = build_multipart_body("B123", r#"{"useDocUnwarping":false}"#, "scan.pdf", b"%PDF-1.4");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"model\"\r\n\r\nPaddleOCR-VL-1.6"));
        assert!(text.contains("name=\"optionalPayload\"\r\n\r\n{\"useDocUnwarping\":false}"));
        assert!(text.contains("filename=\"scan.pdf\""));
        assert!(text.contains("%PDF-1.4"));
    }
}
