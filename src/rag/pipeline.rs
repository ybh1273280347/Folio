//! Incremental indexing, annotation sync, hybrid RRF fusion, and position-aware reranking.

use super::{
    AccuratePdfMode, DirtyDocument, RagConfig, RagError, ReindexReport, SearchHit, SearchRequest,
    SearchResponse, SubSearchType,
    chunker::{chunk_annotations, chunk_text_document},
    client::{EmbeddingClient, RerankClient},
    doc_kind, has_folio_pdf_fingerprint, load_derived_pdf_hashes, ocr, sha256_hex,
    store::{RagStore, ScoredChunk, path_matches_filters},
};
use crate::{annotations::AnnotationItem, storage};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::UNIX_EPOCH,
};

const RRF_K: f64 = 60.0;
const RERANK_TOP_K: usize = 25;
const MAX_CHUNKS_PER_FILE: usize = 4;
const SNIPPET_CHARS: usize = 120;
const EMBED_SYNC_PAGE: usize = 64;

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), RagError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(RagError::cancelled())
    } else {
        Ok(())
    }
}

pub struct SyncOptions<'a> {
    pub config: &'a RagConfig,
    pub embed_client: Option<&'a EmbeddingClient>,
    pub force_rebuild: bool,
    pub accurate_mode: Option<AccuratePdfMode>,
    pub accurate_paths: &'a [String],
}

pub fn sync_project(
    root: &Path,
    eligible_paths: &[String],
    store: &mut RagStore,
    opts: SyncOptions<'_>,
    cancelled: &AtomicBool,
) -> Result<ReindexReport, RagError> {
    check_cancelled(cancelled)?;
    if opts.force_rebuild {
        store
            .reset_index(&opts.config.embed_model, opts.config.embed_dim)
            .map_err(|e| RagError::new("store_error", e))?;
    }

    let existing = store
        .active_documents()
        .map_err(|e| RagError::new("store_error", e))?;
    let derived_hashes = load_derived_pdf_hashes(root);
    let mut live_set = HashSet::with_capacity(eligible_paths.len());
    let mut updated_files = 0usize;
    let mut skipped_unchanged = 0usize;

    for rel_path in eligible_paths {
        check_cancelled(cancelled)?;
        live_set.insert(rel_path.clone());

        let Some(kind) = doc_kind(rel_path) else {
            continue;
        };
        let abs_path = root.join(rel_path.replace('/', "\\"));
        let Ok(meta) = fs::metadata(&abs_path) else {
            continue;
        };
        let file_size = meta.len() as i64;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);

        if let Some(prev) = existing.get(rel_path)
            && prev.mtime_ns == mtime_ns
            && prev.file_size == file_size
        {
            skipped_unchanged += 1;
            continue;
        }

        let Ok(bytes) = fs::read(&abs_path) else {
            continue;
        };
        let content_hash = sha256_hex(&bytes);
        if kind == "pdf"
            && (derived_hashes.contains(&content_hash) || has_folio_pdf_fingerprint(&bytes))
        {
            let derived_key = format!("derived:{content_hash}");
            store
                .upsert_document(
                    rel_path,
                    kind,
                    &derived_key,
                    mtime_ns,
                    file_size,
                    "derived",
                    Some(""),
                    Some("derived"),
                    Some(&[]),
                )
                .map_err(|e| RagError::new("store_error", e))?;
            updated_files += 1;
            continue;
        }

        let already_parsed = store
            .has_content(&content_hash)
            .map_err(|e| RagError::new("store_error", e))?;

        if already_parsed {
            store
                .upsert_document(
                    rel_path,
                    kind,
                    &content_hash,
                    mtime_ns,
                    file_size,
                    "indexed",
                    None,
                    None,
                    None,
                )
                .map_err(|e| RagError::new("store_error", e))?;
            updated_files += 1;
            continue;
        }

        if kind == "pdf" {
            if let Some(cached) = ocr::load_cached_pdf(root, rel_path, &content_hash) {
                let status = if cached.needs_ocr {
                    "needs_ocr"
                } else {
                    "indexed"
                };
                store
                    .upsert_document(
                        rel_path,
                        kind,
                        &content_hash,
                        mtime_ns,
                        file_size,
                        status,
                        Some(&cached.raw_text),
                        Some(cached.parse_tier),
                        Some(&cached.chunks),
                    )
                    .map_err(|e| RagError::new("store_error", e))?;
                updated_files += 1;
            } else {
                match super::chunker::extract_and_chunk_pdf_with_pages(rel_path, &bytes) {
                    Ok((extracted, pages)) => {
                        ocr::write_pages_cache(&ocr::cache_dir(root, &content_hash), &pages, None)
                            .map_err(|e| RagError::new("io_error", e))?;
                        let status = if extracted.needs_ocr {
                            "needs_ocr"
                        } else {
                            "indexed"
                        };
                        store
                            .upsert_document(
                                rel_path,
                                kind,
                                &content_hash,
                                mtime_ns,
                                file_size,
                                status,
                                Some(&extracted.raw_text),
                                Some(extracted.parse_tier),
                                Some(&extracted.chunks),
                            )
                            .map_err(|e| RagError::new("store_error", e))?;
                        updated_files += 1;
                    }
                    Err(_) => {
                        store
                            .upsert_document(
                                rel_path,
                                kind,
                                &content_hash,
                                mtime_ns,
                                file_size,
                                "needs_ocr",
                                Some(""),
                                Some("fast_pdf"),
                                Some(&[]),
                            )
                            .map_err(|e| RagError::new("store_error", e))?;
                        updated_files += 1;
                    }
                }
            }
        } else {
            let text = String::from_utf8_lossy(&bytes);
            let extracted = chunk_text_document(rel_path, kind, &text);
            store
                .upsert_document(
                    rel_path,
                    kind,
                    &content_hash,
                    mtime_ns,
                    file_size,
                    "indexed",
                    Some(&extracted.raw_text),
                    Some(extracted.parse_tier),
                    Some(&extracted.chunks),
                )
                .map_err(|e| RagError::new("store_error", e))?;
            updated_files += 1;
        }
    }

    updated_files += sync_annotations(root, eligible_paths, store, &mut live_set, &existing)?;

    let removed_files = store
        .deactivate_missing(&live_set)
        .map_err(|e| RagError::new("store_error", e))?;

    let mut accurate_stats = ocr::AccurateUpgradeStats::default();
    if let Some(accurate_mode) = opts.accurate_mode {
        accurate_stats = ocr::upgrade_pdfs(
            root,
            store,
            opts.config,
            accurate_mode,
            opts.accurate_paths,
            cancelled,
        )?;
    }

    let mut embedded_chunks = 0usize;
    let embed_server_ready = opts.embed_client.is_some_and(EmbeddingClient::ensure_ready);
    if embed_server_ready && let Some(client) = opts.embed_client {
        loop {
            check_cancelled(cancelled)?;
            let pending = store
                .pending_vector_chunks(EMBED_SYNC_PAGE)
                .map_err(|e| RagError::new("store_error", e))?;
            if pending.is_empty() {
                break;
            }
            let texts: Vec<&str> = pending.iter().map(|c| c.chunk_text.as_str()).collect();
            let vectors = client
                .embed_documents(&texts)
                .map_err(|e| RagError::new("embed_error", e))?;
            let pairs: Vec<(i64, Vec<f32>)> = pending
                .iter()
                .zip(vectors)
                .map(|(chunk, vec)| (chunk.chunk_id, vec))
                .collect();
            embedded_chunks += pairs.len();
            store
                .insert_chunk_vectors(&pairs)
                .map_err(|e| RagError::new("store_error", e))?;
        }
    }

    let stats = store.stats().map_err(|e| RagError::new("store_error", e))?;
    Ok(ReindexReport {
        scanned_files: eligible_paths.len(),
        updated_files,
        skipped_unchanged,
        removed_files,
        embedded_chunks,
        pending_vectors: stats.pending_vectors,
        embed_server_ready,
        accurate_upgraded_pdfs: accurate_stats.upgraded_pdfs,
        accurate_failed_pdfs: accurate_stats.failed_pdfs,
        needs_ocr_files: stats.needs_ocr_files,
    })
}

