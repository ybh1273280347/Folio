//! Bridges `rag` workspace tool requests and debounced background sync to `crate::rag`.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    rag::{
        DirtyDocument, RagError, SearchRequest,
        client::{EmbeddingClient, RerankClient, reap_idle_servers},
        collect_eligible_paths, mineru,
        pipeline::{SearchServices, SyncOptions, search_project, sync_project},
        store::{RagStore, StoreStats},
    },
    tags::Snapshot,
};
use gpui_kit::{AppContext, Context};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const SYNC_DEBOUNCE: Duration = Duration::from_millis(2500);
const REAP_INTERVAL: Duration = Duration::from_secs(15);

pub(in crate::workspace) struct RagSyncState {
    pending_deadline: Option<Instant>,
    in_flight: Option<(String, Arc<AtomicBool>)>,
    last_reap: Instant,
}

impl Default for RagSyncState {
    fn default() -> Self {
        Self {
            pending_deadline: None,
            in_flight: None,
            last_reap: Instant::now(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum RagOperation {
    Search {
        params: SearchToolParams,
    },
    Status,
    Reindex {
        #[serde(default)]
        params: ReindexToolParams,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchToolParams {
    query: String,
    #[serde(default, alias = "lex_queries")]
    lex_queries: Vec<String>,
    #[serde(default, alias = "vec_queries")]
    vec_queries: Vec<String>,
    #[serde(default)]
    mode: crate::rag::SearchMode,
    #[serde(default)]
    rerank: Option<bool>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_match")]
    r#match: String,
    #[serde(default, alias = "under_directory")]
    under_directory: Option<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReindexToolParams {
    #[serde(default, alias = "force_rebuild")]
    force_rebuild: bool,
    #[serde(default, alias = "upgrade_pdfs")]
    upgrade_pdfs: bool,
}

fn default_match() -> String {
    "all".to_owned()
}

fn default_limit() -> usize {
    8
}

impl Workspace {
    fn active_rag_root(&self) -> Option<(String, PathBuf)> {
        let project = self.projects.directory()?;
        let root = dunce::canonicalize(&project.folder).ok()?;
        let key = crate::ai::project_key(&root);
        Some((key, root))
    }

    pub(in crate::workspace) fn schedule_rag_sync(&mut self) {
        let Some((_, root)) = self.active_rag_root() else {
            return;
        };
        if !root.join(".folio").join("rag.sqlite3").exists() {
            return;
        }
        self.rag_sync.pending_deadline = Some(Instant::now() + SYNC_DEBOUNCE);
    }

    pub(in crate::workspace) fn cancel_rag_sync(&mut self) {
        self.rag_sync.pending_deadline = None;
        if let Some((_, cancelled)) = self.rag_sync.in_flight.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub(in crate::workspace) fn reset_rag_sync_on_project_switch(&mut self, cx: &mut Context<Self>) {
        self.cancel_rag_sync();
        let Some((_, root)) = self.active_rag_root() else {
            self.ai.update(cx, |pane, cx| pane.set_rag_status(None, cx));
            return;
        };
        if !root.join(".folio").join("rag.sqlite3").exists() {
            self.ai.update(cx, |pane, cx| pane.set_rag_status(None, cx));
            return;
        }
        self.ai.update(cx, |pane, cx| {
            pane.set_rag_status(
                Some(("KB".to_owned(), "正在同步当前项目知识库…".to_owned())),
                cx,
            );
        });
        self.rag_sync.pending_deadline = Some(Instant::now() + Duration::from_millis(500));
    }

    pub(in crate::workspace) fn trigger_rag_sync_now(&mut self, cx: &mut Context<Self>) {
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        self.rag_sync.pending_deadline = None;
        if let Some((_, old_cancel)) = self.rag_sync.in_flight.take() {
            old_cancel.store(true, Ordering::Relaxed);
        }

        let db_path = root.join(".folio").join("rag.sqlite3");
        let config = self.settings.rag.clone();
        let eligible_paths =
            collect_eligible_paths(self.tree.files().iter().map(|file| file.path.clone()));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.rag_sync.in_flight = Some((project_key.clone(), cancelled.clone()));
        self.ai.update(cx, |pane, cx| {
            pane.set_rag_status(
                Some(("KB…".to_owned(), "正在后台增量同步知识库索引…".to_owned())),
                cx,
            );
        });

        let task = cx.background_spawn(async move {
            let embed_client = EmbeddingClient::from_config(&config).ok();
            let mut store = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .map_err(|e| RagError::new("store_error", e))?;
            let _ = sync_project(
                &root,
                &eligible_paths,
                &mut store,
                SyncOptions {
                    config: &config,
                    embed_client: embed_client.as_ref(),
                    force_rebuild: false,
                    upgrade_pdfs: false,
                },
                &cancelled,
            )?;
            store.stats().map_err(|e| RagError::new("store_error", e))
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                let is_current = app
                    .rag_sync
                    .in_flight
                    .as_ref()
                    .is_some_and(|(k, _)| *k == project_key);
                if is_current {
                    app.rag_sync.in_flight = None;
                }
                if app
                    .active_rag_root()
                    .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    if let Ok(stats) = outcome {
                        app.publish_rag_pill(&stats, cx);
                    }
                }
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn poll_rag(&mut self, cx: &mut Context<Self>) {
        if self.rag_sync.last_reap.elapsed() >= REAP_INTERVAL {
            self.rag_sync.last_reap = Instant::now();
            cx.background_spawn(async { reap_idle_servers() }).detach();
        }
        if let Some(deadline) = self.rag_sync.pending_deadline
            && Instant::now() >= deadline
            && self.rag_sync.in_flight.is_none()
        {
            self.trigger_rag_sync_now(cx);
        }
    }

    fn publish_rag_pill(&self, stats: &StoreStats, cx: &mut Context<Self>) {
        let status = format_rag_pill(stats);
        self.ai.update(cx, |pane, cx| pane.set_rag_status(Some(status), cx));
    }

    pub(super) fn rag_request(&mut self, request: &WorkspaceToolRequest, cx: &mut Context<Self>) {
        if request.action == WorkspaceToolAction::Cancel {
            if let Some(cancelled) = self.tool_requests.rag_requests.remove(&request.id) {
                cancelled.store(true, Ordering::Relaxed);
            }
            return;
        }
        if let Err(error) = self.start_rag_request(request, cx) {
            self.ai.read(cx).respond_workspace_tool(request, error_json(&error));
        }
    }

    fn start_rag_request(
        &mut self,
        request: &WorkspaceToolRequest,
        cx: &mut Context<Self>,
    ) -> Result<(), RagError> {
        let operation: RagOperation = serde_json::from_value(request.arguments.clone())
            .map_err(|e| RagError::new("invalid_request", e.to_string()))?;

        let project = self.projects.directory().ok_or_else(|| {
            RagError::new(
                "directory_workspace_required",
                crate::i18n::tags::ERR_AGENT_TAGS_PROJECT_OR_DEFAULT.get(),
            )
        })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|e| RagError::new("project_unavailable", e.to_string()))?;
        let project_key = crate::ai::project_key(&root);
        if request.project_key != project_key {
            return Err(RagError::new(
                "project_not_active",
                crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get(),
            ));
        }

        let db_path = root.join(".folio").join("rag.sqlite3");
        let config = self.settings.rag.clone();
        let eligible_paths =
            collect_eligible_paths(self.tree.files().iter().map(|file| file.path.clone()));
        let dirty_docs: Vec<DirtyDocument> = self
            .documents
            .open_buffers()
            .filter(|(_, buffer)| buffer.is_dirty() && !buffer.deleted)
            .map(|(path, buffer)| DirtyDocument {
                rel_path: path.to_owned(),
                text: buffer.text.clone(),
            })
            .collect();
        let tag_snapshot = self.tags.snapshot.clone();

        let cancelled = Arc::new(AtomicBool::new(false));
        self.tool_requests
            .rag_requests
            .insert(request.id.clone(), cancelled.clone());

        let task = cx.background_spawn(async move {
            let embed_client = EmbeddingClient::from_config(&config).ok();
            let rerank_client = RerankClient::from_config(&config).ok();
            let mut store = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .map_err(|e| RagError::new("store_error", e))?;

            match operation {
                RagOperation::Status => {
                    let embed_server_ready = embed_client
                        .as_ref()
                        .is_some_and(EmbeddingClient::health_check);
                    let rerank_server_ready = rerank_client
                        .as_ref()
                        .is_some_and(RerankClient::health_check);
                    let stats = store
                        .stats()
                        .map_err(|e| RagError::new("store_error", e))?;
                    let (mineru_upgraded_pdfs, mineru_pending_pdfs) = store
                        .mineru_counts()
                        .map_err(|e| RagError::new("store_error", e))?;
                    let mineru_configured = !mineru::resolve_api_key(&config).is_empty();
                    let report = crate::rag::StatusReport {
                        db_path: db_path.to_string_lossy().into_owned(),
                        embed_model: config.embed_model,
                        embed_dim: config.embed_dim,
                        embed_server_ready,
                        rerank_server_ready,
                        mineru_configured,
                        mineru_upgraded_pdfs,
                        mineru_pending_pdfs,
                        indexed_files: stats.indexed_files,
                        total_chunks: stats.total_chunks,
                        vector_chunks: stats.vector_chunks,
                        pending_vectors: stats.pending_vectors,
                        needs_ocr_files: stats.needs_ocr_files.clone(),
                    };
                    Ok((json!({ "status": "ok", "report": report }), Some(stats)))
                }
                RagOperation::Reindex { params } => {
                    let report = sync_project(
                        &root,
                        &eligible_paths,
                        &mut store,
                        SyncOptions {
                            config: &config,
                            embed_client: embed_client.as_ref(),
                            force_rebuild: params.force_rebuild,
                            upgrade_pdfs: params.upgrade_pdfs,
                        },
                        &cancelled,
                    )?;
                    let stats = store.stats().ok();
                    Ok((json!({ "status": "ok", "report": report }), stats))
                }
                RagOperation::Search { params } => {
                    let allowed_paths = resolve_tag_filter(
                        &tag_snapshot,
                        &eligible_paths,
                        &params.tags,
                        &params.r#match,
                    )?;

                    let stats = store
                        .stats()
                        .map_err(|e| RagError::new("store_error", e))?;
                    if stats.indexed_files == 0 && !eligible_paths.is_empty() {
                        sync_project(
                            &root,
                            &eligible_paths,
                            &mut store,
                            SyncOptions {
                                config: &config,
                                embed_client: embed_client.as_ref(),
                                force_rebuild: false,
                                upgrade_pdfs: false,
                            },
                            &cancelled,
                        )?;
                    }

                    let search_req = SearchRequest {
                        query: params.query,
                        lex_queries: params.lex_queries,
                        vec_queries: params.vec_queries,
                        mode: params.mode,
                        rerank: params.rerank,
                        extensions: params.extensions,
                        under_directory: params.under_directory,
                        limit: params.limit,
                    };
                    let response = search_project(
                        &mut store,
                        &search_req,
                        &dirty_docs,
                        allowed_paths.as_ref(),
                        SearchServices {
                            embed_client: embed_client.as_ref(),
                            rerank_client: rerank_client.as_ref(),
                            rerank_model: &config.rerank_model,
                        },
                        &cancelled,
                    )?;
                    let latest_stats = store.stats().ok();
                    Ok((
                        json!({
                            "status": "ok",
                            "mode_used": response.mode_used,
                            "degraded_reason": response.degraded_reason,
                            "indexed_files": response.indexed_files,
                            "pending_vectors": response.pending_vectors,
                            "results": response.results,
                        }),
                        latest_stats,
                    ))
                }
            }
        });

        let request = request.clone();
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                app.tool_requests.rag_requests.remove(&request.id);
                let payload = match outcome {
                    Ok((payload, stats)) => {
                        if let Some(stats) = stats
                            && app
                                .active_rag_root()
                                .is_some_and(|(active_key, _)| active_key == project_key)
                        {
                            app.publish_rag_pill(&stats, cx);
                        }
                        payload
                    }
                    Err(err) => error_json(&err),
                };
                app.ai.read(cx).respond_workspace_tool(&request, payload);
            });
        })
        .detach();

        Ok(())
    }
}

fn format_rag_pill(stats: &StoreStats) -> (String, String) {
    let label = format!("KB {}", stats.indexed_files);
    let tooltip = format!(
        "本地知识库：已索引 {} 篇 · {} 切片 · 待向量 {}（点击增量同步）",
        stats.indexed_files, stats.total_chunks, stats.pending_vectors
    );
    (label, tooltip)
}

fn resolve_tag_filter(
    snapshot: &Snapshot,
    eligible_paths: &[String],
    tag_names: &[String],
    match_mode: &str,
) -> Result<Option<HashSet<String>>, RagError> {
    if tag_names.is_empty() {
        return Ok(None);
    }
    if !matches!(match_mode, "all" | "any") {
        return Err(RagError::new(
            "invalid_request",
            format!("match 仅支持 all 或 any，收到 `{match_mode}`"),
        ));
    }
    let mut tag_ids = Vec::with_capacity(tag_names.len());
    for name in tag_names {
        let Some(tag) = snapshot.tags.iter().find(|t| t.name == *name) else {
            return Ok(Some(HashSet::new()));
        };
        tag_ids.push(tag.id.as_str());
    }

    let allowed: HashSet<String> = eligible_paths
        .iter()
        .filter(|path| {
            if match_mode == "any" {
                tag_ids.iter().any(|id| snapshot.has(path, id))
            } else {
                tag_ids.iter().all(|id| snapshot.has(path, id))
            }
        })
        .cloned()
        .collect();
    Ok(Some(allowed))
}

fn error_json(error: &RagError) -> Value {
    let status = if error.code == "cancelled" {
        "cancelled"
    } else {
        "error"
    };
    json!({
        "status": status,
        "error": {
            "code": error.code,
            "message": error.message,
            "retryable": false
        }
    })
}
