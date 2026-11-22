//! Local llama-server HTTP clients (Embedding & Reranker) with port probing and idle release.

use super::RagConfig;
use crate::http::{self, RequestOptions};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const QUERY_INSTRUCT: &str =
    "Instruct: Given a search query, retrieve relevant passages from personal notes and papers\nQuery: ";
const EMBED_BATCH_SIZE: usize = 16;
const RERANK_BATCH_SIZE: usize = 25;

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub base_path: String,
}

impl Endpoint {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let parsed =
            url::Url::parse(raw.trim()).map_err(|e| format!("无效的服务地址 `{raw}`: {e}"))?;
        let https = match parsed.scheme() {
            "http" => false,
            "https" => true,
            other => return Err(format!("不支持的协议 `{other}`")),
        };
        let host = parsed
            .host_str()
            .ok_or_else(|| format!("地址缺少主机名: `{raw}`"))?
            .to_owned();
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| format!("地址缺少端口: `{raw}`"))?;
        let base_path = parsed.path().trim_end_matches('/').to_owned();
        Ok(Self {
            https,
            host,
            port,
            base_path,
        })
    }

    pub fn join(&self, suffix: &str) -> String {
        if self.base_path.is_empty() {
            suffix.to_owned()
        } else {
            format!("{}{}", self.base_path, suffix)
        }
    }

    pub fn is_loopback(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }
}

struct ManagedChild {
    child: Child,
    last_used: Instant,
    idle_release_secs: u64,
}

static MANAGED_SERVERS: OnceLock<Mutex<HashMap<u16, ManagedChild>>> = OnceLock::new();

fn server_registry() -> &'static Mutex<HashMap<u16, ManagedChild>> {
    MANAGED_SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn reap_idle_servers() {
    let mut map = server_registry().lock().expect("server registry poisoned");
    map.retain(|_, entry| {
        if entry.child.try_wait().ok().flatten().is_some() {
            return false;
        }
        if entry.idle_release_secs > 0
            && entry.last_used.elapsed() > Duration::from_secs(entry.idle_release_secs)
        {
            let _ = entry.child.kill();
            let _ = entry.child.wait();
            return false;
        }
        true
    });
}

pub fn shutdown_managed_servers() {
    let mut map = server_registry().lock().expect("server registry poisoned");
    for (_, mut entry) in map.drain() {
        let _ = entry.child.kill();
        let _ = entry.child.wait();
    }
}

fn check_health(endpoint: &Endpoint) -> bool {
    let path = endpoint.join("/health");
    let resp = http::send(RequestOptions {
        https: endpoint.https,
        host: &endpoint.host,
        port: endpoint.port,
        method: "GET",
        path: &path,
        headers: &[],
        body: &[],
        connect_timeout_ms: 400,
        receive_timeout_ms: 800,
        use_system_proxy: !endpoint.is_loopback(),
        use_env_proxy: false,
        max_response_bytes: 65_536,
        label: "llama-server",
    });
    resp.is_ok_and(|r| (200..300).contains(&r.status))
}

fn touch_managed(port: u16) {
    let mut map = server_registry().lock().expect("server registry poisoned");
    if let Some(entry) = map.get_mut(&port) {
        entry.last_used = Instant::now();
    }
}