fn sync_annotations(
    root: &Path,
    eligible_paths: &[String],
    store: &mut RagStore,
    live_set: &mut HashSet<String>,
    existing: &HashMap<String, super::store::DocumentMeta>,
) -> Result<usize, RagError> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for path in eligible_paths {
        let dir = path
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent)
            .to_owned();
        dirs.insert(dir);
    }

    let mut updated = 0usize;
    for dir in dirs {
        let sidecar = if dir.is_empty() {
            root.join(".folio").join("annotations.json")
        } else {
            root.join(dir.replace('/', "\\"))
                .join(".folio")
                .join("annotations.json")
        };
        let Ok(meta) = fs::metadata(&sidecar) else {
            continue;
        };
        let Ok(items) = storage::read_json::<Vec<AnnotationItem>>(&sidecar) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);

        let mut grouped: BTreeMap<String, Vec<AnnotationItem>> = BTreeMap::new();
        for item in items {
            let target_rel = if dir.is_empty() {
                item.file.clone()
            } else {
                format!("{dir}/{}", item.file)
            };
            if live_set.contains(&target_rel) {
                grouped.entry(target_rel).or_default().push(item);
            }
        }

        for (target_rel, group) in grouped {
            let syn_path = format!("{target_rel}#annotations");
            let extracted = chunk_annotations(&target_rel, &group);
            if extracted.chunks.is_empty() {
                continue;
            }
            live_set.insert(syn_path.clone());
            let hash = sha256_hex(extracted.raw_text.as_bytes());
            let byte_len = extracted.raw_text.len() as i64;
            if let Some(prev) = existing.get(&syn_path)
                && prev.mtime_ns == mtime_ns
                && prev.file_size == byte_len
            {
                continue;
            }
            store
                .upsert_document(
                    &syn_path,
                    "annotation",
                    &hash,
                    mtime_ns,
                    byte_len,
                    "indexed",
                    Some(&extracted.raw_text),
                    Some(extracted.parse_tier),
                    Some(&extracted.chunks),
                )
                .map_err(|e| RagError::new("store_error", e))?;
            updated += 1;
        }
    }
    Ok(updated)
}

pub struct SearchServices<'a> {
    pub embed_client: Option<&'a EmbeddingClient>,
    pub rerank_client: Option<&'a RerankClient>,
    pub rerank_model: &'a str,
}

