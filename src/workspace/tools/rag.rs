//! Bridges `rag` workspace tool requests and debounced background sync to `crate::rag`.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    rag::{
        DirtyDocument, RagError, SearchRequest,
        client::{EmbeddingClient, RerankClient, reap_idle_servers},
        collect_eligible_paths, mineru,
        pipeline::{SearchServices, SyncOptions, search_project, sync_project},
        store::RagStore,
    },
    tags::Snapshot,
};
use gpui_kit::{AppContext, Context, Window};
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
    searches: Vec<crate::rag::SubSearch>,
    #[serde(default)]
    intent: Option<String>,
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
    #[serde(default)]
    accurate: Option<crate::rag::AccuratePdfMode>,
    #[serde(default)]
    paths: Vec<String>,
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

    pub(in crate::workspace) fn kb_enabled(&self) -> bool {
        self.active_rag_root()
            .is_some_and(|(_, root)| root.join(".folio").join("rag.sqlite3").exists())
    }

    pub(in crate::workspace) fn kb_syncing(&self) -> bool {
        self.rag_sync.in_flight.is_some()
    }

    pub(in crate::workspace) fn kb_badge(&self) -> Option<(String, bool)> {
        self.active_rag_root()?;
        if self.kb_syncing() {
            return Some((crate::i18n::right_panel::BADGE_SYNCING.get().into(), false));
        }
        if !self.kb_enabled() {
            return None;
        }
        let stats = self.right_panel.kb.stats.as_ref()?;
        if !stats.needs_ocr_files.is_empty() {
            return Some((
                crate::i18n::right_panel::kb_ocr_badge(stats.needs_ocr_files.len()),
                true,
            ));
        }
        Some((crate::i18n::right_panel::kb_doc_count(stats.indexed_files), false))
    }

    pub(in crate::workspace) fn reset_rag_sync_on_project_switch(&mut self, cx: &mut Context<Self>) {
        self.cancel_rag_sync();
        self.right_panel.kb.reset_for_project();
        let Some((_, root)) = self.active_rag_root() else {
            cx.notify();
            return;
        };
        if !root.join(".folio").join("rag.sqlite3").exists() {
            cx.notify();
            return;
        }
        self.refresh_kb_snapshot(cx);
        self.rag_sync.pending_deadline = Some(Instant::now() + Duration::from_millis(500));
    }

    pub(in crate::workspace) fn trigger_rag_sync_now(&mut self, cx: &mut Context<Self>) {
        self.run_kb_sync(false, None, Vec::new(), cx);
    }

    pub(in crate::workspace) fn refresh_kb_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some((project_key, root)) = self.active_rag_root() else {
            self.right_panel.kb.stats = None;
            self.right_panel.kb.embed_ready = None;
            cx.notify();
            return;
        };
        let config = self.settings.rag.clone();
        let db_path = root.join(".folio").join("rag.sqlite3");
        let accurate_configured = !mineru::resolve_api_key(&config).is_empty();
        self.right_panel.kb.accurate_configured = accurate_configured;
        if !db_path.exists() {
            self.right_panel.kb.stats = None;
            self.right_panel.kb.embed_ready = None;
            cx.notify();
            return;
        }

        let task = cx.background_spawn(async move {
            let embed_ready = EmbeddingClient::from_config(&config)
                .ok()
                .is_some_and(|client| client.health_check());
            let stats = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .and_then(|store| store.stats())
                .ok();
            (stats, embed_ready, accurate_configured)
        });

        cx.spawn(async move |this, cx| {
            let (stats, embed_ready, accurate_configured) = task.await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .active_rag_root()
                    .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    app.right_panel.kb.accurate_configured = accurate_configured;
                    app.right_panel.kb.embed_ready = Some(embed_ready);
                    if let Some(stats) = stats {
                        app.right_panel.kb.apply_stats(stats);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn run_kb_sync(
        &mut self,
        force_rebuild: bool,
        accurate_mode: Option<crate::rag::AccuratePdfMode>,
        accurate_paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        self.rag_sync.pending_deadline = None;
        if let Some((_, old_cancel)) = self.rag_sync.in_flight.take() {
            old_cancel.store(true, Ordering::Relaxed);
        }

        let db_path = root.join(".folio").join("rag.sqlite3");
        let config = self.settings.rag.clone();
        let accurate_configured = !mineru::resolve_api_key(&config).is_empty();
        self.right_panel.kb.accurate_configured = accurate_configured;
        self.right_panel.kb.sync_error = None;
        let eligible_paths =
            collect_eligible_paths(self.tree.files().iter().map(|file| file.path.clone()));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.rag_sync.in_flight = Some((project_key.clone(), cancelled.clone()));
        cx.notify();

        let task = cx.background_spawn(async move {
            let embed_client = EmbeddingClient::from_config(&config).ok();
            let mut store = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .map_err(|e| RagError::new("store_error", e))?;
            let report = sync_project(
                &root,
                &eligible_paths,
                &mut store,
                SyncOptions {
                    config: &config,
                    embed_client: embed_client.as_ref(),
                    force_rebuild,
                    accurate_mode,
                    accurate_paths: &accurate_paths,
                },
                &cancelled,
            )?;
            let stats = store.stats().map_err(|e| RagError::new("store_error", e))?;
            Ok::<_, RagError>((stats, report.embed_server_ready))
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
                    match outcome {
                        Ok((stats, embed_ready)) => {
                            app.right_panel.kb.embed_ready = Some(embed_ready);
                            app.right_panel.kb.apply_stats(stats);
                            app.right_panel.kb.sync_error = None;
                        }
                        Err(err) if err.code == "cancelled" => {}
                        Err(err) => {
                            app.right_panel.kb.sync_error = Some(err.message);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn disable_kb(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_rag_sync();
        if let Some((_, root)) = self.active_rag_root() {
            let db = root.join(".folio").join("rag.sqlite3");
            for suffix in ["", "-wal", "-shm"] {
                let path = PathBuf::from(format!("{}{suffix}", db.display()));
                let _ = std::fs::remove_file(path);
            }
        }
        self.right_panel.kb.reset_for_project();
        self.right_panel.kb.flush_pending_clear(window, cx);
        cx.notify();
    }

    pub(in crate::workspace) fn submit_kb_search(&mut self, cx: &mut Context<Self>) {
        let query = self.right_panel.kb.search.read(cx).value().trim().to_owned();
        if query.is_empty() {
            self.right_panel.kb.clear_search_results();
            cx.notify();
            return;
        }
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        let db_path = root.join(".folio").join("rag.sqlite3");
        if !db_path.exists() {
            return;
        }

        let search_id = self.right_panel.kb.next_search_id();
        self.right_panel.kb.last_query = query.clone();
        self.right_panel.kb.searching = true;
        self.right_panel.kb.search_error = None;
        cx.notify();

        let config = self.settings.rag.clone();
        let dirty_docs: Vec<DirtyDocument> = self
            .documents
            .open_buffers()
            .filter(|(_, buffer)| buffer.is_dirty() && !buffer.deleted)
            .map(|(path, buffer)| DirtyDocument {
                rel_path: path.to_owned(),
                text: buffer.text.clone(),
            })
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let req = SearchRequest {
            searches: vec![
                crate::rag::SubSearch {
                    kind: crate::rag::SubSearchType::Vec,
                    query: query.clone(),
                },
                crate::rag::SubSearch {
                    kind: crate::rag::SubSearchType::Lex,
                    query: query.clone(),
                },
            ],
            intent: Some(query),
            rerank: None,
            extensions: Vec::new(),
            under_directory: None,
            limit: 12,
        };

        let task = cx.background_spawn(async move {
            let embed_client = EmbeddingClient::from_config(&config).ok();
            let rerank_client = RerankClient::from_config(&config).ok();
            let mut store = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .map_err(|e| RagError::new("store_error", e))?;
            let response = search_project(
                &mut store,
                &req,
                &dirty_docs,
                None,
                SearchServices {
                    embed_client: embed_client.as_ref(),
                    rerank_client: rerank_client.as_ref(),
                    rerank_model: &config.rerank_model,
                },
                &cancel,
            )?;
            let stats = store.stats().ok();
            Ok::<_, RagError>((response, stats))
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app.right_panel.kb.is_current_search(search_id)
                    || !app
                        .active_rag_root()
                        .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    return;
                }
                app.right_panel.kb.searching = false;
                match outcome {
                    Ok((resp, stats)) => {
                        app.right_panel.kb.results = resp.results;
                        app.right_panel.kb.degraded_reason = resp.degraded_reason;
                        app.right_panel.kb.search_error = None;
                        if let Some(stats) = stats {
                            app.right_panel.kb.apply_stats(stats);
                        }
                    }
                    Err(err) => {
                        app.right_panel.kb.search_error = Some(err.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn clear_kb_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_panel.kb.clear_search_results();
        self.right_panel
            .kb
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workspace) fn open_kb_result(
        &mut self,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, root)) = self.active_rag_root() else {
            return;
        };
        let clean = rel_path.strip_suffix("#annotations").unwrap_or(rel_path);
        let abs = root.join(clean.replace('/', "\\"));
        self.open_path_from_ai(&abs, window, cx);
    }

    pub(in crate::workspace) fn open_kb_api_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.update(cx, |panel, cx| {
            panel.sync(&self.settings, window, cx);
            panel.select_api_tab(cx);
        });
        self.show_preferences = true;
        cx.notify();
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
                    let (accurate_upgraded_pdfs, accurate_pending_pdfs) = store
                        .mineru_counts()
                        .map_err(|e| RagError::new("store_error", e))?;
                    let accurate_configured = !mineru::resolve_api_key(&config).is_empty();
                    let report = crate::rag::StatusReport {
                        db_path: db_path.to_string_lossy().into_owned(),
                        embed_model: config.embed_model,
                        embed_dim: config.embed_dim,
                        embed_server_ready,
                        rerank_server_ready,
                        accurate_configured,
                        accurate_upgraded_pdfs,
                        accurate_pending_pdfs,
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
                            accurate_mode: params.accurate,
                            accurate_paths: &params.paths,
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
                                accurate_mode: None,
                                accurate_paths: &[],
                            },
                            &cancelled,
                        )?;
                    }

                    let search_req = SearchRequest {
                        searches: params.searches,
                        intent: params.intent,
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
                            app.right_panel.kb.apply_stats(stats);
                            cx.notify();
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