fn probe_or_spawn(
    endpoint: &Endpoint,
    server_bin: &str,
    model_path: &str,
    auto_manage: bool,
    idle_release_secs: u64,
    mode_args: &[&str],
) -> bool {
    reap_idle_servers();
    if check_health(endpoint) {
        touch_managed(endpoint.port);
        return true;
    }
    if !auto_manage || !endpoint.is_loopback() || model_path.is_empty() {
        return false;
    }

    let bin = if server_bin.is_empty() {
        "llama-server"
    } else {
        server_bin
    };
    let port_str = endpoint.port.to_string();
    let mut cmd = Command::new(bin);
    cmd.args(["-m", model_path])
        .args(mode_args)
        .args([
            "-ngl",
            "99",
            "-c",
            "4096",
            "--host",
            &endpoint.host,
            "--port",
            &port_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let Ok(child) = cmd.spawn() else {
        return false;
    };
    {
        let mut map = server_registry().lock().expect("server registry poisoned");
        map.insert(
            endpoint.port,
            ManagedChild {
                child,
                last_used: Instant::now(),
                idle_release_secs,
            },
        );
    }

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check_health(endpoint) {
            touch_managed(endpoint.port);
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

pub struct EmbeddingClient {
    endpoint: Endpoint,
    model: String,
    server_bin: String,
    model_path: String,
    auto_manage: bool,
    idle_release_secs: u64,
}

impl EmbeddingClient {
    pub fn from_config(config: &RagConfig) -> Result<Self, String> {
        Ok(Self {
            endpoint: Endpoint::parse(&config.embed_url)?,
            model: config.embed_model.clone(),
            server_bin: config.server_bin.trim().to_owned(),
            model_path: config.embed_model_path.trim().to_owned(),
            auto_manage: config.auto_manage_server,
            idle_release_secs: config.idle_release_secs,
        })
    }

    pub fn health_check(&self) -> bool {
        check_health(&self.endpoint)
    }

    pub fn ensure_ready(&self) -> bool {
        probe_or_spawn(
            &self.endpoint,
            &self.server_bin,
            &self.model_path,
            self.auto_manage,
            self.idle_release_secs,
            &["--embedding", "-b", "512", "-ub", "512"],
        )
    }

    pub fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        self.embed_batches(texts, false)
    }

    pub fn embed_queries(&self, queries: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        self.embed_batches(queries, true)
    }

    fn embed_batches(&self, texts: &[&str], is_query: bool) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut all = Vec::with_capacity(texts.len());
        let path = self.endpoint.join("/v1/embeddings");

        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let input: Vec<String> = batch
                .iter()
                .map(|t| {
                    if is_query {
                        format!("{QUERY_INSTRUCT}{t}")
                    } else {
                        (*t).to_owned()
                    }
                })
                .collect();
            let payload = serde_json::to_vec(&EmbedRequest {
                model: &self.model,
                input,
            })
            .map_err(|e| e.to_string())?;

            let resp = http::send(RequestOptions {
                https: self.endpoint.https,
                host: &self.endpoint.host,
                port: self.endpoint.port,
                method: "POST",
                path: &path,
                headers: &[("Content-Type", "application/json")],
                body: &payload,
                connect_timeout_ms: 2_000,
                receive_timeout_ms: 60_000,
                use_system_proxy: !self.endpoint.is_loopback(),
                use_env_proxy: false,
                max_response_bytes: 32 * 1024 * 1024,
                label: "llama-server",
            })?;
            if !(200..300).contains(&resp.status) {
                return Err(format!("Embedding 服务返回 HTTP {}", resp.status));
            }
            let parsed: EmbedResponse = serde_json::from_slice(&resp.body)
                .map_err(|e| format!("解析 Embedding 响应失败: {e}"))?;
            let mut items = parsed.data;
            items.sort_by_key(|item| item.index);
            if items.len() != batch.len() {
                return Err(format!(
                    "Embedding 返回条数不匹配: 期望 {}, 实际 {}",
                    batch.len(),
                    items.len()
                ));
            }
            for item in items {
                all.push(item.embedding);
            }
        }
        touch_managed(self.endpoint.port);
        Ok(all)
    }
}

pub struct RerankClient {
    endpoint: Endpoint,
    model: String,
    server_bin: String,
    model_path: String,
    auto_manage: bool,
    idle_release_secs: u64,
}

impl RerankClient {
    pub fn from_config(config: &RagConfig) -> Result<Self, String> {
        Ok(Self {
            endpoint: Endpoint::parse(&config.rerank_url)?,
            model: config.rerank_model.clone(),
            server_bin: config.server_bin.trim().to_owned(),
            model_path: config.rerank_model_path.trim().to_owned(),
            auto_manage: config.auto_manage_server,
            idle_release_secs: config.idle_release_secs,
        })
    }

    pub fn health_check(&self) -> bool {
        check_health(&self.endpoint)
    }

    pub fn ensure_ready(&self) -> bool {
        probe_or_spawn(
            &self.endpoint,
            &self.server_bin,
            &self.model_path,
            self.auto_manage,
            self.idle_release_secs,
            &["--reranking", "--pooling", "rank"],
        )
    }

    pub fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<f64>, String> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let path = self.endpoint.join("/v1/rerank");
        let mut scores = Vec::with_capacity(documents.len());
        for batch in documents.chunks(RERANK_BATCH_SIZE) {
            let payload = serde_json::to_vec(&RerankRequest {
                model: &self.model,
                query,
                documents: batch,
            })
            .map_err(|e| e.to_string())?;

            let resp = http::send(RequestOptions {
                https: self.endpoint.https,
                host: &self.endpoint.host,
                port: self.endpoint.port,
                method: "POST",
                path: &path,
                headers: &[("Content-Type", "application/json")],
                body: &payload,
                connect_timeout_ms: 2_000,
                receive_timeout_ms: 60_000,
                use_system_proxy: !self.endpoint.is_loopback(),
                use_env_proxy: false,
                max_response_bytes: 8 * 1024 * 1024,
                label: "llama-server-rerank",
            })?;
            if !(200..300).contains(&resp.status) {
                return Err(format!("Rerank 服务返回 HTTP {}", resp.status));
            }
            let parsed: RerankResponse = serde_json::from_slice(&resp.body)
                .map_err(|e| format!("解析 Rerank 响应失败: {e}"))?;
            let mut ordered = vec![0.0f64; batch.len()];
            for item in parsed.results {
                if item.index < ordered.len() {
                    ordered[item.index] = item.relevance_score.or(item.score).unwrap_or(0.0);
                }
            }
            scores.extend(ordered);
        }
        touch_managed(self.endpoint.port);
        Ok(scores)
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedItem>,
}

#[derive(Deserialize)]
struct EmbedItem {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [&'a str],
}

#[derive(Deserialize)]
struct RerankResponse {
    results: Vec<RerankItem>,
}

#[derive(Deserialize)]
struct RerankItem {
    index: usize,
    #[serde(default)]
    relevance_score: Option<f64>,
    #[serde(default)]
    score: Option<f64>,
}