#[allow(clippy::too_many_arguments)]
pub fn search_project(
    store: &mut RagStore,
    req: &SearchRequest,
    dirty_docs: &[DirtyDocument],
    allowed_paths: Option<&HashSet<String>>,
    services: SearchServices<'_>,
    cancelled: &AtomicBool,
) -> Result<SearchResponse, RagError> {
    check_cancelled(cancelled)?;
    let mut lex_queries: Vec<(f64, String)> = Vec::new();
    let mut vec_queries: Vec<(f64, String)> = Vec::new();
    let mut seen_lex: HashSet<String> = HashSet::new();
    let mut seen_vec: HashSet<String> = HashSet::new();
    let mut first = true;
    for item in &req.searches {
        let q = item.query.trim();
        if q.is_empty() {
            continue;
        }
        let weight = if first {
            first = false;
            2.0
        } else {
            1.0
        };
        match item.kind {
            SubSearchType::Lex => {
                if seen_lex.insert(q.to_owned()) {
                    lex_queries.push((weight, q.to_owned()));
                }
            }
            SubSearchType::Vec => {
                if seen_vec.insert(q.to_owned()) {
                    vec_queries.push((weight, q.to_owned()));
                }
            }
        }
    }
    if lex_queries.is_empty() && vec_queries.is_empty() {
        return Err(RagError::new(
            "invalid_request",
            "searches 不能为空，且至少需要一条有效查询。",
        ));
    }

    let intent = req.intent.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let limit = req.limit.clamp(1, 20);
    let candidate_limit = (limit * 4).clamp(24, 60);
    let under_dir = req.under_directory.as_deref();
    let stats = store.stats().map_err(|e| RagError::new("store_error", e))?;

    let mut ranked_lists: Vec<(f64, Vec<Candidate>)> = Vec::new();
    let mut degraded_reason: Option<String> = None;

    let dirty_paths: HashSet<String> = dirty_docs.iter().map(|d| d.rel_path.clone()).collect();
    let dirty_filter = |candidates: Vec<ScoredChunk>, dirty: bool| -> Vec<Candidate> {
        candidates
            .into_iter()
            .filter(|c| dirty || !dirty_paths.contains(&c.rel_path))
            .map(|chunk| Candidate {
                key: format!("db:{}", chunk.chunk_id),
                docid: chunk.docid,
                rel_path: chunk.rel_path,
                kind: chunk.kind,
                parse_tier: chunk.parse_tier,
                content_hash: Some(chunk.content_hash),
                heading_path: chunk.heading_path,
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                page_num: chunk.page_num,
                chunk_text: chunk.chunk_text,
                dirty_buffer: dirty,
            })
            .collect()
    };

    let want_lex = !lex_queries.is_empty();
    let want_vec = !vec_queries.is_empty();

    for (weight, q) in &lex_queries {
        check_cancelled(cancelled)?;
        let hits = store
            .search_fts(q, allowed_paths, under_dir, &req.extensions, candidate_limit)
            .map_err(|e| RagError::new("store_error", e))?;
        if !hits.is_empty() {
            ranked_lists.push((*weight, dirty_filter(hits, false)));
        }
    }

    let mut vec_used = false;
    if want_vec {
        if stats.vector_chunks == 0 {
            degraded_reason = Some("向量索引尚未构建（请确认 llama-server 可用并运行索引）".to_owned());
        } else if let Some(client) = services.embed_client
            && client.ensure_ready()
        {
            let vec_texts: Vec<&str> = vec_queries.iter().map(|(_, q)| q.as_str()).collect();
            match client.embed_queries(&vec_texts) {
                Ok(embeddings) => {
                    vec_used = true;
                    for ((weight, _), emb) in vec_queries.iter().zip(embeddings) {
                        check_cancelled(cancelled)?;
                        let hits = store
                            .search_vec(&emb, allowed_paths, under_dir, &req.extensions, candidate_limit)
                            .map_err(|e| RagError::new("store_error", e))?;
                        if !hits.is_empty() {
                            ranked_lists.push((*weight, dirty_filter(hits, false)));
                        }
                    }
                }
                Err(err) => {
                    degraded_reason = Some(format!("Embedding 服务请求失败，已降级为关键词检索: {err}"));
                }
            }
        } else {
            degraded_reason = Some("未检测到本地 llama-server Embedding 服务，已降级为关键词检索".to_owned());
        }
    }

    let mut dirty_terms: Vec<String> = lex_queries.iter().map(|(_, q)| q.clone()).collect();
    if let Some(intent_text) = intent {
        dirty_terms.push(intent_text.to_owned());
    }
    if dirty_terms.is_empty() {
        dirty_terms.extend(vec_queries.iter().map(|(_, q)| q.clone()));
    }
    let dirty_hits = search_dirty_buffers(dirty_docs, &dirty_terms, allowed_paths, under_dir, &req.extensions);
    if !dirty_hits.is_empty() {
        ranked_lists.push((2.0, dirty_hits));
    }

    if !want_lex && want_vec && !vec_used {
        let fallback_query = intent.unwrap_or(&vec_queries[0].1);
        let fallback = store
            .search_fts(
                fallback_query,
                allowed_paths,
                under_dir,
                &req.extensions,
                candidate_limit,
            )
            .map_err(|e| RagError::new("store_error", e))?;
        if !fallback.is_empty() {
            ranked_lists.push((1.5, dirty_filter(fallback, false)));
        }
    }

    let mut rrf_ranked = fuse_rrf(&ranked_lists);
    let mut rerank_used = false;
    let rerank_anchor = intent
        .or_else(|| vec_queries.first().map(|(_, q)| q.as_str()))
        .or_else(|| lex_queries.first().map(|(_, q)| q.as_str()));
    let should_rerank = req.rerank.unwrap_or(want_vec || intent.is_some());
    if should_rerank
        && rrf_ranked.len() > 1
        && let Some(anchor) = rerank_anchor
        && let Some(reranker) = services.rerank_client
        && reranker.ensure_ready()
    {
        check_cancelled(cancelled)?;
        rerank_used = apply_rerank_blend(
            store,
            reranker,
            services.rerank_model,
            anchor,
            &mut rrf_ranked,
        )?;
    }

    let mut snippet_queries: Vec<String> = lex_queries.iter().map(|(_, q)| q.clone()).collect();
    if let Some(intent_text) = intent {
        snippet_queries.push(intent_text.to_owned());
    }
    if snippet_queries.is_empty() {
        snippet_queries.extend(vec_queries.iter().map(|(_, q)| q.clone()));
    }
    let results = collapse_and_format(rrf_ranked, &snippet_queries, limit);
    let base_mode = match (want_lex, vec_used) {
        (true, true) => "hybrid",
        (false, true) => "semantic",
        _ => "keyword_only",
    };
    let mode_used = if rerank_used {
        format!("{base_mode}+rerank")
    } else {
        base_mode.to_owned()
    };

    Ok(SearchResponse {
        mode_used,
        degraded_reason,
        indexed_files: stats.indexed_files,
        pending_vectors: stats.pending_vectors,
        results,
    })
}

