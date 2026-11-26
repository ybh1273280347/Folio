//! SQLite storage layer using WAL, FTS5 trigram, and statically linked sqlite-vec.

use super::chunker::ChunkDraft;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Once,
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA_VERSION: &str = "1";
const CHUNKER_VERSION: &str = "2";
const FTS_TOKENIZER: &str = "trigram";
const KB_SEARCH_REVISION: &str = "kb_search_revision";

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
    pub content_hash: String,
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
    pub content_hash: String,
    pub heading_path: String,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
    pub page_num: Option<usize>,
    pub chunk_text: String,
    pub raw_score: f64,
}

#[derive(Clone, Debug, Default)]
pub struct FtsKbBuckets {
    pub all: Vec<ScoredChunk>,
    pub notes: Vec<ScoredChunk>,
    pub pdf: Vec<ScoredChunk>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfLibraryItem {
    pub rel_path: String,
    pub content_hash: String,
    pub status: String,
    pub parse_tier: String,
    pub last_error: Option<String>,
    pub total_pages: Option<usize>,
    pub missing_pages: usize,
    pub is_complete: bool,
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
    project_root: std::path::PathBuf,
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

        let project_root = db_path
            .parent()
            .and_then(Path::parent)
            .expect("RAG database path has a project root")
            .to_path_buf();
        let mut store = Self { conn, embed_dim, project_root };
        store.ensure_schema(embed_model, embed_dim, false)?;
        Ok(store)
    }

