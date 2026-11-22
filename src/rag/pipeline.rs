//! Incremental indexing, annotation sync, hybrid RRF fusion, and position-aware reranking.

use super::{
    DirtyDocument, RagConfig, RagError, ReindexReport, SearchHit, SearchMode, SearchRequest,
    SearchResponse,
    chunker::{chunk_annotations, chunk_text_document, extract_and_chunk_pdf},
    client::{EmbeddingClient, RerankClient},
    doc_kind, mineru, sha256_hex,
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
const MAX_CHUNKS_PER_FILE: usize = 2;
const SNIPPET_CHARS: usize = 320;
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
    pub upgrade_pdfs: bool,
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
            if let Some(cached) = mineru::load_cached_pdf(root, rel_path, &content_hash) {
                store
                    .upsert_document(
                        rel_path,
                        kind,
                        &content_hash,
                        mtime_ns,
                        file_size,
                        "indexed",
                        Some(&cached.raw_text),
                        Some(cached.parse_tier),
                        Some(&cached.chunks),
                    )
                    .map_err(|e| RagError::new("store_error", e))?;
                updated_files += 1;
            } else {
                match extract_and_chunk_pdf(rel_path, bytes) {
                    Ok(extracted) => {
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

    let mut mineru_stats = mineru::MinerUUpgradeStats::default();
    if opts.upgrade_pdfs {
        mineru_stats = mineru::upgrade_pdfs(root, store, opts.config, cancelled)?;
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
        mineru_upgraded_pdfs: mineru_stats.upgraded_pdfs,
        mineru_failed_pdfs: mineru_stats.failed_pdfs,
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
    let query = req.query.trim();
    if query.is_empty() {
        return Err(RagError::new("invalid_request", "query 不能为空。"));
    }

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
                heading_path: chunk.heading_path,
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                page_num: chunk.page_num,
                chunk_text: chunk.chunk_text,
                dirty_buffer: dirty,
            })
            .collect()
    };

    let want_lex = matches!(req.mode, SearchMode::Hybrid | SearchMode::Keyword);
    let want_vec = matches!(req.mode, SearchMode::Hybrid | SearchMode::Semantic);

    if want_lex {
        let primary = store
            .search_fts(query, allowed_paths, under_dir, &req.extensions, candidate_limit)
            .map_err(|e| RagError::new("store_error", e))?;
        if !primary.is_empty() {
            ranked_lists.push((2.0, dirty_filter(primary, false)));
        }

        for extra in &req.lex_queries {
            check_cancelled(cancelled)?;
            let extra = extra.trim();
            if extra.is_empty() || extra == query {
                continue;
            }
            let hits = store
                .search_fts(extra, allowed_paths, under_dir, &req.extensions, candidate_limit)
                .map_err(|e| RagError::new("store_error", e))?;
            if !hits.is_empty() {
                ranked_lists.push((1.0, dirty_filter(hits, false)));
            }
        }
    }

    let mut vec_used = false;
    if want_vec {
        if stats.vector_chunks == 0 {
            degraded_reason = Some("向量索引尚未构建（请确认 llama-server 可用并运行索引）".to_owned());
        } else if let Some(client) = services.embed_client
            && client.ensure_ready()
        {
            let mut vec_texts = vec![query];
            for q in &req.vec_queries {
                let q = q.trim();
                if !q.is_empty() && q != query {
                    vec_texts.push(q);
                }
            }
            match client.embed_queries(&vec_texts) {
                Ok(embeddings) => {
                    vec_used = true;
                    for (idx, emb) in embeddings.into_iter().enumerate() {
                        check_cancelled(cancelled)?;
                        let weight = if idx == 0 { 2.0 } else { 1.0 };
                        let hits = store
                            .search_vec(&emb, allowed_paths, under_dir, &req.extensions, candidate_limit)
                            .map_err(|e| RagError::new("store_error", e))?;
                        if !hits.is_empty() {
                            ranked_lists.push((weight, dirty_filter(hits, false)));
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

    let mut dirty_terms = vec![query.to_owned()];
    dirty_terms.extend(req.lex_queries.iter().cloned());
    let dirty_hits = search_dirty_buffers(dirty_docs, &dirty_terms, allowed_paths, under_dir, &req.extensions);
    if !dirty_hits.is_empty() {
        ranked_lists.push((2.0, dirty_hits));
    }

    if req.mode == SearchMode::Semantic && !vec_used {
        let fallback = store
            .search_fts(query, allowed_paths, under_dir, &req.extensions, candidate_limit)
            .map_err(|e| RagError::new("store_error", e))?;
        if !fallback.is_empty() {
            ranked_lists.push((1.5, dirty_filter(fallback, false)));
        }
    }

    let mut rrf_ranked = fuse_rrf(&ranked_lists);
    let mut rerank_used = false;
    if req.rerank.unwrap_or(true)
        && rrf_ranked.len() > 1
        && let Some(reranker) = services.rerank_client
        && reranker.ensure_ready()
    {
        check_cancelled(cancelled)?;
        rerank_used = apply_rerank_blend(
            store,
            reranker,
            services.rerank_model,
            query,
            &mut rrf_ranked,
        )?;
    }

    let results = collapse_and_format(rrf_ranked, query, &req.lex_queries, limit);
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
    query: &str,
    lex_queries: &[String],
    limit: usize,
) -> Vec<SearchHit> {
    let mut snippet_terms: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
    for lq in lex_queries {
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

fn build_snippet(chunk_text: &str, terms: &[String]) -> String {
    let body = chunk_text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n").map(|(_, after)| after))
        .unwrap_or(chunk_text)
        .trim();
    if body.chars().count() <= SNIPPET_CHARS {
        return body.to_owned();
    }

    let lower = body.to_ascii_lowercase();
    let match_pos = terms
        .iter()
        .filter(|t| !t.is_empty())
        .filter_map(|t| lower.find(&t.to_ascii_lowercase()))
        .min()
        .unwrap_or(0);

    let match_char_idx = body[..match_pos].chars().count();
    let start_char = match_char_idx.saturating_sub(SNIPPET_CHARS / 3);
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
        snippet.push_str("…");
    }
    snippet.push_str(body[start_byte..end_byte].trim());
    if end_byte < body.len() {
        snippet.push_str("…");
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::{SearchMode, chunker::ChunkDraft};
    use tempfile::tempdir;

    #[test]
    fn rrf_rewards_multi_list_and_collapses_same_file() {
        let c1 = Candidate {
            key: "db:1".into(),
            docid: "aaaa1111".into(),
            rel_path: "a.md".into(),
            kind: "md".into(),
            parse_tier: "source".into(),
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

        let lists = vec![
            (2.0, vec![c1.clone(), c2.clone(), c3.clone(), c4.clone()]),
            (1.0, vec![c4, c1]),
        ];
        let ranked = fuse_rrf(&lists);
        let hits = collapse_and_format(ranked, "Alpha", &[], 5);
        assert_eq!(hits.len(), 3);
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
    fn sync_project_indexes_annotations_and_cached_mineru_pdfs() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("papers")).unwrap();
        fs::create_dir_all(root.join("papers/.folio")).unwrap();
        fs::write(root.join("papers/sample.pdf"), b"%PDF-1.4 dummy").unwrap();
        let pdf_hash = sha256_hex(b"%PDF-1.4 dummy");
        let cache = mineru::cache_dir(root, &pdf_hash);
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
                upgrade_pdfs: false,
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
            query: "Lagrange".into(),
            lex_queries: vec![],
            vec_queries: vec![],
            mode: SearchMode::Hybrid,
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
                upgrade_pdfs: false,
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
                query: "什么时候满足互补松弛？".into(),
                lex_queries: vec!["KKT".into()],
                vec_queries: vec!["complementary slackness condition".into()],
                mode: SearchMode::Hybrid,
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
                    query: "Adam bias-corrected first and second moment estimates".into(),
                    lex_queries: vec!["Adam".into(), "moment".into()],
                    vec_queries: vec![],
                    mode: SearchMode::Hybrid,
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
    #[ignore = "requires MINERU_API_KEY and local llama-server GGUFs"]
    fn live_mineru_pdf_upgrade_and_search() {
        let embed_path = std::env::var("FOLIO_LIVE_EMBED_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/Qwen3-Embedding-0.6B-Q8_0.gguf".into());
        let rerank_path = std::env::var("FOLIO_LIVE_RERANK_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/qwen3-reranker-0.6b-q8_0.gguf".into());
        let pdf_src = std::env::var("FOLIO_LIVE_PDF").expect("set FOLIO_LIVE_PDF");
        let mineru_key = std::env::var("MINERU_API_KEY").expect("set MINERU_API_KEY");

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::copy(&pdf_src, root.join("adam.pdf")).unwrap();

        let cfg = RagConfig {
            embed_url: "http://127.0.0.1:18121".into(),
            rerank_url: "http://127.0.0.1:18122".into(),
            embed_model_path: embed_path,
            rerank_model_path: rerank_path,
            auto_manage_server: true,
            mineru_api_key: mineru_key,
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
                upgrade_pdfs: true,
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(report.mineru_upgraded_pdfs, 1);
        assert!(report.mineru_failed_pdfs.is_empty());
        assert_eq!(report.pending_vectors, 0);

        let pdf_hash = sha256_hex(&fs::read(root.join("adam.pdf")).unwrap());
        assert!(mineru::cache_dir(root, &pdf_hash).join("full.md").exists());
        assert!(mineru::cache_dir(root, &pdf_hash).join("pages.json").exists());

        let resp = search_project(
            &mut store,
            &SearchRequest {
                query: "Adam convergence regret bound beta1 beta2".into(),
                lex_queries: vec!["AdaMax".into(), "bias correction".into()],
                vec_queries: vec![],
                mode: SearchMode::Hybrid,
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
        assert_eq!(resp.results[0].parse_tier, "mineru");
        assert!(resp.results[0].page.is_some());

        crate::rag::client::shutdown_managed_servers();
    }
}
