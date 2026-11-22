//! SQLite storage layer using WAL, FTS5 trigram, and statically linked sqlite-vec.

use super::chunker::ChunkDraft;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Once,
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA_VERSION: &str = "1";
const CHUNKER_VERSION: &str = "1";
const FTS_TOKENIZER: &str = "trigram";

static REGISTER_VEC: Once = Once::new();

pub fn register_sqlite_vec() {
    REGISTER_VEC.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::ffi::c_int,
        >(sqlite_vec::sqlite3_vec_init as *const ())));
    });
}

#[derive(Clone, Debug)]
pub struct DocumentMeta {
    pub rel_path: String,
    pub mtime_ns: i64,
    pub file_size: i64,
}

#[derive(Clone, Debug)]
pub struct PendingChunk {
    pub chunk_id: i64,
    pub chunk_text: String,
}

#[derive(Clone, Debug)]
pub struct ScoredChunk {
    pub chunk_id: i64,
    pub docid: String,
    pub rel_path: String,
    pub kind: String,
    pub parse_tier: String,
    pub heading_path: String,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
    pub page_num: Option<usize>,
    pub chunk_text: String,
    pub raw_score: f64,
}

#[derive(Clone, Debug)]
pub struct StoreStats {
    pub indexed_files: usize,
    pub total_chunks: usize,
    pub vector_chunks: usize,
    pub pending_vectors: usize,
    pub needs_ocr_files: Vec<String>,
}

pub struct RagStore {
    conn: Connection,
    embed_dim: usize,
}

impl RagStore {
    pub fn open(db_path: &Path, embed_model: &str, embed_dim: usize) -> Result<Self, String> {
        register_sqlite_vec();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建 RAG 数据目录失败: {e}"))?;
        }
        let conn = Connection::open(db_path).map_err(|e| format!("打开 RAG 数据库失败: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| format!("配置 SQLite PRAGMA 失败: {e}"))?;

        let mut store = Self { conn, embed_dim };
        store.ensure_schema(embed_model, embed_dim, false)?;
        Ok(store)
    }

    pub fn reset_index(&mut self, embed_model: &str, embed_dim: usize) -> Result<(), String> {
        self.embed_dim = embed_dim;
        self.ensure_schema(embed_model, embed_dim, true)
    }

    fn ensure_schema(
        &mut self,
        embed_model: &str,
        embed_dim: usize,
        force_reset: bool,
    ) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )
            .map_err(|e| e.to_string())?;

        let prev_schema = self.get_meta("schema_version")?;
        let prev_dim = self.get_meta("embed_dim")?;
        let prev_model = self.get_meta("embed_model")?;
        let prev_chunker = self.get_meta("chunker_version")?;

        let rebuild_all = force_reset
            || prev_schema.as_deref().is_some_and(|v| v != SCHEMA_VERSION)
            || prev_chunker.as_deref().is_some_and(|v| v != CHUNKER_VERSION);

        let rebuild_vectors = rebuild_all
            || prev_dim
                .as_deref()
                .and_then(|v| v.parse::<usize>().ok())
                .is_some_and(|d| d != embed_dim)
            || prev_model.as_deref().is_some_and(|m| m != embed_model);

        if rebuild_all {
            self.conn
                .execute_batch(
                    "DROP TRIGGER IF EXISTS chunks_ai;
                     DROP TRIGGER IF EXISTS chunks_ad;
                     DROP TRIGGER IF EXISTS chunks_au;
                     DROP TABLE IF EXISTS chunks_vec;
                     DROP TABLE IF EXISTS chunks_fts;
                     DROP TABLE IF EXISTS chunks;
                     DROP TABLE IF EXISTS documents;
                     DROP TABLE IF EXISTS contents;
                     DROP TABLE IF EXISTS rerank_cache;",
                )
                .map_err(|e| e.to_string())?;
        } else if rebuild_vectors {
            self.conn
                .execute_batch("DROP TABLE IF EXISTS chunks_vec;")
                .map_err(|e| e.to_string())?;
        }