    pub fn open_readonly_for_search(
        db_path: &Path,
        embed_model: &str,
        embed_dim: usize,
    ) -> Result<Self, String> {
        register_sqlite_vec();
        let conn = Connection::open(db_path).map_err(|e| format!("打开 RAG 数据库失败: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA query_only = ON;")
            .map_err(|e| format!("配置只读检索 PRAGMA 失败: {e}"))?;

        let dim_str = embed_dim.to_string();
        let schema_ok = Self::get_meta(&conn, "schema_version").ok().flatten().as_deref()
            == Some(SCHEMA_VERSION)
            && Self::get_meta(&conn, "chunker_version").ok().flatten().as_deref()
                == Some(CHUNKER_VERSION)
            && Self::get_meta(&conn, "embed_dim").ok().flatten().as_deref()
                == Some(dim_str.as_str())
            && Self::get_meta(&conn, "embed_model").ok().flatten().as_deref() == Some(embed_model);
        if schema_ok {
            let project_root = db_path
                .parent()
                .and_then(Path::parent)
                .expect("RAG database path has a project root")
                .to_path_buf();
            return Ok(Self { conn, embed_dim, project_root });
        }
        drop(conn);
        Self::open(db_path, embed_model, embed_dim)
    }

    /// Open an already-initialized RAG store without modifying embedding settings
    /// or creating a database for projects that have not enabled the knowledge base.
    pub fn open_existing(db_path: &Path) -> Result<Option<Self>, String> {
        if !db_path.is_file() {
            return Ok(None);
        }
        register_sqlite_vec();
        let conn = Connection::open(db_path).map_err(|e| format!("打开 RAG 数据库失败: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| format!("配置 SQLite PRAGMA 失败: {e}"))?;

        let has_tables = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')
                 AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'documents')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?;
        if !has_tables {
            return Ok(None);
        }
        let schema_ok = Self::get_meta(&conn, "schema_version")?.as_deref() == Some(SCHEMA_VERSION)
            && Self::get_meta(&conn, "chunker_version")?.as_deref() == Some(CHUNKER_VERSION);
        if !schema_ok {
            return Ok(None);
        }
        let embed_dim = Self::get_meta(&conn, "embed_dim")?
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);

        let mut store = Self {
            conn,
            embed_dim,
            project_root: db_path
                .parent()
                .and_then(Path::parent)
                .expect("RAG database path has a project root")
                .to_path_buf(),
        };
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        Self::ensure_documents_trash_column(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(store))
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn with_read_snapshot<T>(
        &self,
        f: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<T, String> {
        struct ReadSnapshotGuard<'a>(&'a Connection);
        impl Drop for ReadSnapshotGuard<'_> {
            fn drop(&mut self) {
                let _ = self.0.execute_batch("ROLLBACK");
            }
        }
        self.conn
            .execute_batch("BEGIN DEFERRED")
            .map_err(|e| e.to_string())?;
        let _guard = ReadSnapshotGuard(&self.conn);
        f(self)
    }

    pub fn reset_index(&mut self, embed_model: &str, embed_dim: usize) -> Result<(), String> {
        self.ensure_schema(embed_model, embed_dim, true)?;
        self.embed_dim = embed_dim;
        Ok(())
    }

    fn ensure_schema(
        &mut self,
        embed_model: &str,
        embed_dim: usize,
        force_reset: bool,
    ) -> Result<(), String> {
        // The background sync and search can open the same cache concurrently during a migration.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )
            .map_err(|e| e.to_string())?;

        let prev_schema = Self::get_meta(&tx, "schema_version")?;
        let prev_dim = Self::get_meta(&tx, "embed_dim")?;
        let prev_model = Self::get_meta(&tx, "embed_model")?;
        let prev_chunker = Self::get_meta(&tx, "chunker_version")?;

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
            tx
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
            tx
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
                active       INTEGER NOT NULL DEFAULT 1,
                trash_id     TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_documents_content ON documents(content_hash, active);
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
        tx
            .execute_batch(&sql)
            .map_err(|e| format!("初始化 RAG 表结构失败: {e}"))?;

        Self::ensure_documents_trash_column(&tx)?;
        Self::set_meta(&tx, "schema_version", SCHEMA_VERSION)?;
        Self::set_meta(&tx, "chunker_version", CHUNKER_VERSION)?;
        Self::set_meta(&tx, "fts_tokenizer", FTS_TOKENIZER)?;
        Self::set_meta(&tx, "embed_model", embed_model)?;
        Self::set_meta(&tx, "embed_dim", &embed_dim.to_string())?;
        if rebuild_all {
            Self::bump_kb_search_revision(&tx)?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    fn ensure_documents_trash_column(conn: &Connection) -> Result<(), String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(documents)")
            .map_err(|e| e.to_string())?;
        let has_trash_id = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .any(|name| name == "trash_id");
        if !has_trash_id {
            conn.execute("ALTER TABLE documents ADD COLUMN trash_id TEXT", [])
                .map_err(|e| e.to_string())?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_documents_trash ON documents(trash_id) WHERE trash_id IS NOT NULL",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read inside the same snapshot as the lexical query. Older indexes have
    /// no revision until the first lexical write.
    pub fn kb_search_revision(&self) -> Result<Option<String>, String> {
        Self::get_meta(&self.conn, KB_SEARCH_REVISION)
    }

    fn bump_kb_search_revision(tx: &Transaction<'_>) -> Result<(), String> {
        Self::set_meta(tx, KB_SEARCH_REVISION, &uuid::Uuid::new_v4().to_string())
    }

    fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>, String> {
        conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())
    }

    fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
        conn
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
                "SELECT rel_path, content_hash, mtime_ns, file_size
                 FROM documents
                 WHERE active = 1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DocumentMeta {
                    rel_path: row.get(0)?,
                    content_hash: row.get(1)?,
                    mtime_ns: row.get(2)?,
                    file_size: row.get(3)?,
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

    pub fn indexed_content_status(&self, content_hash: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                // Older copies can have been marked indexed even when the original needed OCR.
                "SELECT status FROM documents WHERE content_hash = ?1 AND active = 1
                 ORDER BY (status = 'needs_ocr') DESC LIMIT 1",
                [content_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
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
        let mut chunks_changed = false;
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
                chunks_changed = !chunk_list.is_empty();
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

        let lexical_changed = chunks_changed
            || !tx
                .query_row(
                    "SELECT EXISTS (
                        SELECT 1 FROM documents
                        WHERE rel_path = ?1 AND kind = ?2 AND content_hash = ?3 AND active = 1
                    )",
                    params![rel_path, kind, content_hash],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?;
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

        if lexical_changed {
            Self::bump_kb_search_revision(&tx)?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn deactivate_missing(&mut self, live_paths: &HashSet<String>) -> Result<usize, String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut stmt = tx
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

        for path in &missing {
            tx.execute(
                "DELETE FROM documents WHERE rel_path = ?1 AND active = 1 AND trash_id IS NULL",
                [path],
            )
            .map_err(|e| e.to_string())?;
        }
        // A save can orphan the previous hash without removing a path. Sweep after every
        // document has been linked so a rename or another copy can still reuse its chunks.
        if prune_orphans(&tx)? || !missing.is_empty() {
            Self::bump_kb_search_revision(&tx)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(missing.len())
    }

    /// Soft-delete a live document and its sidecar annotations row into the trash state,
    /// keeping `contents`, `chunks`, `chunks_fts`, and `chunks_vec` intact for instant restore.
    pub fn trash_document(&mut self, rel_path: &str, trash_id: &str) -> Result<usize, String> {
        let normalized = rel_path.replace('\\', "/");
        let ann_path = format!("{normalized}#annotations");
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut updated = 0usize;
        for source_path in [&normalized, &ann_path] {
            let parked = format!("__folio_trash__/{trash_id}/{source_path}");
            updated += tx
                .execute(
                    "UPDATE documents
                     SET rel_path = ?1, active = 0, trash_id = ?2
                     WHERE rel_path = ?3 AND active = 1 AND trash_id IS NULL",
                    params![parked, trash_id, source_path],
                )
                .map_err(|e| e.to_string())?;
        }
        if updated > 0 {
            Self::bump_kb_search_revision(&tx)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(updated)
    }

    /// Immediately reactivate soft-deleted document rows belonging to `trash_id`.
    /// Handles conflict-renamed restores by remapping `rel_path` without re-chunking or re-embedding.
    pub fn restore_document(
        &mut self,
        trash_id: &str,
        restored_rel_path: &str,
        mtime_ns: i64,
        file_size: i64,
    ) -> Result<usize, String> {
        let restored_rel = restored_rel_path.replace('\\', "/");
        let restored_ann = format!("{restored_rel}#annotations");
        let prefix = format!("__folio_trash__/{trash_id}/");
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;

        let mut stmt = tx
            .prepare("SELECT id, rel_path, kind FROM documents WHERE trash_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([trash_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);

        if rows.is_empty() {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(0);
        }

        let mut restored = 0usize;
        let mut removed_stale = false;
        for (id, parked_path, kind) in rows {
            let is_ann = kind == "annotation" || parked_path.ends_with("#annotations");
            let orig_path = parked_path.strip_prefix(&prefix);
            if is_ann && orig_path != Some(restored_ann.as_str()) {
                // Annotations in `.folio/annotations.json` still reference the original path;
                // do not attach them to a conflict-renamed copy (`foo (1).md`).
                tx.execute("DELETE FROM documents WHERE id = ?1", [id])
                    .map_err(|e| e.to_string())?;
                removed_stale = true;
                continue;
            }
            let target_rel = if is_ann {
                restored_ann.as_str()
            } else {
                restored_rel.as_str()
            };
            let cleared = tx
                .execute(
                    "DELETE FROM documents WHERE rel_path = ?1 AND id != ?2 AND trash_id IS NULL",
                    params![target_rel, id],
                )
                .map_err(|e| e.to_string())?;
            if cleared > 0 {
                removed_stale = true;
            }
            if is_ann {
                tx.execute(
                    "UPDATE documents
                     SET rel_path = ?1, active = 1, trash_id = NULL
                     WHERE id = ?2",
                    params![target_rel, id],
                )
                .map_err(|e| e.to_string())?;
            } else {
                tx.execute(
                    "UPDATE documents
                     SET rel_path = ?1, mtime_ns = ?2, file_size = ?3, active = 1, trash_id = NULL
                     WHERE id = ?4",
                    params![target_rel, mtime_ns, file_size, id],
                )
                .map_err(|e| e.to_string())?;
            }
            restored += 1;
        }

        let pruned = if removed_stale { prune_orphans(&tx)? } else { false };
        if restored > 0 || pruned {
            Self::bump_kb_search_revision(&tx)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(restored)
    }

    /// Permanently delete soft-deleted rows for a purged trash entry and prune orphaned content/vectors.
    pub fn purge_trashed_document(&mut self, trash_id: &str) -> Result<usize, String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let deleted = tx
            .execute("DELETE FROM documents WHERE trash_id = ?1", [trash_id])
            .map_err(|e| e.to_string())?;
        let pruned = prune_orphans(&tx)?;
        if deleted > 0 || pruned {
            Self::bump_kb_search_revision(&tx)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(deleted)
    }

    pub fn pending_vector_chunks(&self, after_chunk_id: i64, limit: usize) -> Result<Vec<PendingChunk>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.chunk_text
                 FROM chunks c
                 WHERE c.id > ?1
                   AND EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1)
                   AND NOT EXISTS (SELECT 1 FROM chunks_vec v WHERE v.chunk_id = c.id)
                 ORDER BY c.id
                 LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![after_chunk_id, limit as i64], |row| {
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

    pub fn has_indexed_documents(&self) -> Result<bool, String> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM documents WHERE active = 1 AND status != 'derived')",
            [],
            |row| row.get(0),
        ).map_err(|e| e.to_string())
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
        // Scan the vector IDs once. A LEFT JOIN from chunks makes vec0 load each
        // embedding through its point lookup, even for a count.
        let (total_chunks, vector_chunks): (i64, i64) = self
            .conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM chunks c
                     WHERE EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1)),
                    (SELECT COUNT(*) FROM chunks_vec v
                     CROSS JOIN chunks c ON c.id = v.chunk_id
                     WHERE EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash AND d.active = 1))",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;

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
            total_chunks: total_chunks as usize,
            vector_chunks: vector_chunks as usize,
            pending_vectors: (total_chunks - vector_chunks) as usize,
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
        let (long_terms, short_terms) = partition_query_terms(&terms);
        let matching_paths = self.matching_paths_json(allowed_paths, under_dir, extensions)?;
        let (merged, _) = self.collect_fts_matches(
            &long_terms,
            &short_terms,
            limit,
            "",
            matching_paths.as_deref(),
        )?;
        Ok(finalize_fts_hits(merged.into_values().collect(), !short_terms.is_empty(), limit))
    }

    pub fn search_fts_kb_buckets(&self, query: &str, limit: usize) -> Result<FtsKbBuckets, String> {
        let terms = split_query_terms(query);
        let (long_terms, short_terms) = partition_query_terms(&terms);
        let long_limit = (limit * 20).clamp(200, 2000);
        let (merged, long_raw_count) =
            self.collect_fts_matches(&long_terms, &short_terms, long_limit, "", None)?;

        let mut all_raw: Vec<ScoredChunk> = Vec::with_capacity(merged.len());
        let mut notes_map: HashMap<i64, ScoredChunk> = HashMap::new();
        let mut pdf_map: HashMap<i64, ScoredChunk> = HashMap::new();
        for hit in merged.into_values() {
            all_raw.push(hit.clone());
            if is_pdf_path(&hit.rel_path) {
                pdf_map.insert(hit.chunk_id, hit);
            } else {
                notes_map.insert(hit.chunk_id, hit);
            }
        }

        if !long_terms.is_empty() && long_raw_count >= long_limit {
            if notes_map.len() < limit {
                let (extra_notes, _) = self.collect_fts_matches(
                    &long_terms,
                    &[],
                    long_limit,
                    " AND lower(replace(d.rel_path, '#annotations', '')) NOT LIKE '%.pdf'",
                    None,
                )?;
                for (id, hit) in extra_notes {
                    notes_map.entry(id).or_insert(hit);
                }
            }
            if pdf_map.len() < limit {
                let (extra_pdf, _) = self.collect_fts_matches(
                    &long_terms,
                    &[],
                    long_limit,
                    " AND lower(replace(d.rel_path, '#annotations', '')) LIKE '%.pdf'",
                    None,
                )?;
                for (id, hit) in extra_pdf {
                    pdf_map.entry(id).or_insert(hit);
                }
            }
        }

        let has_short = !short_terms.is_empty();
        Ok(FtsKbBuckets {
            all: finalize_fts_hits(all_raw, has_short, limit),
            notes: finalize_fts_hits(notes_map.into_values().collect(), has_short, limit),
            pdf: finalize_fts_hits(pdf_map.into_values().collect(), has_short, limit),
        })
    }

    /// Resolve path rules once per retrieval, then bind matching paths as one SQLite value.
    fn matching_paths_json(
        &self,
        allowed_paths: Option<&HashSet<String>>,
        under_dir: Option<&str>,
        extensions: &[String],
    ) -> Result<Option<String>, String> {
        if allowed_paths.is_none() && under_dir.is_none() && extensions.is_empty() {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare("SELECT rel_path FROM documents WHERE active = 1")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut paths = Vec::new();
        for row in rows {
            let path = row.map_err(|e| e.to_string())?;
            if path_matches_filters(&path, allowed_paths, under_dir, extensions) {
                paths.push(path);
            }
        }
        Ok(Some(serde_json::to_string(&paths).map_err(|e| e.to_string())?))
    }

    fn collect_fts_matches(
        &self,
        long_terms: &[&str],
        short_terms: &[&str],
        long_limit: usize,
        extra_long_sql: &str,
        matching_paths: Option<&str>,
    ) -> Result<(HashMap<i64, ScoredChunk>, usize), String> {
        let mut merged: HashMap<i64, ScoredChunk> = HashMap::new();
        let mut long_raw_count = 0usize;

        if !long_terms.is_empty() {
            let match_expr = long_terms
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR ");
            let sql = format!(
                "SELECT c.id, d.docid, d.rel_path, d.kind, ct.parse_tier, c.heading_path,
                        c.line_start, c.line_end, c.page_num, c.chunk_text,
                        -bm25(chunks_fts) AS score, d.content_hash
                 FROM chunks_fts
                 JOIN chunks c ON c.id = chunks_fts.rowid
                 JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1
                 JOIN contents ct ON ct.content_hash = c.content_hash
                 WHERE chunks_fts MATCH ?1{extra_long_sql}
                   AND (?3 IS NULL OR d.rel_path IN (SELECT value FROM json_each(?3)))
                 ORDER BY bm25(chunks_fts) ASC, c.id ASC, d.id ASC
                 LIMIT ?2"
            );
            let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![match_expr, long_limit as i64, matching_paths], map_scored_row)
                .map_err(|e| e.to_string())?;
            for row in rows {
                let hit = row.map_err(|e| e.to_string())?;
                long_raw_count += 1;
                merged.insert(hit.chunk_id, hit);
            }
        }

        // FTS5 trigram skips tokens shorter than 3 chars (e.g. 1-2 char CJK words);
        // scan active chunks without premature LIMIT so large earlier PDFs cannot starve notes.
        if !short_terms.is_empty() {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT c.id, d.docid, d.rel_path, d.kind, ct.parse_tier, c.heading_path,
                            c.line_start, c.line_end, c.page_num, c.chunk_text, d.content_hash
                     FROM chunks c
                     JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1
                     JOIN contents ct ON ct.content_hash = c.content_hash
                     WHERE (
                         c.chunk_text LIKE ?1 ESCAPE '\\'
                         OR d.rel_path LIKE ?1 ESCAPE '\\'
                         OR c.heading_path LIKE ?1 ESCAPE '\\'
                     ) AND (?2 IS NULL OR d.rel_path IN (SELECT value FROM json_each(?2)))",
                )
                .map_err(|e| e.to_string())?;
            for term in short_terms {
                let pattern = format!("%{}%", escape_like(term));
                let rows = stmt
                    .query_map(params![pattern, matching_paths], |row| {
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
                            raw_score: 0.0,
                            content_hash: row.get(10)?,
                        })
                    })
                    .map_err(|e| e.to_string())?;
                for row in rows {
                    let mut hit = row.map_err(|e| e.to_string())?;
                    let term_score = score_short_term_hit(
                        term,
                        &hit.rel_path,
                        &hit.heading_path,
                        &hit.chunk_text,
                    );
                    hit.raw_score = term_score;
                    merged
                        .entry(hit.chunk_id)
                        .and_modify(|existing| existing.raw_score += term_score)
                        .or_insert(hit);
                }
            }
        }

        Ok((merged, long_raw_count))
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
        let matching_paths = self.matching_paths_json(allowed_paths, under_dir, extensions)?;
        // sqlite-vec applies this row-ID constraint inside KNN, before picking the nearest k.
        let knn_filter = if matching_paths.is_some() {
            "AND chunk_id IN (
                SELECT c.id FROM chunks c JOIN documents d ON d.content_hash = c.content_hash
                WHERE d.active = 1 AND d.rel_path IN (SELECT value FROM json_each(?3))
            )"
        } else {
            ""
        };
        let bytes = f32_slice_to_le_bytes(embedding);
        let sql = format!(
            "WITH nearest AS MATERIALIZED (
                 SELECT chunk_id, distance FROM chunks_vec
                 WHERE embedding MATCH ?1 AND k = ?2 {knn_filter}
             )
             SELECT v.chunk_id, d.docid, d.rel_path, d.kind, ct.parse_tier, c.heading_path,
                        c.line_start, c.line_end, c.page_num, c.chunk_text,
                        (1.0 - v.distance) AS score, d.content_hash
                 FROM nearest v
                 JOIN chunks c ON c.id = v.chunk_id
                 JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1
                 JOIN contents ct ON ct.content_hash = c.content_hash
                 WHERE (?3 IS NULL OR d.rel_path IN (SELECT value FROM json_each(?3)))
                 ORDER BY v.distance ASC, v.chunk_id ASC, d.id ASC
                 LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![bytes, limit as i64, matching_paths], map_scored_row)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn candidate_vectors(&self, chunk_ids: &[i64]) -> Result<HashMap<i64, Vec<f32>>, String> {
        if chunk_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = serde_json::to_string(chunk_ids).map_err(|err| err.to_string())?;
        let mut stmt = self.conn.prepare(
            "SELECT v.chunk_id, v.embedding FROM chunks_vec v\n\
             JOIN chunks c ON c.id = v.chunk_id\n\
             JOIN documents d ON d.content_hash = c.content_hash AND d.active = 1\n\
             WHERE v.chunk_id IN (SELECT value FROM json_each(?1))",
        ).map_err(|err| err.to_string())?;
        let rows = stmt.query_map([ids], |row| {
            let bytes: Vec<u8> = row.get(1)?;
            let embedding = bytes
                .chunks_exact(4)
                .map(|value| f32::from_le_bytes(value.try_into().expect("4-byte embedding value")))
                .collect();
            Ok((row.get(0)?, embedding))
        }).map_err(|err| err.to_string())?;
        rows.collect::<Result<HashMap<_, _>, _>>().map_err(|err| err.to_string())
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
        parse_tier: &str,
        status: &str,
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
            "UPDATE contents SET raw_text = ?2, parse_tier = ?3, byte_len = ?4 WHERE content_hash = ?1",
            params![content_hash, raw_text, parse_tier, raw_text.len() as i64],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE documents SET status = ?2 WHERE content_hash = ?1",
            params![content_hash, status],
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
        Self::bump_kb_search_revision(&tx)?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn pdf_library_items(&self) -> Result<Vec<PdfLibraryItem>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT d.rel_path, d.content_hash, d.status, c.parse_tier,
                        CASE WHEN j.state = 'failed' THEN j.err_msg ELSE NULL END
                 FROM documents d
                 JOIN contents c ON c.content_hash = d.content_hash
                 LEFT JOIN ocr_jobs j ON j.content_hash = d.content_hash
                 WHERE d.active = 1 AND d.kind = 'pdf' AND d.status != 'derived'
                 ORDER BY (d.status = 'needs_ocr') DESC, d.rel_path ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let status: String = row.get(2)?;
                let parse_tier: String = row.get(3)?;
                let is_complete = status != "needs_ocr";
                Ok(PdfLibraryItem {
                    rel_path: row.get(0)?,
                    content_hash: row.get(1)?,
                    status,
                    parse_tier,
                    last_error: row.get(4)?,
                    total_pages: None,
                    missing_pages: usize::from(!is_complete),
                    is_complete,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn cached_accurate_pdfs(&self) -> Result<Vec<(String, String, String)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT d.rel_path, d.content_hash, c.parse_tier
                 FROM documents d
                 JOIN contents c ON c.content_hash = d.content_hash
                 WHERE d.active = 1 AND d.kind = 'pdf' AND d.status = 'indexed'
                 ORDER BY d.rel_path",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
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
        content_hash: row.get(11)?,
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
    Ok(crate::files::sha256_hex(rel_path.as_bytes())[..12].to_owned())
}

fn prune_orphans(tx: &Transaction<'_>) -> Result<bool, String> {
    let mut stmt = tx.prepare(
        "SELECT content_hash FROM contents c
         WHERE NOT EXISTS (SELECT 1 FROM documents d WHERE d.content_hash = c.content_hash)",
    ).map_err(|e| e.to_string())?;
    let orphan_hashes = stmt.query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if orphan_hashes.is_empty() {
        return Ok(false);
    }
    let mut delete_vectors = tx.prepare_cached(
        "DELETE FROM chunks_vec WHERE chunk_id IN (SELECT id FROM chunks WHERE content_hash = ?1)",
    ).map_err(|e| e.to_string())?;
    let mut delete_chunks = tx.prepare_cached("DELETE FROM chunks WHERE content_hash = ?1")
        .map_err(|e| e.to_string())?;
    let mut delete_content = tx.prepare_cached("DELETE FROM contents WHERE content_hash = ?1")
        .map_err(|e| e.to_string())?;
    let mut deleted_chunks = 0;
    for hash in orphan_hashes {
        delete_vectors.execute([&hash]).map_err(|e| e.to_string())?;
        deleted_chunks += delete_chunks.execute([&hash]).map_err(|e| e.to_string())?;
        delete_content.execute([&hash]).map_err(|e| e.to_string())?;
    }
    Ok(deleted_chunks > 0)
}

pub fn is_pdf_path(rel_path: &str) -> bool {
    let base_path = rel_path.strip_suffix("#annotations").unwrap_or(rel_path);
    base_path.to_ascii_lowercase().ends_with(".pdf")
}

pub fn is_near_duplicate(candidate: &str, selected: &[String]) -> bool {
    const N: usize = 3;
    let grams: HashSet<Vec<char>> = candidate
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<Vec<_>>()
        .windows(N)
        .map(<[char]>::to_vec)
        .collect();
    if grams.is_empty() {
        return false;
    }
    selected.iter().any(|text| {
        let other: HashSet<Vec<char>> = text
            .to_lowercase()
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<Vec<_>>()
            .windows(N)
            .map(<[char]>::to_vec)
            .collect();
        let intersection = grams.intersection(&other).count();
        let union = grams.union(&other).count();
        intersection * 100 > union * 65
    })
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

fn partition_query_terms(terms: &[String]) -> (Vec<&str>, Vec<&str>) {
    let long_terms = terms
        .iter()
        .map(String::as_str)
        .filter(|t| t.chars().count() >= 3)
        .collect();
    let short_terms = terms
        .iter()
        .map(String::as_str)
        .filter(|t| (1..3).contains(&t.chars().count()))
        .collect();
    (long_terms, short_terms)
}

fn finalize_fts_hits(
    mut results: Vec<ScoredChunk>,
    has_short_terms: bool,
    limit: usize,
) -> Vec<ScoredChunk> {
    const MAX_SHORT_HITS_PER_FILE: usize = 4;
    results.sort_by(|a, b| {
        b.raw_score
            .total_cmp(&a.raw_score)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    if has_short_terms && results.len() > limit {
        let mut per_file: HashMap<String, usize> = HashMap::new();
        let mut diverse = Vec::with_capacity(limit);
        let mut overflow = Vec::new();
        for hit in results {
            // Keep annotation hits in the candidate pool even when their PDF body fills
            // its quota. The response formatter still controls the final per-file limit.
            let count = per_file.entry(hit.rel_path.clone()).or_insert(0);
            if *count < MAX_SHORT_HITS_PER_FILE {
                *count += 1;
                diverse.push(hit);
                if diverse.len() >= limit {
                    break;
                }
            } else {
                overflow.push(hit);
            }
        }
        if diverse.len() < limit {
            let need = limit - diverse.len();
            diverse.extend(overflow.into_iter().take(need));
        }
        diverse
    } else {
        results.truncate(limit);
        results
    }
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

fn score_short_term_hit(term: &str, rel_path: &str, heading_path: &str, chunk_text: &str) -> f64 {
    let needle = term.to_lowercase();
    let path_match = rel_path.to_lowercase().contains(&needle);
    let heading_match = heading_path.to_lowercase().contains(&needle);
    let body = chunk_text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n").map(|(_, b)| b))
        .unwrap_or(chunk_text);
    let body_hits = body.to_lowercase().matches(&needle).count();

    let mut score = 0.5;
    if path_match {
        score += 4.0;
    }
    if heading_match {
        score += 3.0;
    }
    if body_hits > 0 {
        score += (body_hits.min(12) as f64).sqrt() * 1.25;
    }
    score
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
        let pending = store.pending_vector_chunks(0, 10).unwrap();
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
    fn character_trigram_similarity_collapses_whitespace_and_case() {
        assert!(is_near_duplicate(
            "The quick brown fox jumps over the lazy dog.",
            &["the QUICK brown fox jumps over the lazy dog".into()],
        ));
        assert!(!is_near_duplicate("gradient descent", &["gradient ascend".into()]));
        assert!(!is_near_duplicate("栈", &["栈".into()]));
    }

    #[test]
    fn filtered_retrieval_finds_matches_outside_global_top_k() {
        let dir = tempdir().unwrap();
        let mut store = RagStore::open(&dir.path().join("rag.sqlite3"), "test-embed", 2).unwrap();
        let outside_chunks: Vec<_> = (0..201)
            .map(|seq| ChunkDraft {
                seq,
                heading_path: "outside.md".into(),
                line_start: None,
                line_end: None,
                page_num: None,
                chunk_text: "gradient".into(),
            })
            .collect();
        let target = ChunkDraft {
            seq: 0,
            heading_path: "notes/target.md".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "gradient alpha beta gamma delta epsilon zeta".into(),
        };
        store.upsert_document("outside.md", "md", "hash-outside", 1, 8, "indexed", Some("gradient"), Some("source"), Some(&outside_chunks)).unwrap();
        store.upsert_document("notes/target.md", "md", "hash-inside", 1, 43, "indexed", Some(&target.chunk_text), Some("source"), Some(std::slice::from_ref(&target))).unwrap();
        let vectors: Vec<_> = store.pending_vector_chunks(0, 202).unwrap().into_iter()
            .zip(vec![vec![1.0, 0.0]; 201].into_iter().chain([vec![0.0, 1.0]]))
            .map(|(chunk, vector)| (chunk.chunk_id, vector))
            .collect();
        store.insert_chunk_vectors(&vectors).unwrap();

        let lex = store.search_fts("gradient", None, Some("notes"), &[], 1).unwrap();
        assert_eq!(lex.first().map(|hit| hit.rel_path.as_str()), Some("notes/target.md"));
        let allowed = HashSet::from(["notes/target.md".into()]);
        let semantic = store.search_vec(&[1.0, 0.0], Some(&allowed), None, &[".md".into()], 1).unwrap();
        assert_eq!(semantic.first().map(|hit| hit.rel_path.as_str()), Some("notes/target.md"));
    }

    #[test]
    fn changed_document_releases_old_vectors_without_removing_its_path() {
        let dir = tempdir().unwrap();
        let mut store = RagStore::open(&dir.path().join("rag.sqlite3"), "test-embed", 2).unwrap();
        let mut draft = ChunkDraft {
            seq: 0,
            heading_path: "note.md".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "old content".into(),
        };
        store.upsert_document("note.md", "md", "hash-old", 1, 11, "indexed", Some("old content"), Some("source"), Some(std::slice::from_ref(&draft))).unwrap();
        let old_chunk_id = store.pending_vector_chunks(0, 1).unwrap()[0].chunk_id;
        store.insert_chunk_vectors(&[(old_chunk_id, vec![1.0, 0.0])]).unwrap();

        draft.chunk_text = "replacement content".into();
        store.upsert_document("note.md", "md", "hash-new", 2, 19, "indexed", Some("replacement content"), Some("source"), Some(&[draft])).unwrap();
        assert_eq!(store.deactivate_missing(&HashSet::from(["note.md".into()])).unwrap(), 0);
        let new_chunk_id = store.pending_vector_chunks(0, 1).unwrap()[0].chunk_id;
        store.insert_chunk_vectors(&[(new_chunk_id, vec![0.0, 1.0])]).unwrap();

        let nearest = store.search_vec(&[1.0, 0.0], None, None, &[], 1).unwrap();
        assert_eq!(nearest.first().map(|hit| hit.chunk_text.as_str()), Some("replacement content"));
    }

    #[test]
    fn opening_character_index_discards_stale_chunks_and_vectors() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 4).unwrap();
        let text = "旧版字符分块";
        let draft = ChunkDraft {
            seq: 0,
            heading_path: "notes.txt".into(),
            line_start: Some(1),
            line_end: Some(1),
            page_num: None,
            chunk_text: format!("[notes.txt]\n{text}"),
        };
        store
            .upsert_document(
                "notes.txt",
                "txt",
                "character-index-hash",
                1,
                text.len() as i64,
                "indexed",
                Some(text),
                Some("source"),
                Some(&[draft]),
            )
            .unwrap();
        let pending = store.pending_vector_chunks(0, 1).unwrap();
        store
            .insert_chunk_vectors(&[(pending[0].chunk_id, vec![1.0, 0.0, 0.0, 0.0])])
            .unwrap();
        drop(store);
        Connection::open(&db)
            .unwrap()
            .execute("UPDATE meta SET value = '1' WHERE key = 'chunker_version'", [])
            .unwrap();

        let store = RagStore::open(&db, "test-embed", 4).unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(
            (stats.indexed_files, stats.total_chunks, stats.vector_chunks),
            (0, 0, 0)
        );
        assert!(store.indexed_content_status("character-index-hash").unwrap().is_none());
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

    #[test]
    fn short_term_search_does_not_starve_notes_behind_many_pdf_chunks() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 4).unwrap();

        for pdf_idx in 0..2 {
            let chunks: Vec<ChunkDraft> = (0..40)
                .map(|seq| ChunkDraft {
                    seq,
                    heading_path: format!("第{pdf_idx}章 > 例题"),
                    line_start: None,
                    line_end: None,
                    page_num: Some(seq + 1),
                    chunk_text: format!(
                        "[pdf_{pdf_idx}.pdf]\n入栈出栈清空栈操作序列第{seq}题。"
                    ),
                })
                .collect();
            store
                .upsert_document(
                    &format!("pdf_{pdf_idx}.pdf"),
                    "pdf",
                    &format!("hash_pdf_{pdf_idx}"),
                    100,
                    200,
                    "indexed",
                    Some("pdf raw"),
                    Some("fast_pdf"),
                    Some(&chunks),
                )
                .unwrap();
        }

        let titled_note = vec![ChunkDraft {
            seq: 0,
            heading_path: "栈的定义与性质".into(),
            line_start: Some(1),
            line_end: Some(4),
            page_num: None,
            chunk_text: "[notes/第三章栈与队列系统笔记.md > 栈的定义与性质]\n栈是仅在表尾进行插入和删除的线性表。".into(),
        }];
        store
            .upsert_document(
                "notes/第三章栈与队列系统笔记.md",
                "md",
                "hash_note_titled",
                100,
                64,
                "indexed",
                Some("note raw"),
                Some("source"),
                Some(&titled_note),
            )
            .unwrap();

        let body_note = vec![ChunkDraft {
            seq: 0,
            heading_path: "考前复习".into(),
            line_start: Some(1),
            line_end: Some(3),
            page_num: None,
            chunk_text: "[notes/review.md > 考前复习]\n记得复习栈的应用。".into(),
        }];
        store
            .upsert_document(
                "notes/review.md",
                "md",
                "hash_note_body",
                100,
                40,
                "indexed",
                Some("review raw"),
                Some("source"),
                Some(&body_note),
            )
            .unwrap();

        let all_hits = store.search_fts("栈", None, None, &[], 60).unwrap();
        assert_eq!(all_hits[0].rel_path, "notes/第三章栈与队列系统笔记.md");
        assert!(all_hits.iter().any(|h| h.rel_path == "notes/review.md"));

        let md_hits = store
            .search_fts("栈", None, None, &["md".to_owned()], 20)
            .unwrap();
        let md_paths: Vec<&str> = md_hits.iter().map(|h| h.rel_path.as_str()).collect();
        assert_eq!(
            md_paths,
            vec!["notes/第三章栈与队列系统笔记.md", "notes/review.md"]
        );

        let ann = vec![ChunkDraft {
            seq: 0,
            heading_path: "pdf_0.pdf#annotations".into(),
            line_start: None,
            line_end: None,
            page_num: Some(1),
            chunk_text: "[pdf_0.pdf#annotations]\n批注：栈溢出注意点".into(),
        }];
        store
            .upsert_document(
                "pdf_0.pdf#annotations",
                "annotation",
                "hash_pdf_ann",
                100,
                32,
                "indexed",
                Some("ann raw"),
                Some("annotation"),
                Some(&ann),
            )
            .unwrap();
        drop(store);

        let ro = RagStore::open_readonly_for_search(&db, "test-embed", 4).unwrap();
        let buckets = ro
            .with_read_snapshot(|s| s.search_fts_kb_buckets("栈", 20))
            .unwrap();
        assert_eq!(buckets.all[0].rel_path, "notes/第三章栈与队列系统笔记.md");
        assert_eq!(
            buckets
                .notes
                .iter()
                .map(|h| h.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["notes/第三章栈与队列系统笔记.md", "notes/review.md"]
        );
        assert!(!buckets.pdf.is_empty());
        assert!(buckets.pdf.iter().all(|h| is_pdf_path(&h.rel_path)));
        assert!(buckets.pdf.iter().any(|h| h.rel_path == "pdf_0.pdf#annotations"));
    }

    #[test]
    fn trash_soft_delete_hides_immediately_survives_sync_and_restores_without_reembedding() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 2).unwrap();
        let doc_chunk = ChunkDraft {
            seq: 0,
            heading_path: "note.md".into(),
            line_start: Some(1),
            line_end: Some(2),
            page_num: None,
            chunk_text: "[note.md]\n量子力学与波函数演化".into(),
        };
        let ann_chunk = ChunkDraft {
            seq: 0,
            heading_path: "note.md#annotations".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "[note.md#annotations]\n批注：波函数坍缩".into(),
        };
        store
            .upsert_document(
                "note.md",
                "md",
                "hash-note",
                100,
                32,
                "indexed",
                Some("量子力学与波函数演化"),
                Some("source"),
                Some(&[doc_chunk]),
            )
            .unwrap();
        store
            .upsert_document(
                "note.md#annotations",
                "annotation",
                "hash-ann:annotation",
                100,
                20,
                "indexed",
                Some("批注：波函数坍缩"),
                Some("annotation"),
                Some(&[ann_chunk]),
            )
            .unwrap();
        let pending = store.pending_vector_chunks(0, 10).unwrap();
        assert_eq!(pending.len(), 2);
        let original_chunk_ids: Vec<i64> = pending.iter().map(|c| c.chunk_id).collect();
        store
            .insert_chunk_vectors(&[
                (pending[0].chunk_id, vec![1.0, 0.0]),
                (pending[1].chunk_id, vec![0.9, 0.1]),
            ])
            .unwrap();
        let rev_before = store.kb_search_revision().unwrap();

        // Moving to trash hides both document and annotations immediately.
        assert_eq!(store.trash_document("note.md", "trash-1").unwrap(), 2);
        let rev_trashed = store.kb_search_revision().unwrap();
        assert_ne!(rev_before, rev_trashed);

        let stats_trashed = store.stats().unwrap();
        assert_eq!(stats_trashed.indexed_files, 0);
        assert_eq!(stats_trashed.total_chunks, 0);
        assert_eq!(stats_trashed.vector_chunks, 0);
        assert!(store.search_fts("波函数", None, None, &[], 5).unwrap().is_empty());
        assert!(store.search_vec(&[1.0, 0.0], None, None, &[], 5).unwrap().is_empty());

        // Subsequent sync `deactivate_missing` must NOT prune trashed rows or their vectors.
        assert_eq!(store.deactivate_missing(&HashSet::new()).unwrap(), 0);

        // Restoring reactivates both rows with the exact same chunk IDs & vectors intact.
        assert_eq!(store.restore_document("trash-1", "note.md", 200, 32).unwrap(), 2);
        let rev_restored = store.kb_search_revision().unwrap();
        assert_ne!(rev_trashed, rev_restored);

        let stats_restored = store.stats().unwrap();
        assert_eq!(stats_restored.indexed_files, 2);
        assert_eq!(stats_restored.total_chunks, 2);
        assert_eq!(stats_restored.vector_chunks, 2);
        assert_eq!(stats_restored.pending_vectors, 0);
        assert!(store.pending_vector_chunks(0, 10).unwrap().is_empty());

        let fts_hits = store.search_fts("波函数", None, None, &[], 5).unwrap();
        assert_eq!(fts_hits.len(), 2);
        let vec_hits = store.search_vec(&[1.0, 0.0], None, None, &[], 5).unwrap();
        assert_eq!(vec_hits.len(), 2);
        assert!(original_chunk_ids.contains(&vec_hits[0].chunk_id));
    }

    #[test]
    fn trash_collision_with_same_path_and_conflict_rename_restore_and_purge() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        let mut store = RagStore::open(&db, "test-embed", 2).unwrap();

        let chunk1 = ChunkDraft {
            seq: 0,
            heading_path: "note.md".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "first archived draft".into(),
        };
        let ann1 = ChunkDraft {
            seq: 0,
            heading_path: "note.md#annotations".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "unique_ann_token".into(),
        };
        store
            .upsert_document("note.md", "md", "hash-1", 1, 10, "indexed", Some("first archived draft"), Some("source"), Some(&[chunk1]))
            .unwrap();
        store
            .upsert_document("note.md#annotations", "annotation", "hash-ann-1", 1, 10, "indexed", Some("unique_ann_token"), Some("annotation"), Some(&[ann1]))
            .unwrap();
        let p1 = store.pending_vector_chunks(0, 10).unwrap();
        store.insert_chunk_vectors(&[(p1[0].chunk_id, vec![1.0, 0.0]), (p1[1].chunk_id, vec![1.0, 0.0])]).unwrap();

        store.trash_document("note.md", "trash-a").unwrap();

        // Creating a brand-new file at the same path does not overwrite the trashed entry.
        let chunk2 = ChunkDraft {
            seq: 0,
            heading_path: "note.md".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "second live draft".into(),
        };
        store
            .upsert_document("note.md", "md", "hash-2", 2, 10, "indexed", Some("second live draft"), Some("source"), Some(&[chunk2]))
            .unwrap();

        // Conflict rename restore moves the trashed file to `note (1).md` and drops stale annotations.
        assert_eq!(store.restore_document("trash-a", "note (1).md", 3, 10).unwrap(), 1);
        let hits = store.search_fts("archived", None, None, &[], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel_path, "note (1).md");
        assert!(store.search_fts("unique_ann_token", None, None, &[], 5).unwrap().is_empty());

        // Trashing and purging a second entry removes its orphan chunks & vectors completely.
        store.trash_document("note.md", "trash-b").unwrap();
        assert_eq!(store.purge_trashed_document("trash-b").unwrap(), 1);
        let remaining_chunks: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM chunks WHERE content_hash = 'hash-2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining_chunks, 0);
    }

    #[test]
    fn open_existing_migrates_legacy_documents_table_in_place_without_dropping_vectors() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("rag.sqlite3");
        assert!(RagStore::open_existing(&db).unwrap().is_none());

        let mut store = RagStore::open(&db, "test-embed", 2).unwrap();
        let draft = ChunkDraft {
            seq: 0,
            heading_path: "legacy.md".into(),
            line_start: None,
            line_end: None,
            page_num: None,
            chunk_text: "legacy vector content".into(),
        };
        store
            .upsert_document("legacy.md", "md", "hash-legacy", 1, 21, "indexed", Some("legacy vector content"), Some("source"), Some(&[draft]))
            .unwrap();
        let chunk_id = store.pending_vector_chunks(0, 1).unwrap()[0].chunk_id;
        store.insert_chunk_vectors(&[(chunk_id, vec![1.0, 0.0])]).unwrap();
        drop(store);

        // Simulate a pre-migration DB lacking the `trash_id` column.
        let raw_conn = Connection::open(&db).unwrap();
        raw_conn.execute_batch("DROP INDEX IF EXISTS idx_documents_trash; ALTER TABLE documents DROP COLUMN trash_id;").unwrap();
        drop(raw_conn);

        let mut migrated = RagStore::open_existing(&db).unwrap().expect("existing store opens");
        assert_eq!(migrated.stats().unwrap().vector_chunks, 1);
        assert_eq!(migrated.trash_document("legacy.md", "trash-legacy").unwrap(), 1);
        assert_eq!(migrated.restore_document("trash-legacy", "legacy.md", 2, 21).unwrap(), 1);
        assert_eq!(migrated.stats().unwrap().vector_chunks, 1);
    }
}
