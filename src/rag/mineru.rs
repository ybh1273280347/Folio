//! MinerU API v4 offline PDF parsing and `.folio/pdf-cache/<sha256>/` persistence.

use super::{
    RagConfig, RagError,
    chunker::{ExtractedDocument, chunk_text_document},
    client::Endpoint,
    store::RagStore,
};
use crate::http::{self, RequestOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use zip::ZipArchive;

const MINERU_HOST: &str = "mineru.net";
const MINERU_BATCH_SIZE: usize = 20;
const POLL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PageAnchor {
    pub page_num: usize,
    pub snippet: String,
}

#[derive(Default)]
pub struct MinerUUpgradeStats {
    pub upgraded_pdfs: usize,
    pub failed_pdfs: Vec<String>,
}

pub fn cache_dir(root: &Path, content_hash: &str) -> PathBuf {
    root.join(".folio").join("pdf-cache").join(content_hash)
}

pub fn resolve_api_key(config: &RagConfig) -> String {
    let trimmed = config.mineru_api_key.trim();
    if !trimmed.is_empty() {
        return trimmed.to_owned();
    }
    std::env::var("MINERU_API_KEY")
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
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
    extracted.parse_tier = "mineru";

    let anchors: Vec<PageAnchor> = fs::read(dir.join("pages.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    if !anchors.is_empty() {
        let mut last_page = 1usize;
        for chunk in &mut extracted.chunks {
            chunk.line_start = None;
            chunk.line_end = None;
            if let Some(page) = match_chunk_page(&chunk.chunk_text, &anchors, last_page) {
                last_page = page;
                chunk.page_num = Some(page);
            } else {
                chunk.page_num = Some(last_page);
            }
        }
    } else {
        for chunk in &mut extracted.chunks {
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
    mode: super::AccuratePdfMode,
    paths: &[String],
    cancelled: &AtomicBool,
) -> Result<MinerUUpgradeStats, RagError> {
    let candidates = store
        .pdf_upgrade_candidates(mode, paths)
        .map_err(|e| RagError::new("store_error", e))?;
    if candidates.is_empty() {
        return Ok(MinerUUpgradeStats::default());
    }

    let mut stats = MinerUUpgradeStats::default();
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

    let api_key = resolve_api_key(config);
    if api_key.is_empty() {
        return Err(RagError::new(
            "accurate_pdf_unconfigured",
            "尚未配置深度 PDF 解析密钥，请在「设置 → API 服务」填写后重试。",
        ));
    }

    for batch in to_upload.chunks(MINERU_BATCH_SIZE) {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RagError::cancelled());
        }
        let batch_stats = process_batch(root, store, &api_key, batch, cancelled)?;
        stats.upgraded_pdfs += batch_stats.upgraded_pdfs;
        stats.failed_pdfs.extend(batch_stats.failed_pdfs);
    }

    Ok(stats)
}

fn process_batch(
    root: &Path,
    store: &mut RagStore,
    api_key: &str,
    batch: &[(String, String)],
    cancelled: &AtomicBool,
) -> Result<MinerUUpgradeStats, RagError> {
    let mut files_payload = Vec::with_capacity(batch.len());
    let mut hash_by_name: HashMap<String, (String, String)> = HashMap::new();

    for (rel_path, hash) in batch {
        let file_name = Path::new(rel_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{}.pdf", &hash[..8]));
        let unique_name = if hash_by_name.contains_key(&file_name) {
            format!("{}_{}", &hash[..8], file_name)
        } else {
            file_name
        };
        files_payload.push(json!({
            "name": unique_name,
            "data_id": hash,
        }));
        hash_by_name.insert(unique_name, (rel_path.clone(), hash.clone()));
    }

    let req_body = serde_json::to_vec(&json!({
        "enable_formula": true,
        "enable_table": true,
        "model_version": "vlm",
        "files": files_payload,
    }))
    .map_err(|e| RagError::new("mineru_error", e.to_string()))?;

    let auth = format!("Bearer {api_key}");
    let resp = http::send(RequestOptions {
        https: true,
        host: MINERU_HOST,
        port: 443,
        method: "POST",
        path: "/api/v4/file-urls/batch",
        headers: &[
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        body: &req_body,
        connect_timeout_ms: 5_000,
        receive_timeout_ms: 30_000,
        use_system_proxy: true,
        use_env_proxy: true,
        max_response_bytes: 4 * 1024 * 1024,
        label: "MinerU",
    })
    .map_err(|e| RagError::new("mineru_network", e))?;

    if !(200..300).contains(&resp.status) {
        return Err(RagError::new(
            "mineru_http",
            format!("MinerU 申请上传链接失败 (HTTP {})", resp.status),
        ));
    }
    let parsed: BatchUrlResponse = serde_json::from_slice(&resp.body)
        .map_err(|e| RagError::new("mineru_parse", format!("解析 MinerU 响应失败: {e}")))?;
    if parsed.code != 0 {
        return Err(RagError::new(
            "mineru_api",
            parsed.msg.unwrap_or_else(|| format!("MinerU 错误码 {}", parsed.code)),
        ));
    }

    let data = parsed
        .data
        .ok_or_else(|| RagError::new("mineru_parse", "MinerU 响应缺少 data 字段"))?;
    if data.file_urls.len() != batch.len() {
        return Err(RagError::new(
            "mineru_parse",
            "MinerU 返回上传 URL 数量与请求文件数不一致",
        ));
    }

    for ((rel_path, hash), upload_url) in batch.iter().zip(&data.file_urls) {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RagError::cancelled());
        }
        let abs = root.join(rel_path.replace('/', "\\"));
        let bytes = fs::read(&abs)
            .map_err(|e| RagError::new("io_error", format!("读取 PDF 失败 ({rel_path}): {e}")))?;
        upload_presigned_binary(upload_url, &bytes)?;
        store
            .upsert_mineru_job(hash, rel_path, Some(&data.batch_id), "polling", None)
            .map_err(|e| RagError::new("store_error", e))?;
    }

    poll_and_consume_batch(root, store, &auth, &data.batch_id, batch, cancelled)
}

fn upload_presigned_binary(upload_url: &str, bytes: &[u8]) -> Result<(), RagError> {
    let ep = Endpoint::parse(upload_url).map_err(|e| RagError::new("mineru_url", e))?;
    let path_and_query = match url::Url::parse(upload_url) {
        Ok(parsed) => match parsed.query() {
            Some(q) => format!("{}?{q}", parsed.path()),
            None => parsed.path().to_owned(),
        },
        Err(e) => return Err(RagError::new("mineru_url", e.to_string())),
    };

    // Presigned OSS PUT must NOT include a Content-Type header, or signature validation fails with 403.
    let resp = http::send(RequestOptions {
        https: ep.https,
        host: &ep.host,
        port: ep.port,
        method: "PUT",
        path: &path_and_query,
        headers: &[],
        body: bytes,
        connect_timeout_ms: 10_000,
        receive_timeout_ms: 180_000,
        use_system_proxy: true,
        use_env_proxy: true,
        max_response_bytes: 1024 * 1024,
        label: "MinerU-OSS",
    })
    .map_err(|e| RagError::new("mineru_upload", e))?;

    if !(200..300).contains(&resp.status) {
        return Err(RagError::new(
            "mineru_upload",
            format!("MinerU 文件上传失败 (HTTP {})", resp.status),
        ));
    }
    Ok(())
}

fn poll_and_consume_batch(
    root: &Path,
    store: &mut RagStore,
    auth: &str,
    batch_id: &str,
    batch: &[(String, String)],
    cancelled: &AtomicBool,
) -> Result<MinerUUpgradeStats, RagError> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    let poll_path = format!("/api/v4/extract-results/batch/{batch_id}");
    let mut completed: HashMap<String, bool> = HashMap::new();
    let mut stats = MinerUUpgradeStats::default();

    while Instant::now() < deadline {
        sleep_cancellable(Duration::from_secs(5), cancelled)?;

        let resp = http::send(RequestOptions {
            https: true,
            host: MINERU_HOST,
            port: 443,
            method: "GET",
            path: &poll_path,
            headers: &[("Authorization", auth)],
            body: &[],
            connect_timeout_ms: 5_000,
            receive_timeout_ms: 30_000,
            use_system_proxy: true,
            use_env_proxy: true,
            max_response_bytes: 8 * 1024 * 1024,
            label: "MinerU",
        })
        .map_err(|e| RagError::new("mineru_network", e))?;

        if !(200..300).contains(&resp.status) {
            continue;
        }
        let Ok(poll) = serde_json::from_slice::<PollBatchResponse>(&resp.body) else {
            continue;
        };
        if poll.code != 0 {
            continue;
        }
        let Some(data) = poll.data else {
            continue;
        };

        for item in data.extract_result {
            let hash = item.data_id.clone().unwrap_or_default();
            let Some((rel_path, _)) = batch.iter().find(|(_, h)| *h == hash) else {
                continue;
            };
            if completed.contains_key(&hash) {
                continue;
            }
            match item.state.as_str() {
                "done" => {
                    if let Some(zip_url) = item.full_zip_url.as_deref() {
                        let zip_bytes = download_zip(zip_url)?;
                        let dir = cache_dir(root, &hash);
                        extract_zip_to_cache(&zip_bytes, &dir)
                            .map_err(|e| RagError::new("mineru_zip", e))?;
                        if let Some(extracted) = load_cached_pdf(root, rel_path, &hash) {
                            store
                                .upgrade_pdf_content(&hash, &extracted.raw_text, &extracted.chunks)
                                .map_err(|e| RagError::new("store_error", e))?;
                        }
                        store
                            .upsert_mineru_job(&hash, rel_path, Some(batch_id), "done", None)
                            .map_err(|e| RagError::new("store_error", e))?;
                        stats.upgraded_pdfs += 1;
                    } else {
                        stats.failed_pdfs.push(rel_path.clone());
                    }
                    completed.insert(hash, true);
                }
                "failed" | "error" => {
                    let msg = item.err_msg.unwrap_or_else(|| "MinerU 解析失败".to_owned());
                    let _ = store.upsert_mineru_job(
                        &hash,
                        rel_path,
                        Some(batch_id),
                        "failed",
                        Some(&msg),
                    );
                    stats.failed_pdfs.push(format!("{rel_path}: {msg}"));
                    completed.insert(hash, false);
                }
                _ => {}
            }
        }

        if completed.len() >= batch.len() {
            return Ok(stats);
        }
    }

    Err(RagError::new("mineru_timeout", "MinerU 批量解析轮询超时。"))
}

fn download_zip(zip_url: &str) -> Result<Vec<u8>, RagError> {
    let ep = Endpoint::parse(zip_url).map_err(|e| RagError::new("mineru_url", e))?;
    let parsed = url::Url::parse(zip_url).map_err(|e| RagError::new("mineru_url", e.to_string()))?;
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
        label: "MinerU-ZIP",
    })
    .map_err(|e| RagError::new("mineru_download", e))?;
    if !(200..300).contains(&resp.status) {
        return Err(RagError::new(
            "mineru_download",
            format!("下载 MinerU ZIP 失败 (HTTP {})", resp.status),
        ));
    }
    Ok(resp.body)
}