#[derive(Clone, Debug)]
struct Candidate {
    key: String,
    docid: String,
    rel_path: String,
    kind: String,
    parse_tier: String,
    content_hash: Option<String>,
    heading_path: String,
    line_start: Option<usize>,
    line_end: Option<usize>,
    page_num: Option<usize>,
    chunk_text: String,
    dirty_buffer: bool,
}

fn fuse_rrf(lists: &[(f64, Vec<Candidate>)]) -> Vec<(f64, Candidate)> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut items: HashMap<String, Candidate> = HashMap::new();

    for (weight, list) in lists {
        for (rank, candidate) in list.iter().enumerate() {
            let bonus = match rank {
                0 => 0.05,
                1 | 2 => 0.02,
                _ => 0.0,
            };
            let delta = (*weight / (RRF_K + (rank as f64) + 1.0)) + bonus;
            *scores.entry(candidate.key.clone()).or_insert(0.0) += delta;
            items
                .entry(candidate.key.clone())
                .or_insert_with(|| candidate.clone());
        }
    }

    let mut ranked: Vec<(f64, Candidate)> = scores
        .into_iter()
        .filter_map(|(k, score)| items.remove(&k).map(|c| (score, c)))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked
}

fn apply_rerank_blend(
    store: &mut RagStore,
    reranker: &RerankClient,
    rerank_model: &str,
    query: &str,
    ranked: &mut [(f64, Candidate)],
) -> Result<bool, RagError> {
    let count = ranked.len().min(RERANK_TOP_K);
    let keys: Vec<String> = ranked[..count]
        .iter()
        .map(|(_, c)| {
            let text_hash = sha256_hex(c.chunk_text.as_bytes());
            sha256_hex(format!("{rerank_model}\0{query}\0{text_hash}").as_bytes())
        })
        .collect();

    let cached = store
        .get_rerank_scores(&keys)
        .map_err(|e| RagError::new("store_error", e))?;

    let mut miss_indices = Vec::new();
    let mut miss_texts = Vec::new();
    for (idx, key) in keys.iter().enumerate() {
        if !cached.contains_key(key) {
            miss_indices.push(idx);
            miss_texts.push(ranked[idx].1.chunk_text.as_str());
        }
    }

    let mut fresh_scores: HashMap<usize, f64> = HashMap::new();
    if !miss_texts.is_empty() {
        let Ok(scores) = reranker.rerank(query, &miss_texts) else {
            return Ok(false);
        };
        let mut to_cache = Vec::with_capacity(scores.len());
        for (idx, score) in miss_indices.into_iter().zip(scores) {
            fresh_scores.insert(idx, score);
            to_cache.push((keys[idx].clone(), score));
        }
        let _ = store.put_rerank_scores(&to_cache);
    }

    let mut rerank_scores = Vec::with_capacity(count);
    for (idx, key) in keys.iter().enumerate() {
        let score = cached
            .get(key)
            .copied()
            .or_else(|| fresh_scores.get(&idx).copied())
            .unwrap_or(0.0);
        rerank_scores.push(score);
    }

    position_aware_blend(&mut ranked[..count], &rerank_scores);
    ranked[..count].sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(true)
}

pub fn position_aware_blend_scores(rrf_scores: &[f64], rerank_scores: &[f64]) -> Vec<f64> {
    let max_rrf = rrf_scores
        .iter()
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    rrf_scores
        .iter()
        .zip(rerank_scores)
        .enumerate()
        .map(|(idx, (rrf, rerank))| {
            let norm_rrf = (rrf / max_rrf).clamp(0.0, 1.0);
            let clamped_rerank = rerank.clamp(0.0, 1.0);
            let w_rrf = match idx {
                0..=2 => 0.75,
                3..=9 => 0.60,
                _ => 0.40,
            };
            w_rrf * norm_rrf + (1.0 - w_rrf) * clamped_rerank
        })
        .collect()
}

fn position_aware_blend(ranked: &mut [(f64, Candidate)], rerank_scores: &[f64]) {
    let rrf_scores: Vec<f64> = ranked.iter().map(|(s, _)| *s).collect();
    let blended = position_aware_blend_scores(&rrf_scores, rerank_scores);
    for ((slot, _), score) in ranked.iter_mut().zip(blended) {
        *slot = score;
    }
}