        let sql = format!(
            "CREATE TABLE IF NOT EXISTS contents (
                content_hash TEXT PRIMARY KEY,
                raw_text     TEXT NOT NULL,
                parse_tier   TEXT NOT NULL,
                byte_len     INTEGER NOT NULL,
                created_at   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS documents (
                id           INTEGER PRIMARY KEY,
                docid        TEXT NOT NULL UNIQUE,
                rel_path     TEXT NOT NULL UNIQUE,
                kind         TEXT NOT NULL,
                content_hash TEXT NOT NULL REFERENCES contents(content_hash),
                mtime_ns     INTEGER NOT NULL,
                file_size    INTEGER NOT NULL,
                status       TEXT NOT NULL DEFAULT 'indexed',
                active       INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS chunks (
                id           INTEGER PRIMARY KEY,
                content_hash TEXT NOT NULL,
                seq          INTEGER NOT NULL,
                heading_path TEXT NOT NULL,
                line_start   INTEGER,
                line_end     INTEGER,
                page_num     INTEGER,
                chunk_text   TEXT NOT NULL,
                UNIQUE(content_hash, seq)
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(content_hash);
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                chunk_text,
                heading_path,
                content='chunks',
                content_rowid='id',
                tokenize='trigram'
            );
            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, chunk_text, heading_path)
                VALUES (new.id, new.chunk_text, new.heading_path);
            END;
            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, chunk_text, heading_path)
                VALUES ('delete', old.id, old.chunk_text, old.heading_path);
            END;
            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, chunk_text, heading_path)
                VALUES ('delete', old.id, old.chunk_text, old.heading_path);
                INSERT INTO chunks_fts(rowid, chunk_text, heading_path)
                VALUES (new.id, new.chunk_text, new.heading_path);
            END;
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
                chunk_id INTEGER PRIMARY KEY,
                embedding float[{embed_dim}] distance_metric=cosine
            );
            CREATE TABLE IF NOT EXISTS rerank_cache (
                cache_key TEXT PRIMARY KEY,
                score     REAL NOT NULL,
                ts        INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ocr_jobs (
                content_hash TEXT PRIMARY KEY,
                rel_path     TEXT NOT NULL,
                batch_id     TEXT,
                state        TEXT NOT NULL,
                err_msg      TEXT,
                updated_at   INTEGER NOT NULL
            );"
        );
        self.conn
            .execute_batch(&sql)
            .map_err(|e| format!("初始化 RAG 表结构失败: {e}"))?;

        self.set_meta("schema_version", SCHEMA_VERSION)?;
        self.set_meta("chunker_version", CHUNKER_VERSION)?;
        self.set_meta("fts_tokenizer", FTS_TOKENIZER)?;
        self.set_meta("embed_model", embed_model)?;
        self.set_meta("embed_dim", &embed_dim.to_string())?;
        Ok(())
    }

    fn get_meta(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn active_documents(&self) -> Result<HashMap<String, DocumentMeta>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT rel_path, mtime_ns, file_size
                 FROM documents
                 WHERE active = 1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DocumentMeta {
                    rel_path: row.get(0)?,
                    mtime_ns: row.get(1)?,
                    file_size: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut map = HashMap::new();
        for row in rows {
            let meta = row.map_err(|e| e.to_string())?;
            map.insert(meta.rel_path.clone(), meta);
        }
        Ok(map)
    }

    pub fn has_content(&self, content_hash: &str) -> Result<bool, String> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM contents WHERE content_hash = ?1",
                [content_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(exists.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_document(
        &mut self,
        rel_path: &str,
        kind: &str,
        content_hash: &str,
        mtime_ns: i64,
        file_size: i64,
        status: &str,
        raw_text: Option<&str>,
        parse_tier: Option<&str>,
        chunks: Option<&[ChunkDraft]>,
    ) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        if let (Some(raw), Some(tier), Some(chunk_list)) = (raw_text, parse_tier, chunks) {
            let now = unix_secs();
            tx.execute(
                "INSERT OR IGNORE INTO contents(content_hash, raw_text, parse_tier, byte_len, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![content_hash, raw, tier, raw.len() as i64, now],
            )
            .map_err(|e| e.to_string())?;

            let existing_chunks: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM chunks WHERE content_hash = ?1",
                    [content_hash],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if existing_chunks == 0 {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO chunks(content_hash, seq, heading_path, line_start, line_end, page_num, chunk_text)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .map_err(|e| e.to_string())?;
                for chunk in chunk_list {
                    stmt.execute(params![
                        content_hash,
                        chunk.seq as i64,
                        chunk.heading_path,
                        chunk.line_start.map(|v| v as i64),
                        chunk.line_end.map(|v| v as i64),
                        chunk.page_num.map(|v| v as i64),
                        chunk.chunk_text,
                    ])
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        let docid = allocate_docid(&tx, rel_path, content_hash)?;
        tx.execute(
            "INSERT INTO documents(docid, rel_path, kind, content_hash, mtime_ns, file_size, status, active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)
             ON CONFLICT(rel_path) DO UPDATE SET
                kind = excluded.kind,
                content_hash = excluded.content_hash,
                mtime_ns = excluded.mtime_ns,
                file_size = excluded.file_size,
                status = excluded.status,
                active = 1",
            params![docid, rel_path, kind, content_hash, mtime_ns, file_size, status],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())
    }

    pub fn deactivate_missing(&mut self, live_paths: &HashSet<String>) -> Result<usize, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT rel_path FROM documents WHERE active = 1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut missing = Vec::new();
        for row in rows {
            let path = row.map_err(|e| e.to_string())?;
            if !live_paths.contains(&path) {
                missing.push(path);
            }
        }
        drop(stmt);

        if missing.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        for path in &missing {
            tx.execute("DELETE FROM documents WHERE rel_path = ?1", [path])
                .map_err(|e| e.to_string())?;
        }
        prune_orphans(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(missing.len())
    }

    pub fn pending_vector_chunks(&self, limit: usize) -> Result<Vec<PendingChunk>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.chunk_text
                 FROM chunks c
                 WHERE EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1)
                   AND NOT EXISTS (SELECT 1 FROM chunks_vec v WHERE v.chunk_id = c.id)
                 ORDER BY c.id
                 LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(PendingChunk {
                    chunk_id: row.get(0)?,
                    chunk_text: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn insert_chunk_vectors(&mut self, pairs: &[(i64, Vec<f32>)]) -> Result<(), String> {
        if pairs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        let mut stmt = tx
            .prepare("INSERT OR REPLACE INTO chunks_vec(chunk_id, embedding) VALUES (?1, ?2)")
            .map_err(|e| e.to_string())?;
        for (chunk_id, vec) in pairs {
            if vec.len() != self.embed_dim {
                return Err(format!(
                    "向量维度不匹配：期望 {}，实际 {}",
                    self.embed_dim,
                    vec.len()
                ));
            }
            let bytes = f32_slice_to_le_bytes(vec);
            stmt.execute(params![chunk_id, bytes])
                .map_err(|e| e.to_string())?;
        }
        drop(stmt);
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn stats(&self) -> Result<StoreStats, String> {
        let indexed_files: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE active = 1 AND status != 'derived'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())? as usize;
        let total_chunks: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM chunks c
                 WHERE EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())? as usize;
        let vector_chunks: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks_vec", [], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())? as usize;
        let pending_vectors: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM chunks c
                 WHERE EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1)
                   AND NOT EXISTS (SELECT 1 FROM chunks_vec v WHERE v.chunk_id = c.id)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())? as usize;

        let mut stmt = self
            .conn
            .prepare("SELECT rel_path FROM documents WHERE active = 1 AND status = 'needs_ocr' ORDER BY rel_path")
            .map_err(|e| e.to_string())?;
        let needs_ocr_files = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(StoreStats {
            indexed_files,
            total_chunks,
            vector_chunks,
            pending_vectors,
            needs_ocr_files,
        })
    }

    pub fn search_fts(
        &self,
        query: &str,
        allowed_paths: Option<&HashSet<String>>,
        under_dir: Option<&str>,
        extensions: &[String],
        limit: usize,
    ) -> Result<Vec<ScoredChunk>, String> {
        let terms = split_query_terms(query);
        let long_terms: Vec<&str> = terms
            .iter()
            .map(String::as_str)
            .filter(|t| t.chars().count() >= 3)
            .collect();
        let short_terms: Vec<&str> = terms
            .iter()
            .map(String::as_str)
            .filter(|t| (1..3).contains(&t.chars().count()))
            .collect();

        let mut merged: HashMap<i64, ScoredChunk> = HashMap::new();
        let fetch_limit = if allowed_paths.is_some() || under_dir.is_some() || !extensions.is_empty() {
            (limit * 4).clamp(30, 200)
        } else {
            limit
        };

        if !long_terms.is_empty() {
            let match_expr = long_terms
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT c.id, d.docid, d.rel_path, d.kind, ct.parse_tier, c.heading_path,
                            c.line_start, c.line_end, c.page_num, c.chunk_text,
                            -bm25(chunks_fts) AS score
                     FROM chunks_fts
                     JOIN chunks c ON c.id = chunks_fts.rowid
                     JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1
                     JOIN contents ct ON ct.content_hash = c.content_hash
                     WHERE chunks_fts MATCH ?1
                     ORDER BY bm25(chunks_fts) ASC
                     LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![match_expr, fetch_limit as i64], map_scored_row)
                .map_err(|e| e.to_string())?;
            for row in rows {
                let hit = row.map_err(|e| e.to_string())?;
                if path_matches_filters(&hit.rel_path, allowed_paths, under_dir, extensions) {
                    merged.insert(hit.chunk_id, hit);
                }
            }
        }

        // FTS5 trigram skips tokens shorter than 3 chars (e.g. 2-char CJK words); scan active chunks via LIKE.
        if !short_terms.is_empty() {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT c.id, d.docid, d.rel_path, d.kind, ct.parse_tier, c.heading_path,
                            c.line_start, c.line_end, c.page_num, c.chunk_text
                     FROM chunks c
                     JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1
                     JOIN contents ct ON ct.content_hash = c.content_hash
                     WHERE c.chunk_text LIKE ?1 ESCAPE '\\'
                     LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            for term in short_terms {
                let pattern = format!("%{}%", escape_like(term));
                let rows = stmt
                    .query_map(params![pattern, fetch_limit as i64], |row| {
                        Ok(ScoredChunk {
                            chunk_id: row.get(0)?,
                            docid: row.get(1)?,
                            rel_path: row.get(2)?,
                            kind: row.get(3)?,
                            parse_tier: row.get(4)?,
                            heading_path: row.get(5)?,
                            line_start: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                            line_end: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                            page_num: row.get::<_, Option<i64>>(8)?.map(|v| v as usize),
                            chunk_text: row.get(9)?,
                            raw_score: 1.0,
                        })
                    })
                    .map_err(|e| e.to_string())?;
                for row in rows {
                    let hit = row.map_err(|e| e.to_string())?;
                    if path_matches_filters(&hit.rel_path, allowed_paths, under_dir, extensions) {
                        merged
                            .entry(hit.chunk_id)
                            .and_modify(|existing| existing.raw_score += 1.0)
                            .or_insert(hit);
                    }
                }
            }
        }

        let mut results: Vec<ScoredChunk> = merged.into_values().collect();
        results.sort_by(|a, b| b.raw_score.total_cmp(&a.raw_score));
        results.truncate(limit);
        Ok(results)
    }

    pub fn search_vec(
        &self,
        embedding: &[f32],
        allowed_paths: Option<&HashSet<String>>,
        under_dir: Option<&str>,
        extensions: &[String],
        limit: usize,
    ) -> Result<Vec<ScoredChunk>, String> {
        if embedding.len() != self.embed_dim {
            return Err(format!(
                "查询向量维度不匹配：期望 {}，实际 {}",
                self.embed_dim,
                embedding.len()
            ));
        }
        let k = if allowed_paths.is_some() || under_dir.is_some() || !extensions.is_empty() {
            (limit * 4).clamp(30, 200)
        } else {
            limit
        };
        let bytes = f32_slice_to_le_bytes(embedding);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT v.chunk_id, d.docid, d.rel_path, d.kind, ct.parse_tier, c.heading_path,
                        c.line_start, c.line_end, c.page_num, c.chunk_text,
                        (1.0 - v.distance) AS score
                 FROM chunks_vec v
                 JOIN chunks c ON c.id = v.chunk_id
                 JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1
                 JOIN contents ct ON ct.content_hash = c.content_hash
                 WHERE v.embedding MATCH ?1 AND k = ?2
                 ORDER BY v.distance ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![bytes, k as i64], map_scored_row)
            .map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        for row in rows {
            let hit = row.map_err(|e| e.to_string())?;
            if path_matches_filters(&hit.rel_path, allowed_paths, under_dir, extensions) {
                results.push(hit);
                if results.len() >= limit {
                    break;
                }
            }
        }
        Ok(results)
    }

    pub fn get_rerank_scores(&self, keys: &[String]) -> Result<HashMap<String, f64>, String> {
        let mut out = HashMap::new();
        if keys.is_empty() {
            return Ok(out);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT score FROM rerank_cache WHERE cache_key = ?1")
            .map_err(|e| e.to_string())?;
        for key in keys {
            if let Some(score) = stmt
                .query_row([key], |row| row.get::<_, f64>(0))
                .optional()
                .map_err(|e| e.to_string())?
            {
                out.insert(key.clone(), score);
            }
        }
        Ok(out)
    }

    pub fn put_rerank_scores(&mut self, entries: &[(String, f64)]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let now = unix_secs();
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        let mut stmt = tx
            .prepare(
                "INSERT INTO rerank_cache(cache_key, score, ts) VALUES (?1, ?2, ?3)
                 ON CONFLICT(cache_key) DO UPDATE SET score = excluded.score, ts = excluded.ts",
            )
            .map_err(|e| e.to_string())?;
        for (key, score) in entries {
            stmt.execute(params![key, score, now])
                .map_err(|e| e.to_string())?;
        }
        drop(stmt);
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn pdf_upgrade_candidates(
        &self,
        mode: super::AccuratePdfMode,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        let sql = match mode {
            super::AccuratePdfMode::OnlyOcr => {
                "SELECT d.rel_path, d.content_hash
                 FROM documents d
                 JOIN contents c ON c.content_hash = d.content_hash
                 WHERE d.active = 1 AND d.kind = 'pdf' AND d.status = 'needs_ocr'
                   AND c.parse_tier NOT IN ('accurate', 'mineru')
                 ORDER BY d.rel_path"
            }
            super::AccuratePdfMode::All => {
                "SELECT d.rel_path, d.content_hash
                 FROM documents d
                 JOIN contents c ON c.content_hash = d.content_hash
                 WHERE d.active = 1 AND d.kind = 'pdf' AND d.status != 'derived'
                   AND c.parse_tier NOT IN ('accurate', 'mineru')
                 ORDER BY d.rel_path"
            }
        };
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut out = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        if !paths.is_empty() {
            let wanted: HashSet<String> = paths
                .iter()
                .map(|p| p.trim().replace('\\', "/"))
                .filter(|p| !p.is_empty())
                .collect();
            out.retain(|(rel_path, _)| wanted.contains(rel_path));
        }
        Ok(out)
    }

    pub fn ocr_counts(&self) -> Result<(usize, usize), String> {
        let upgraded: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM documents d
                 JOIN contents c ON c.content_hash = d.content_hash
                 WHERE d.active = 1 AND d.kind = 'pdf' AND c.parse_tier IN ('accurate', 'mineru')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let pending: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM documents d
                 JOIN contents c ON c.content_hash = d.content_hash
                 WHERE d.active = 1 AND d.kind = 'pdf' AND d.status != 'derived'
                   AND c.parse_tier NOT IN ('accurate', 'mineru')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok((upgraded as usize, pending as usize))
    }

    pub fn upsert_ocr_job(
        &mut self,
        content_hash: &str,
        rel_path: &str,
        batch_id: Option<&str>,
        state: &str,
        err_msg: Option<&str>,
    ) -> Result<(), String> {
        let now = unix_secs();
        self.conn
            .execute(
                "INSERT INTO ocr_jobs(content_hash, rel_path, batch_id, state, err_msg, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(content_hash) DO UPDATE SET
                    rel_path = excluded.rel_path,
                    batch_id = COALESCE(excluded.batch_id, ocr_jobs.batch_id),
                    state = excluded.state,
                    err_msg = excluded.err_msg,
                    updated_at = excluded.updated_at",
                params![content_hash, rel_path, batch_id, state, err_msg, now],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn upgrade_pdf_content(
        &mut self,
        content_hash: &str,
        raw_text: &str,
        chunks: &[ChunkDraft],
    ) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM chunks_vec WHERE chunk_id IN (SELECT id FROM chunks WHERE content_hash = ?1)",
            [content_hash],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM chunks WHERE content_hash = ?1", [content_hash])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE contents SET raw_text = ?2, parse_tier = 'accurate', byte_len = ?3 WHERE content_hash = ?1",
            params![content_hash, raw_text, raw_text.len() as i64],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE documents SET status = 'indexed' WHERE content_hash = ?1",
            [content_hash],
        )
        .map_err(|e| e.to_string())?;

        let mut stmt = tx
            .prepare(
                "INSERT INTO chunks(content_hash, seq, heading_path, line_start, line_end, page_num, chunk_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|e| e.to_string())?;
        for chunk in chunks {
            stmt.execute(params![
                content_hash,
                chunk.seq as i64,
                chunk.heading_path,
                chunk.line_start.map(|v| v as i64),
                chunk.line_end.map(|v| v as i64),
                chunk.page_num.map(|v| v as i64),
                chunk.chunk_text,
            ])
            .map_err(|e| e.to_string())?;
        }
        drop(stmt);
        tx.commit().map_err(|e| e.to_string())
    }
}

fn map_scored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScoredChunk> {
    Ok(ScoredChunk {
        chunk_id: row.get(0)?,
        docid: row.get(1)?,
        rel_path: row.get(2)?,
        kind: row.get(3)?,
        parse_tier: row.get(4)?,
        heading_path: row.get(5)?,
        line_start: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
        line_end: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
        page_num: row.get::<_, Option<i64>>(8)?.map(|v| v as usize),
        chunk_text: row.get(9)?,
        raw_score: row.get(10)?,
    })
}

fn allocate_docid(tx: &Transaction<'_>, rel_path: &str, content_hash: &str) -> Result<String, String> {
    if let Some(existing) = tx
        .query_row(
            "SELECT docid FROM documents WHERE rel_path = ?1",
            [rel_path],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        return Ok(existing);
    }

    for len in [8usize, 12, 16, content_hash.len()] {
        let candidate = &content_hash[..len.min(content_hash.len())];
        let taken: Option<i64> = tx
            .query_row("SELECT 1 FROM documents WHERE docid = ?1", [candidate], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if taken.is_none() {
            return Ok(candidate.to_owned());
        }
    }
    Ok(super::sha256_hex(rel_path.as_bytes())[..12].to_owned())
}

fn prune_orphans(tx: &Transaction<'_>) -> Result<(), String> {
    tx.execute(
        "DELETE FROM chunks_vec WHERE chunk_id IN (
            SELECT c.id FROM chunks c
            WHERE NOT EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1)
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM chunks WHERE NOT EXISTS (
            SELECT 1 FROM documents d WHERE d.content_hash = chunks.content_hash AND d.active = 1
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM contents WHERE NOT EXISTS (
            SELECT 1 FROM documents d WHERE d.content_hash = contents.content_hash AND d.active = 1
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn path_matches_filters(
    rel_path: &str,
    allowed_paths: Option<&HashSet<String>>,
    under_dir: Option<&str>,
    extensions: &[String],
) -> bool {
    let base_path = rel_path.strip_suffix("#annotations").unwrap_or(rel_path);
    if let Some(allowed) = allowed_paths
        && !allowed.contains(base_path)
    {
        return false;
    }
    if let Some(dir) = under_dir {
        let dir = dir.trim_matches('/').trim_matches('\\');
        if !dir.is_empty() && dir != "." {
            let prefix = format!("{dir}/");
            if base_path != dir && !base_path.starts_with(&prefix) {
                return false;
            }
        }
    }
    if !extensions.is_empty() {
        let ext = base_path
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !extensions
            .iter()
            .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext))
        {
            return false;
        }
    }
    true
}

fn split_query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for part in query.split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '。' | '；' | ';' | '(' | ')' | '（' | '）')) {
        let trimmed = part.trim();
        if !trimmed.is_empty() && !terms.iter().any(|t| t == trimmed) {
            terms.push(trimmed.to_owned());
        }
    }
    if terms.is_empty() && !query.trim().is_empty() {
        terms.push(query.trim().to_owned());
    }
    terms
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn f32_slice_to_le_bytes(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sqlite_vec_and_trigram_short_cjk_search_work() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 4).unwrap();

        let chunks = vec![ChunkDraft {
            seq: 0,
            heading_path: "notes/opt.md > KKT".into(),
            line_start: Some(1),
            line_end: Some(5),
            page_num: None,
            chunk_text: "[notes/opt.md > KKT]\n梯度下降法与互补松弛条件推导。".into(),
        }];
        store
            .upsert_document(
                "notes/opt.md",
                "md",
                "hash111122223333",
                100,
                64,
                "indexed",
                Some("梯度下降法与互补松弛条件推导。"),
                Some("source"),
                Some(&chunks),
            )
            .unwrap();
        let pending = store.pending_vector_chunks(10).unwrap();
        assert_eq!(pending.len(), 1);
        store
            .insert_chunk_vectors(&[(pending[0].chunk_id, vec![1.0, 0.0, 0.0, 0.0])])
            .unwrap();

        // 4-char CJK hits FTS5 trigram
        let hits_long = store.search_fts("互补松弛", None, None, &[], 5).unwrap();
        assert_eq!(hits_long.len(), 1);

        // 2-char CJK hits LIKE fallback
        let hits_short = store.search_fts("梯度", None, None, &[], 5).unwrap();
        assert_eq!(hits_short.len(), 1);

        // Vector search hits sqlite-vec
        let vec_hits = store
            .search_vec(&[0.9, 0.1, 0.0, 0.0], None, None, &[], 5)
            .unwrap();
        assert_eq!(vec_hits.len(), 1);
        assert_eq!(vec_hits[0].rel_path, "notes/opt.md");
    }

    #[test]
    fn pdf_upgrade_candidates_filters_mode_and_paths() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 4).unwrap();
        store
            .upsert_document("papers/scan.pdf", "pdf", "hash_scan", 1, 10, "needs_ocr", Some(""), Some("fast_pdf"), Some(&[]))
            .unwrap();
        store
            .upsert_document("papers/text.pdf", "pdf", "hash_text", 1, 10, "indexed", Some("text"), Some("fast_pdf"), Some(&[]))
            .unwrap();
        store
            .upsert_document("papers/done.pdf", "pdf", "hash_done", 1, 10, "indexed", Some("done"), Some("mineru"), Some(&[]))
            .unwrap();

        let only_ocr = store.pdf_upgrade_candidates(crate::rag::AccuratePdfMode::OnlyOcr, &[]).unwrap();
        assert_eq!(only_ocr.len(), 1);
        assert_eq!(only_ocr[0].0, "papers/scan.pdf");

        let all = store.pdf_upgrade_candidates(crate::rag::AccuratePdfMode::All, &[]).unwrap();
        assert_eq!(all.len(), 2);

        let selected = store
            .pdf_upgrade_candidates(
                crate::rag::AccuratePdfMode::All,
                &["papers\\text.pdf".to_owned()],
            )
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, "papers/text.pdf");
    }
}