pub fn extract_zip_to_cache(zip_bytes: &[u8], out_dir: &Path) -> Result<(), String> {
    let mut archive =
        ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| format!("打开 ZIP 失败: {e}"))?;

    let mut best_md: Option<String> = None;
    let mut anchors: Vec<PageAnchor> = Vec::new();

    for idx in 0..archive.len() {
        let mut file = archive.by_index(idx).map_err(|e| e.to_string())?;
        if file.is_dir() {
            continue;
        }
        let name = file
            .name()
            .map_err(|e| e.to_string())?
            .to_ascii_lowercase();
        if name.ends_with(".md") {
            let mut buf = String::new();
            file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
            if best_md.as_ref().is_none_or(|prev| buf.len() > prev.len()) {
                best_md = Some(buf);
            }
        } else if name.ends_with("content_list.json") || name.ends_with("middle.json") {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            if anchors.is_empty()
                && let Ok(val) = serde_json::from_slice::<Value>(&buf)
            {
                anchors = parse_page_anchors(&val);
            }
        }
    }

    let md = best_md.ok_or_else(|| "MinerU ZIP 内未找到 Markdown 输出".to_owned())?;
    fs::create_dir_all(out_dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;
    fs::write(out_dir.join("full.md"), md).map_err(|e| e.to_string())?;
    if !anchors.is_empty() {
        let json_bytes = serde_json::to_vec(&anchors).map_err(|e| e.to_string())?;
        fs::write(out_dir.join("pages.json"), json_bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn parse_page_anchors(val: &Value) -> Vec<PageAnchor> {
    let mut out = Vec::new();
    if let Some(items) = val.as_array() {
        for item in items {
            if item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| matches!(t, "header" | "footer" | "page_number" | "aside_text" | "page_footnote"))
            {
                continue;
            }
            let Some(page_idx) = item.get("page_idx").and_then(Value::as_u64) else {
                continue;
            };
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| item.get("table_caption").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str))
                .unwrap_or("")
                .trim();
            let norm = normalize_anchor(text);
            if norm.chars().count() >= 12 {
                out.push(PageAnchor {
                    page_num: (page_idx as usize) + 1,
                    snippet: norm,
                });
            }
        }
    } else if let Some(pages) = val.get("pdf_info").and_then(Value::as_array) {
        for (idx, page) in pages.iter().enumerate() {
            let page_num = page
                .get("page_idx")
                .and_then(Value::as_u64)
                .map_or(idx + 1, |p| (p as usize) + 1);
            let raw = serde_json::to_string(page).unwrap_or_default();
            if let Some(blocks) = page.get("para_blocks").and_then(Value::as_array) {
                for block in blocks {
                    let s = extract_block_text(block);
                    let norm = normalize_anchor(&s);
                    if norm.chars().count() >= 12 {
                        out.push(PageAnchor {
                            page_num,
                            snippet: norm,
                        });
                    }
                }
            } else if !raw.is_empty() {
                out.push(PageAnchor {
                    page_num,
                    snippet: normalize_anchor(&raw),
                });
            }
        }
    }
    out
}

