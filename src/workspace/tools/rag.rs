//! Handles AI knowledge-base requests and replies for the active workspace project.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    rag::{
        DirtyDocument, RagError, SearchRequest,
        client::{EmbeddingClient, RerankClient},
        collect_eligible_paths, ocr,
        pipeline::{SearchServices, SyncOptions, search_project, sync_project},
        store::RagStore,
    },
    tags::Snapshot,
};
use gpui_kit::{AppContext, Context};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

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
    ReadPdf {
        params: ReadPdfToolParams,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadPdfToolParams {
    path: String,
    #[serde(default, alias = "start_page")]
    start_page: Option<usize>,
    #[serde(default, alias = "end_page")]
    end_page: Option<usize>,
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
            if let RagOperation::ReadPdf { params } = operation {
                let res = ocr::read_pdf_markdown(
                    &root,
                    &params.path,
                    params.start_page,
                    params.end_page,
                )?;
                let value = serde_json::to_value(res)
                    .map_err(|e| RagError::new("serialize_error", e.to_string()))?;
                let mut map = match value {
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                map.insert("status".to_owned(), json!("ok"));
                return Ok((Value::Object(map), None));
            }

            let embed_client = EmbeddingClient::from_config(&config).ok();
            let rerank_client = RerankClient::from_config(&config).ok();
            let mut store = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .map_err(|e| RagError::new("store_error", e))?;

            match operation {
                RagOperation::ReadPdf { .. } => unreachable!(),
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
                    let items = store.pdf_library_items().unwrap_or_default();
                    let (accurate_upgraded_pdfs, accurate_pending_pdfs) = store
                        .ocr_counts()
                        .map_err(|e| RagError::new("store_error", e))?;
                    let accurate_configured = !ocr::resolve_ocr_token(&config).is_empty();
                    let cached_pdf_markdowns = store
                        .cached_accurate_pdfs()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|(path, hash, parse_tier)| {
                            ocr::cache_dir(&root, &hash)
                                .join("full.md")
                                .exists()
                                .then(|| crate::rag::CachedPdfInfo {
                                    path,
                                    parse_tier,
                                    markdown_cache: format!(".folio/pdf-cache/{hash}/full.md"),
                                })
                        })
                        .collect();
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
                        cached_pdf_markdowns,
                    };
                    Ok((json!({ "status": "ok", "report": report }), Some((stats, items))))
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
                    let snapshot = store
                        .stats()
                        .ok()
                        .zip(store.pdf_library_items().ok());
                    Ok((json!({ "status": "ok", "report": report }), snapshot))
                }
                RagOperation::Search { params } => {
                    let allowed_paths = resolve_tag_filter(
                        &tag_snapshot,
                        &eligible_paths,
                        &params.tags,
                        &params.r#match,
                    )?;

                    if !eligible_paths.is_empty()
                        && !store.has_indexed_documents().map_err(|e| RagError::new("store_error", e))?
                    {
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
                    let snapshot = store
                        .stats()
                        .ok()
                        .zip(store.pdf_library_items().ok());
                    Ok((
                        json!({
                            "status": "ok",
                            "mode_used": response.mode_used,
                            "degraded_reason": response.degraded_reason,
                            "indexed_files": response.indexed_files,
                            "pending_vectors": response.pending_vectors,
                            "results": response.results,
                        }),
                        snapshot,
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
                    Ok((payload, snapshot)) => {
                        if let Some((stats, items)) = snapshot
                            && app
                                .active_rag_root()
                                .is_some_and(|(active_key, _)| active_key == project_key)
                        {
                            app.right_panel.kb.apply_snapshot(stats, items);
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