fn collapse_and_format(
    ranked: Vec<(f64, Candidate)>,
    queries: &[String],
    limit: usize,
) -> Vec<SearchHit> {
    let mut snippet_terms: Vec<String> = Vec::new();
    for lq in queries {
        for term in lq.split_whitespace() {
            snippet_terms.push(term.to_owned());
        }
    }

    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for (score, cand) in ranked {
        let base_path = cand
            .rel_path
            .strip_suffix("#annotations")
            .unwrap_or(&cand.rel_path)
            .to_owned();
        let count = per_file.entry(base_path).or_insert(0);
        if *count >= MAX_CHUNKS_PER_FILE {
            continue;
        }
        *count += 1;

        let markdown_cache = if cand.kind == "pdf"
            && matches!(cand.parse_tier.as_str(), "accurate" | "mineru" | "hybrid")
        {
            cand.content_hash
                .as_ref()
                .map(|h| format!(".folio/pdf-cache/{h}/full.md"))
        } else {
            None
        };

        out.push(SearchHit {
            docid: cand.docid,
            path: cand.rel_path,
            kind: cand.kind,
            parse_tier: cand.parse_tier,
            heading: cand.heading_path,
            line_start: cand.line_start,
            line_end: cand.line_end,
            page: cand.page_num,
            score: (score * 1000.0).round() / 1000.0,
            snippet: build_snippet(&cand.chunk_text, &snippet_terms),
            markdown_cache,
            dirty_buffer: cand.dirty_buffer,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn search_dirty_buffers(
    dirty_docs: &[DirtyDocument],
    terms: &[String],
    allowed_paths: Option<&HashSet<String>>,
    under_dir: Option<&str>,
    extensions: &[String],
) -> Vec<Candidate> {
    let mut query_tokens: Vec<String> = Vec::new();
    for raw in terms {
        for tok in raw.split_whitespace() {
            let lower = tok.trim().to_ascii_lowercase();
            if !lower.is_empty() && !query_tokens.contains(&lower) {
                query_tokens.push(lower);
            }
        }
    }
    if query_tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, Candidate)> = Vec::new();
    for doc in dirty_docs {
        if !path_matches_filters(&doc.rel_path, allowed_paths, under_dir, extensions) {
            continue;
        }
        let Some(kind) = doc_kind(&doc.rel_path) else {
            continue;
        };
        if kind == "pdf" {
            continue;
        }
        let extracted = chunk_text_document(&doc.rel_path, kind, &doc.text);
        let docid = sha256_hex(doc.rel_path.as_bytes())[..8].to_owned();
        for chunk in extracted.chunks {
            let lower = chunk.chunk_text.to_ascii_lowercase();
            let hits: usize = query_tokens
                .iter()
                .filter(|t| lower.contains(t.as_str()))
                .count();
            if hits > 0 {
                scored.push((
                    hits,
                    Candidate {
                        key: format!("dirty:{}:{}", doc.rel_path, chunk.seq),
                        docid: docid.clone(),
                        rel_path: doc.rel_path.clone(),
                        kind: kind.to_owned(),
                        parse_tier: "dirty_buffer".to_owned(),
                        content_hash: None,
                        heading_path: chunk.heading_path,
                        line_start: chunk.line_start,
                        line_end: chunk.line_end,
                        page_num: None,
                        chunk_text: chunk.chunk_text,
                        dirty_buffer: true,
                    },
                ));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, c)| c).collect()
}

pub fn clean_heading_text(raw: &str) -> String {
    let mut out = unescape_markdown_punct(raw.trim());
    out = out.replace("**", "").replace("__", "").replace('`', "");
    collapse_spaces(&out)
}

pub fn clean_markdown_snippet(chunk_text: &str) -> String {
    let body = chunk_text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n").map(|(_, after)| after))
        .unwrap_or(chunk_text)
        .trim();

    let mut cleaned_lines: Vec<String> = Vec::new();
    let mut in_block_math = false;
    let mut in_code_fence = false;

    for raw_line in body.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if trimmed == "$$" || trimmed == "\\[" || trimmed == "\\]" {
            if !in_block_math {
                cleaned_lines.push("[公式]".to_owned());
            }
            in_block_math = !in_block_math;
            continue;
        }
        if in_block_math {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let line = if in_code_fence {
            trimmed.to_owned()
        } else {
            clean_inline_markdown_line(trimmed)
        };
        if !line.is_empty() {
            cleaned_lines.push(line);
        }
    }

    collapse_spaces(&cleaned_lines.join(" "))
}

fn clean_inline_markdown_line(line: &str) -> String {
    let mut s = line.trim_start_matches('#').trim_start();
    if let Some(rest) = s.strip_prefix("> ") {
        s = rest.trim_start();
    }
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim_start();
            break;
        }
    }
    let with_math = replace_inline_math(s);
    let unescaped = unescape_markdown_punct(&with_math);
    unescaped
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
}

fn replace_inline_math(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("$$") {
        out.push_str(&replace_single_dollar_math(&rest[..start]));
        let after = &rest[start + 2..];
        if let Some(end) = after.find("$$") {
            out.push_str(" [公式] ");
            rest = &after[end + 2..];
        } else {
            out.push_str(&replace_single_dollar_math(after));
            rest = "";
            break;
        }
    }
    out.push_str(&replace_single_dollar_math(rest));
    out
}

fn replace_single_dollar_math(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('$') {
            let expr = after[..end].trim();
            if expr.chars().count() <= 18 && !expr.contains('\\') {
                out.push_str(expr);
            } else {
                out.push_str("[公式]");
            }
            rest = &after[end + 1..];
        } else {
            out.push('$');
            out.push_str(after);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn unescape_markdown_punct(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && let Some(&next) = chars.peek()
            && matches!(next, '.' | '-' | '_' | '*' | '#' | '(' | ')' | '[' | ']')
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(ch);
    }
    out
}

fn collapse_spaces(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn build_snippet(chunk_text: &str, terms: &[String]) -> String {
    let body = clean_markdown_snippet(chunk_text);
    if body.chars().count() <= SNIPPET_CHARS {
        return body;
    }

    let lower = body.to_ascii_lowercase();
    let match_pos = terms
        .iter()
        .filter(|t| !t.trim().is_empty())
        .filter_map(|t| lower.find(&t.trim().to_ascii_lowercase()))
        .min()
        .unwrap_or(0);

    let match_char_idx = body[..match_pos].chars().count();
    let start_char = match_char_idx.saturating_sub(28);
    let start_byte = body
        .char_indices()
        .nth(start_char)
        .map_or(0, |(idx, _)| idx);
    let end_byte = body[start_byte..]
        .char_indices()
        .nth(SNIPPET_CHARS)
        .map_or(body.len(), |(offset, _)| start_byte + offset);

    let mut snippet = String::new();
    if start_byte > 0 {
        snippet.push('…');
    }
    snippet.push_str(body[start_byte..end_byte].trim());
    if end_byte < body.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::{SubSearch, chunker::ChunkDraft};
    use tempfile::tempdir;

    #[test]
    fn rrf_rewards_multi_list_and_collapses_same_file() {
        let c1 = Candidate {
            key: "db:1".into(),
            docid: "aaaa1111".into(),
            rel_path: "a.md".into(),
            kind: "md".into(),
            parse_tier: "source".into(),
            content_hash: None,
            heading_path: "a.md > H1".into(),
            line_start: Some(1),
            line_end: Some(10),
            page_num: None,
            chunk_text: "[a.md > H1]\nAlpha beta gamma".into(),
            dirty_buffer: false,
        };
        let c2 = Candidate {
            key: "db:2".into(),
            rel_path: "a.md".into(),
            ..c1.clone()
        };
        let c3 = Candidate {
            key: "db:3".into(),
            rel_path: "a.md".into(),
            ..c1.clone()
        };
        let c4 = Candidate {
            key: "db:4".into(),
            docid: "bbbb2222".into(),
            rel_path: "b.md".into(),
            ..c1.clone()
        };
        let c5 = Candidate {
            key: "db:5".into(),
            rel_path: "a.md".into(),
            ..c1.clone()
        };
        let c6 = Candidate {
            key: "db:6".into(),
            rel_path: "a.md".into(),
            ..c1.clone()
        };

        let lists = vec![
            (
                2.0,
                vec![c1.clone(), c2.clone(), c3.clone(), c5, c6, c4.clone()],
            ),
            (1.0, vec![c4, c1]),
        ];
        let ranked = fuse_rrf(&lists);
        let hits = collapse_and_format(ranked, &["Alpha".to_owned()], 6);
        assert_eq!(hits.len(), 5);
        assert_eq!(hits[0].docid, "aaaa1111");
        assert!(hits.iter().any(|h| h.path == "b.md"));
    }

    #[test]
    fn position_aware_blend_protects_top_ranks_and_promotes_lower_hits() {
        let mut rrf = vec![0.10; 12];
        rrf[0] = 1.0; // rank 1
        rrf[11] = 0.2; // rank 12
        let mut rerank = vec![0.1; 12];
        rerank[0] = 0.0; // reranker dislikes rank 1
        rerank[11] = 1.0; // reranker strongly prefers rank 12

        let blended = position_aware_blend_scores(&rrf, &rerank);
        // Rank 1 keeps 0.75 weight from RRF (0.75) vs rank 12 getting 0.60*1.0 + 0.40*0.2 = 0.68.
        assert!((blended[0] - 0.75).abs() < 1e-6);
        assert!((blended[11] - 0.68).abs() < 1e-6);
    }

    #[test]
    fn sync_project_indexes_annotations_and_cached_accurate_pdfs() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("papers")).unwrap();
        fs::create_dir_all(root.join("papers/.folio")).unwrap();
        fs::write(root.join("papers/sample.pdf"), b"%PDF-1.4 dummy").unwrap();
        let pdf_hash = sha256_hex(b"%PDF-1.4 dummy");
        let cache = ocr::cache_dir(root, &pdf_hash);
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join("full.md"),
            "# Theorem\n\nProximal gradient convergence rate.\n",
        )
        .unwrap();
        fs::write(
            root.join("papers/.folio/annotations.json"),
            r#"[{"id":1,"file":"sample.pdf","page":2,"quote":"Proximal gradient","occurrence":0,"color":"yellow","comment":"Key step in proof"}]"#,
        )
        .unwrap();

        let db = root.join(".folio/rag.sqlite3");
        let cfg = RagConfig::default();
        let mut store = RagStore::open(&db, &cfg.embed_model, cfg.embed_dim).unwrap();
        let cancel = AtomicBool::new(false);
        let report = sync_project(
            root,
            &["papers/sample.pdf".to_owned()],
            &mut store,
            SyncOptions {
                config: &cfg,
                embed_client: None,
                force_rebuild: false,
                accurate_mode: None,
                accurate_paths: &[],
            },
            &cancel,
        )
        .unwrap();

        assert_eq!(report.updated_files, 2); // pdf + annotation
        let ann_hits = store
            .search_fts("Key step in proof", None, None, &[], 5)
            .unwrap();
        assert_eq!(ann_hits.len(), 1);
        assert_eq!(ann_hits[0].kind, "annotation");
        assert_eq!(ann_hits[0].page_num, Some(3));
    }

    #[test]
    fn search_degrades_gracefully_and_includes_dirty_buffers() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 4).unwrap();
        let chunks = vec![ChunkDraft {
            seq: 0,
            heading_path: "old.md > Sec".into(),
            line_start: Some(1),
            line_end: Some(3),
            page_num: None,
            chunk_text: "[old.md > Sec]\nOld disk content with Lagrange multiplier.".into(),
        }];
        store
            .upsert_document(
                "old.md",
                "md",
                "hash00001111",
                1,
                32,
                "indexed",
                Some("Old disk content with Lagrange multiplier."),
                Some("source"),
                Some(&chunks),
            )
            .unwrap();

        let dirty = vec![DirtyDocument {
            rel_path: "old.md".into(),
            text: "# Sec\nFresh unsaved Lagrange dual bound.\n".into(),
        }];
        let req = SearchRequest {
            searches: vec![SubSearch {
                kind: SubSearchType::Vec,
                query: "Lagrange".into(),
            }],
            intent: None,
            rerank: Some(true),
            extensions: vec![],
            under_directory: None,
            limit: 5,
        };
        let cancel = AtomicBool::new(false);
        let resp = search_project(
            &mut store,
            &req,
            &dirty,
            None,
            SearchServices {
                embed_client: None,
                rerank_client: None,
                rerank_model: "rerank",
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(resp.mode_used, "keyword_only");
        assert!(resp.degraded_reason.is_some());
        assert_eq!(resp.results.len(), 1);
        assert!(resp.results[0].dirty_buffer);
        assert!(resp.results[0].snippet.contains("Fresh unsaved"));

        let lex_only = SearchRequest {
            searches: vec![SubSearch {
                kind: SubSearchType::Lex,
                query: "Lagrange".into(),
            }],
            intent: None,
            rerank: None,
            extensions: vec![],
            under_directory: None,
            limit: 5,
        };
        let lex_resp = search_project(
            &mut store,
            &lex_only,
            &dirty,
            None,
            SearchServices {
                embed_client: None,
                rerank_client: None,
                rerank_model: "rerank",
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(lex_resp.mode_used, "keyword_only");
        assert!(lex_resp.degraded_reason.is_none());
        assert_eq!(lex_resp.results.len(), 1);
    }

    #[test]
    #[ignore = "requires local llama-server binary and GGUF models"]
    fn live_llama_server_end_to_end() {
        let embed_path = std::env::var("FOLIO_LIVE_EMBED_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/Qwen3-Embedding-0.6B-Q8_0.gguf".into());
        let rerank_path = std::env::var("FOLIO_LIVE_RERANK_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/qwen3-reranker-0.6b-q8_0.gguf".into());
        let pdf_src = std::env::var("FOLIO_LIVE_PDF").ok();

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("opt.md"),
            "# 凸优化笔记\n\n## KKT 互补松弛\n\n在强对偶成立时，最优解满足互补松弛条件 $\\lambda_i^* g_i(x^*) = 0$。\n",
        )
        .unwrap();
        fs::write(
            root.join("cooking.md"),
            "# 家常菜谱\n\n番茄炒蛋需要先将鸡蛋打散，热锅冷油滑炒后盛出。\n",
        )
        .unwrap();

        let mut files = vec!["opt.md".to_owned(), "cooking.md".to_owned()];
        if let Some(pdf) = pdf_src.as_deref().filter(|p| Path::new(p).exists()) {
            fs::copy(pdf, root.join("adam.pdf")).unwrap();
            files.push("adam.pdf".to_owned());
        }

        let cfg = RagConfig {
            embed_url: "http://127.0.0.1:18111".into(),
            rerank_url: "http://127.0.0.1:18112".into(),
            embed_model_path: embed_path,
            rerank_model_path: rerank_path,
            auto_manage_server: true,
            ..RagConfig::default()
        };

        let embed_client = EmbeddingClient::from_config(&cfg).unwrap();
        let rerank_client = RerankClient::from_config(&cfg).unwrap();
        let db = root.join(".folio/rag.sqlite3");
        let mut store = RagStore::open(&db, &cfg.embed_model, cfg.embed_dim).unwrap();
        let cancel = AtomicBool::new(false);

        let report = sync_project(
            root,
            &files,
            &mut store,
            SyncOptions {
                config: &cfg,
                embed_client: Some(&embed_client),
                force_rebuild: false,
                accurate_mode: None,
                accurate_paths: &[],
            },
            &cancel,
        )
        .unwrap();
        assert!(report.embed_server_ready);
        assert!(report.embedded_chunks >= 2);
        assert_eq!(report.pending_vectors, 0);

        let resp = search_project(
            &mut store,
            &SearchRequest {
                searches: vec![
                    SubSearch {
                        kind: SubSearchType::Lex,
                        query: "KKT".into(),
                    },
                    SubSearch {
                        kind: SubSearchType::Vec,
                        query: "complementary slackness condition".into(),
                    },
                ],
                intent: Some("什么时候满足互补松弛？".into()),
                rerank: Some(true),
                extensions: vec![],
                under_directory: None,
                limit: 3,
            },
            &[],
            None,
            SearchServices {
                embed_client: Some(&embed_client),
                rerank_client: Some(&rerank_client),
                rerank_model: &cfg.rerank_model,
            },
            &cancel,
        )
        .unwrap();
        println!("live_resp: {resp:#?}");
        assert_eq!(resp.mode_used, "hybrid+rerank");
        assert_eq!(resp.results[0].path, "opt.md");

        if files.iter().any(|f| f == "adam.pdf") {
            let pdf_resp = search_project(
                &mut store,
                &SearchRequest {
                    searches: vec![
                        SubSearch {
                            kind: SubSearchType::Lex,
                            query: "Adam moment".into(),
                        },
                        SubSearch {
                            kind: SubSearchType::Vec,
                            query: "Adam bias-corrected first and second moment estimates".into(),
                        },
                    ],
                    intent: Some("Adam bias-corrected first and second moment estimates".into()),
                    rerank: Some(true),
                    extensions: vec![],
                    under_directory: None,
                    limit: 3,
                },
                &[],
                None,
                SearchServices {
                    embed_client: Some(&embed_client),
                    rerank_client: Some(&rerank_client),
                    rerank_model: &cfg.rerank_model,
                },
                &cancel,
            )
            .unwrap();
            println!("pdf_resp: {pdf_resp:#?}");
            assert_eq!(pdf_resp.mode_used, "hybrid+rerank");
            assert_eq!(pdf_resp.results[0].path, "adam.pdf");
            assert!(pdf_resp.results[0].page.is_some());
        }

        crate::rag::client::shutdown_managed_servers();
    }

    #[test]
    #[ignore = "requires PaddleOCR token and local llama-server GGUFs"]
    fn live_accurate_pdf_upgrade_and_search() {
        let embed_path = std::env::var("FOLIO_LIVE_EMBED_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/Qwen3-Embedding-0.6B-Q8_0.gguf".into());
        let rerank_path = std::env::var("FOLIO_LIVE_RERANK_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/qwen3-reranker-0.6b-q8_0.gguf".into());
        let pdf_src = std::env::var("FOLIO_LIVE_PDF").expect("set FOLIO_LIVE_PDF");
        let paddleocr_token = ocr::resolve_ocr_token(&RagConfig::default());
        assert!(
            !paddleocr_token.is_empty(),
            "set PADDLEOCR_ACCESS_TOKEN or configure ~/.pi/agent/mcp.json"
        );

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::copy(&pdf_src, root.join("adam.pdf")).unwrap();

        let cfg = RagConfig {
            embed_url: "http://127.0.0.1:18121".into(),
            rerank_url: "http://127.0.0.1:18122".into(),
            embed_model_path: embed_path,
            rerank_model_path: rerank_path,
            auto_manage_server: true,
            paddleocr_token,
            ..RagConfig::default()
        };
        let embed_client = EmbeddingClient::from_config(&cfg).unwrap();
        let rerank_client = RerankClient::from_config(&cfg).unwrap();
        let db = root.join(".folio/rag.sqlite3");
        let mut store = RagStore::open(&db, &cfg.embed_model, cfg.embed_dim).unwrap();
        let cancel = AtomicBool::new(false);

        let report = sync_project(
            root,
            &["adam.pdf".to_owned()],
            &mut store,
            SyncOptions {
                config: &cfg,
                embed_client: Some(&embed_client),
                force_rebuild: false,
                accurate_mode: Some(AccuratePdfMode::All),
                accurate_paths: &[],
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(report.accurate_upgraded_pdfs, 1);
        assert!(report.accurate_failed_pdfs.is_empty());
        assert_eq!(report.pending_vectors, 0);

        let pdf_hash = sha256_hex(&fs::read(root.join("adam.pdf")).unwrap());
        assert!(ocr::cache_dir(root, &pdf_hash).join("full.md").exists());
        assert!(ocr::cache_dir(root, &pdf_hash).join("pages.json").exists());

        let resp = search_project(
            &mut store,
            &SearchRequest {
                searches: vec![
                    SubSearch {
                        kind: SubSearchType::Lex,
                        query: "AdaMax bias correction".into(),
                    },
                    SubSearch {
                        kind: SubSearchType::Vec,
                        query: "Adam convergence regret bound beta1 beta2".into(),
                    },
                ],
                intent: Some("Adam convergence regret bound beta1 beta2".into()),
                rerank: Some(true),
                extensions: vec![],
                under_directory: None,
                limit: 3,
            },
            &[],
            None,
            SearchServices {
                embed_client: Some(&embed_client),
                rerank_client: Some(&rerank_client),
                rerank_model: &cfg.rerank_model,
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(resp.mode_used, "hybrid+rerank");
        assert!(!resp.results.is_empty());
        assert_eq!(resp.results[0].parse_tier, "accurate");
        assert!(resp.results[0].page.is_some());

        crate::rag::client::shutdown_managed_servers();
    }

    #[test]
    fn clean_snippet_strips_markdown_and_centers_on_match() {
        let raw = "[Notes/线性表.md > 2\\.1\\.1 什么是线性表]\n### 2\\.1\\.1 什么是**线性表**\n\n线性表是 $n$ 个数据元素的**有限序列**：\n$$\nL = (a_1, a_2, \\dots, a_n)\n$$\n当需要描述复杂的逻辑前驱与后继关系时，可以借助 `ArrayList` 或链式结构存储数据元素。";
        let snippet = build_snippet(raw, &["有限序列".to_owned()]);
        assert!(!snippet.contains("###"));
        assert!(!snippet.contains("**"));
        assert!(!snippet.contains("\\."));
        assert!(!snippet.contains("$$"));
        assert!(snippet.contains("n 个数据元素的有限序列"));
        assert!(snippet.contains("[公式]"));
        assert_eq!(clean_heading_text("2\\.1\\.1 **线性表**"), "2.1.1 线性表");
    }

    #[test]
    fn derived_pdf_filtering_uses_fingerprints_only_not_filenames() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("notes.tex"), "\\section{Intro}\nUniqueTeXToken\n").unwrap();
        // Same-named independent PDF without Folio fingerprint must still be indexed.
        fs::write(root.join("notes.pdf"), b"%PDF-1.4\n% independent slides").unwrap();
        // Renamed TeX build recorded in `.folio/derived-pdfs.json`.
        let tex_out = b"%PDF-1.4\n% compiled tex output bytes";
        fs::write(root.join("renamed-tex-output.pdf"), tex_out).unwrap();
        crate::rag::record_derived_pdf(
            root,
            &root.join("notes.tex"),
            &root.join("renamed-tex-output.pdf"),
            tex_out,
            "tex",
        )
        .unwrap();
        // Renamed Markdown/krilla export stamped with protocol metadata.
        fs::write(
            root.join("renamed-md-export.pdf"),
            b"%PDF-1.7\n1 0 obj << /Creator (Folio) /Keywords (folio-derived) >> endobj",
        )
        .unwrap();

        let eligible = crate::rag::collect_eligible_paths(vec![
            "notes.tex".into(),
            "notes.pdf".into(),
            "renamed-tex-output.pdf".into(),
            "renamed-md-export.pdf".into(),
        ]);
        assert_eq!(eligible.len(), 4);

        let cfg = RagConfig::default();
        let db = root.join(".folio/rag.sqlite3");
        let mut store = RagStore::open(&db, &cfg.embed_model, cfg.embed_dim).unwrap();
        let cancel = AtomicBool::new(false);
        let report = sync_project(
            root,
            &eligible,
            &mut store,
            SyncOptions {
                config: &cfg,
                embed_client: None,
                force_rebuild: false,
                accurate_mode: None,
                accurate_paths: &[],
            },
            &cancel,
        )
        .unwrap();

        // Only notes.tex + independent notes.pdf are indexed; the two fingerprinted exports are derived.
        let stats = store.stats().unwrap();
        assert_eq!(stats.indexed_files, 2);
        assert_eq!(stats.needs_ocr_files, vec!["notes.pdf".to_owned()]);
        assert_eq!(report.needs_ocr_files, vec!["notes.pdf".to_owned()]);
        assert_eq!(
            store
                .pdf_upgrade_candidates(AccuratePdfMode::All, &[])
                .unwrap()
                .len(),
            1
        );
    }
}