fn extract_block_text(block: &Value) -> String {
    let mut out = String::new();
    if let Some(lines) = block.get("lines").and_then(Value::as_array) {
        for line in lines {
            if let Some(spans) = line.get("spans").and_then(Value::as_array) {
                for span in spans {
                    if let Some(content) = span.get("content").and_then(Value::as_str) {
                        out.push_str(content);
                        out.push(' ');
                    }
                }
            }
        }
    }
    out
}

fn normalize_anchor(s: &str) -> String {
    s.split_whitespace().take(12).collect::<Vec<_>>().join(" ")
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
struct BatchUrlResponse {
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<BatchUrlData>,
}

#[derive(Deserialize)]
struct BatchUrlData {
    batch_id: String,
    file_urls: Vec<String>,
}

#[derive(Deserialize)]
struct PollBatchResponse {
    code: i64,
    #[serde(default)]
    data: Option<PollBatchData>,
}

#[derive(Deserialize)]
struct PollBatchData {
    #[serde(default)]
    extract_result: Vec<PollExtractItem>,
}

#[derive(Deserialize)]
struct PollExtractItem {
    #[serde(default)]
    data_id: Option<String>,
    state: String,
    #[serde(default)]
    full_zip_url: Option<String>,
    #[serde(default)]
    err_msg: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    #[test]
    fn extract_zip_caches_markdown_and_maps_page_numbers() {
        let mut zip_buf = Vec::new();
        {
            let mut writer = ZipArchiveWriterHelper::new(&mut zip_buf);
            writer.add(
                "full.md",
                b"# Introduction\n\nGradient descent converges linearly under strong convexity.\n\n## KKT\n\n$$\\nabla f(x^*) + \\lambda \\nabla g(x^*) = 0$$\n",
            );
            writer.add(
                "paper_content_list.json",
                br#"[
                  {"page_idx": 0, "text": "Gradient descent converges linearly under strong convexity."},
                  {"page_idx": 3, "text": "$$\\nabla f(x^*) + \\lambda \\nabla g(x^*) = 0$$"}
                ]"#,
            );
            writer.finish();
        }

        let dir = tempdir().unwrap();
        let hash = "deadbeef12345678";
        extract_zip_to_cache(&zip_buf, &cache_dir(dir.path(), hash)).unwrap();

        let extracted = load_cached_pdf(dir.path(), "papers/opt.pdf", hash).unwrap();
        assert_eq!(extracted.parse_tier, "mineru");
        assert_eq!(extracted.chunks.len(), 2);
        assert_eq!(extracted.chunks[0].page_num, Some(1));
        assert_eq!(extracted.chunks[1].page_num, Some(4));
    }

    struct ZipArchiveWriterHelper<'a>(zip::ZipWriter<Cursor<&'a mut Vec<u8>>>);
    impl<'a> ZipArchiveWriterHelper<'a> {
        fn new(buf: &'a mut Vec<u8>) -> Self {
            Self(zip::ZipWriter::new(Cursor::new(buf)))
        }
        fn add(&mut self, name: &str, bytes: &[u8]) {
            self.0
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            self.0.write_all(bytes).unwrap();
        }
        fn finish(self) {
            self.0.finish().unwrap();
        }
    }
}
