//! Derived SQLite index for canvas backlinks and path relocation inside `.folio/metadata.sqlite3`.

use super::{anchor::remap_rel_path, model::CanvasDocument};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasBacklink {
    pub canvas_path: String,
    pub node_id: String,
    pub anchor_kind: String,
    pub label: String,
}

pub fn metadata_db_path(project_root: Option<&Path>) -> io::Result<PathBuf> {
    match project_root {
        Some(root) => Ok(root.join(".folio").join("metadata.sqlite3")),
        None => Ok(crate::storage::data_directory()?.join("loose-metadata.sqlite3")),
    }
}

fn open_links_db(db_path: &Path, create: bool) -> Result<Option<Connection>, String> {
    if !db_path.exists() && !create {
        return Ok(None);
    }
    if create && let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(db_path, flags).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(250))
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS canvas_links (
            canvas_path TEXT NOT NULL,
            node_id     TEXT NOT NULL,
            target_path TEXT NOT NULL,
            anchor_kind TEXT NOT NULL,
            label       TEXT NOT NULL,
            PRIMARY KEY (canvas_path, node_id)
        );
        CREATE INDEX IF NOT EXISTS idx_canvas_links_target ON canvas_links(target_path);",
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(conn))
}

pub fn sync_canvas_links(
    db_path: &Path,
    canvas_path: &str,
    doc: &CanvasDocument,
) -> Result<(), String> {
    let normalized_canvas = canvas_path.replace('\\', "/");
    let mut links = Vec::new();
    for node in &doc.nodes {
        let target = node
            .anchor()
            .map(|a| (a.target_path().replace('\\', "/"), a.kind_label()))
            .or_else(|| node.file.as_deref().map(|f| (f.replace('\\', "/"), "file")));
        if let Some((target_path, anchor_kind)) = target
            && !target_path.is_empty()
        {
            let preview = node
                .display_text()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(48)
                .collect::<String>();
            links.push((node.id.as_str(), target_path, anchor_kind, preview));
        }
    }

    if links.is_empty() && !db_path.exists() {
        return Ok(());
    }
    let Some(mut conn) = open_links_db(db_path, !links.is_empty())? else {
        return Ok(());
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM canvas_links WHERE canvas_path = ?1",
        [&normalized_canvas],
    )
    .map_err(|e| e.to_string())?;
    if !links.is_empty() {
        let mut stmt = tx
            .prepare(
                "INSERT INTO canvas_links (canvas_path, node_id, target_path, anchor_kind, label)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(|e| e.to_string())?;
        for (node_id, target_path, anchor_kind, label) in links {
            stmt.execute(params![
                normalized_canvas,
                node_id,
                target_path,
                anchor_kind,
                label
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

pub fn remove_canvas_links(db_path: &Path, canvas_path: &str) -> Result<(), String> {
    let Some(conn) = open_links_db(db_path, false)? else {
        return Ok(());
    };
    conn.execute(
        "DELETE FROM canvas_links WHERE canvas_path = ?1",
        [canvas_path.replace('\\', "/")],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn backlinks_for(db_path: &Path, target_path: &str) -> Vec<CanvasBacklink> {
    let Ok(Some(conn)) = open_links_db(db_path, false) else {
        return Vec::new();
    };
    let normalized = target_path.replace('\\', "/");
    let Ok(mut stmt) = conn.prepare(
        "SELECT canvas_path, node_id, anchor_kind, label
         FROM canvas_links
         WHERE target_path = ?1
         ORDER BY canvas_path, node_id",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([normalized], |row| {
        Ok(CanvasBacklink {
            canvas_path: row.get(0)?,
            node_id: row.get(1)?,
            anchor_kind: row.get(2)?,
            label: row.get(3)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// Moves index keys when canvas files move. Target paths follow the saved canvas contents.
pub fn relocate_canvas_paths(
    db_path: &Path,
    moves: &[(String, String)],
    is_folder: bool,
) -> Result<(), String> {
    if moves.is_empty() {
        return Ok(());
    }
    let Some(mut conn) = open_links_db(db_path, false)? else {
        return Ok(());
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let paths = {
        let mut stmt = tx
            .prepare("SELECT DISTINCT canvas_path FROM canvas_links")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        mapped.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
    };
    for path in paths {
        if let Some(next) = remap_rel_path(&path, moves, is_folder) {
            tx.execute(
                "UPDATE canvas_links SET canvas_path = ?1 WHERE canvas_path = ?2",
                params![next, path],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{anchor::KnowledgeAnchor, model::CanvasNode};

    #[test]
    fn sync_and_query_canvas_backlinks() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join(".folio").join("metadata.sqlite3");
        let mut doc = CanvasDocument::default();
        doc.nodes.push(CanvasNode::new_anchor(
            "n1",
            0.0,
            0.0,
            "核心结论",
            KnowledgeAnchor::MarkdownQuote {
                path: "notes/a.md".into(),
                quote: "abc".into(),
                occurrence: 0,
                heading_path: vec![],
                line_hint: None,
            },
        ));
        sync_canvas_links(&db, "maps/overview.canvas", &doc).unwrap();
        let links = backlinks_for(&db, "notes/a.md");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].canvas_path, "maps/overview.canvas");
        assert_eq!(links[0].label, "核心结论");

        relocate_canvas_paths(&db, &[("maps/overview.canvas".into(), "maps/renamed.canvas".into())], false).unwrap();
        assert_eq!(backlinks_for(&db, "notes/a.md")[0].canvas_path, "maps/renamed.canvas");
        doc.relocate_paths(&[("notes/a.md".into(), "notes/b.md".into())], false);
        sync_canvas_links(&db, "maps/renamed.canvas", &doc).unwrap();
        assert!(backlinks_for(&db, "notes/a.md").is_empty());
        assert_eq!(backlinks_for(&db, "notes/b.md").len(), 1);

        let mut other = CanvasDocument::default();
        other.nodes.push(CanvasNode::new_anchor(
            "n2",
            0.0,
            0.0,
            "保留引用",
            KnowledgeAnchor::MarkdownFile {
                path: "notes/b.md".into(),
            },
        ));
        sync_canvas_links(&db, "maps/other.canvas", &other).unwrap();
        remove_canvas_links(&db, "maps/renamed.canvas").unwrap();
        let remaining = backlinks_for(&db, "notes/b.md");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].canvas_path, "maps/other.canvas");
    }
}
