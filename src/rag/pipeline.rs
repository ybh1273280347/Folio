//! Incremental indexing, hybrid RRF fusion, dirty-buffer fallback, and snippet formatting.

use super::{
    DirtyDocument, RagError, ReindexReport, SearchHit, SearchMode, SearchRequest, SearchResponse,
    chunker::{chunk_text_document, extract_and_chunk_pdf},
    client::EmbeddingClient,
    doc_kind, sha256_hex,
    store::{RagStore, ScoredChunk, path_matches_filters},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::UNIX_EPOCH,
};

const RRF_K: f64 = 60.0;
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

pub fn sync_project(
    root: &Path,
    eligible_paths: &[String],
    store: &mut RagStore,
    embed_model: &str,
    embed_dim: usize,
    client: Option<&EmbeddingClient>,
    cancelled: &AtomicBool,
    force_rebuild: bool,
) -> Result<ReindexReport, RagError> {
    check_cancelled(cancelled)?;
    if force_rebuild {
        store
            .reset_index(embed_model, embed_dim)
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

    let removed_files = store
        .deactivate_missing(&live_set)
        .map_err(|e| RagError::new("store_error", e))?;

    let mut embedded_chunks = 0usize;
    let embed_server_ready = client.is_some_and(EmbeddingClient::ensure_ready);
    if embed_server_ready && let Some(client) = client {
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
        needs_ocr_files: stats.needs_ocr_files,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn search_project(
    store: &RagStore,
    req: &SearchRequest,
    dirty_docs: &[DirtyDocument],
    allowed_paths: Option<&HashSet<String>>,
    client: Option<&EmbeddingClient>,
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
        } else if let Some(client) = client
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

    // Search open dirty buffers in memory so unsaved edits are immediately discoverable.
    let mut dirty_terms = vec![query.to_owned()];
    dirty_terms.extend(req.lex_queries.iter().cloned());
    let dirty_hits = search_dirty_buffers(dirty_docs, &dirty_terms, allowed_paths, under_dir, &req.extensions);
    if !dirty_hits.is_empty() {
        ranked_lists.push((2.0, dirty_hits));
    }

    // If Semantic mode was requested while vector service was offline, fall back to FTS5 rather than failing empty.
    if req.mode == SearchMode::Semantic && !vec_used {
        let fallback = store
            .search_fts(query, allowed_paths, under_dir, &req.extensions, candidate_limit)
            .map_err(|e| RagError::new("store_error", e))?;
        if !fallback.is_empty() {
            ranked_lists.push((1.5, dirty_filter(fallback, false)));
        }
    }

    let results = fuse_rrf_and_collapse(&ranked_lists, query, &req.lex_queries, limit);
    let mode_used = match (want_lex, vec_used) {
        (true, true) => "hybrid",
        (false, true) => "semantic",
        _ => "keyword_only",
    }
    .to_owned();

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

fn fuse_rrf_and_collapse(
    lists: &[(f64, Vec<Candidate>)],
    query: &str,
    lex_queries: &[String],
    limit: usize,
) -> Vec<SearchHit> {
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

    let mut snippet_terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    for lq in lex_queries {
        for term in lq.split_whitespace() {
            snippet_terms.push(term.to_owned());
        }
    }

    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for (score, cand) in ranked {
        let count = per_file.entry(cand.rel_path.clone()).or_insert(0);
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
        let hits = fuse_rrf_and_collapse(&lists, "Alpha", &[], 5);
        // Max 2 per file, so c3 is collapsed and b.md enters Top 3.
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].docid, "aaaa1111");
        assert!(hits.iter().any(|h| h.path == "b.md"));
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
            extensions: vec![],
            under_directory: None,
            limit: 5,
        };
        let cancel = AtomicBool::new(false);
        let resp = search_project(&store, &req, &dirty, None, None, &cancel).unwrap();
        assert_eq!(resp.mode_used, "keyword_only");
        assert!(resp.degraded_reason.is_some());
        assert_eq!(resp.results.len(), 1);
        assert!(resp.results[0].dirty_buffer);
        assert!(resp.results[0].snippet.contains("Fresh unsaved"));
    }
}
