//! Persistent file trash for Folio.
//!
//! Each item owns a UUID directory containing its copied payload and metadata.
//! A source file is removed only after both have been written durably, so an
//! interrupted put leaves either the original or a listed trash copy.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const PAYLOAD_FILE: &str = "payload";
const METADATA_FILE: &str = "entry.json";
const METADATA_TEMP_FILE: &str = "entry.json.tmp";

/// A persisted file deletion and the information needed to restore it.
#[derive(Clone, Serialize, Deserialize)]
pub struct TrashEntry {
    pub id: String,
    pub batch: String,
    pub project_id: String,
    pub project_root: PathBuf,
    pub relative: String,
    pub deleted_at: u64,
    #[serde(default)]
    pub deleted_at_nanos: u32,
    pub was_main: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

fn trash_directory() -> io::Result<PathBuf> {
    Ok(crate::storage::data_directory()?.join("trash"))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync_directory(parent)
}

fn entry_directory(id: &str) -> io::Result<PathBuf> {
    let parsed = Uuid::parse_str(id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::workspace::ERR_TRASH_INVALID_ID.get()))?;
    if parsed.to_string() != id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_TRASH_INVALID_ID.get(),
        ));
    }
    Ok(trash_directory()?.join(id))
}

fn read_entry(directory: &Path) -> io::Result<TrashEntry> {
    let bytes = fs::read(directory.join(METADATA_FILE))?;
    let entry: TrashEntry = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let directory_id = directory.file_name().and_then(|name| name.to_str());
    if directory_id != Some(entry.id.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            crate::i18n::workspace::ERR_TRASH_MISMATCH_DIR.get(),
        ));
    }
    Ok(entry)
}

fn persist_entry(directory: &Path, entry: &TrashEntry) -> io::Result<()> {
    let bytes = serde_json::to_vec(entry).map_err(io::Error::other)?;
    let temporary = directory.join(METADATA_TEMP_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);

    // The final name appears atomically only after the complete JSON is durable.
    fs::rename(&temporary, directory.join(METADATA_FILE))?;
    sync_directory(directory)
}

/// Return every persisted trash item. Incomplete directories from an interrupted
/// put have no final metadata file and are ignored; malformed metadata is an error.
pub fn list() -> io::Result<Vec<TrashEntry>> {
    let root = trash_directory()?;
    let directories = match fs::read_dir(&root) {
        Ok(directories) => directories,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut entries = Vec::new();
    for item in directories {
        let item = item?;
        if !item.file_type()?.is_dir() {
            continue;
        }
        let name = item.file_name();
        let Some(id) = name.to_str() else { continue };
        let Ok(parsed) = Uuid::parse_str(id) else {
            continue;
        };
        if parsed.to_string() != id {
            continue;
        }
        match read_entry(&item.path()) {
            Ok(entry) => entries.push(entry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
    }
    entries.sort_by(|left, right| {
        left.deleted_at
            .cmp(&right.deleted_at)
            .then_with(|| left.deleted_at_nanos.cmp(&right.deleted_at_nanos))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(entries)
}

/// Copy a project file into persistent trash, publish its metadata, then remove
/// the source. Any failure before the final removal leaves the source in place.
pub fn put(
    project_id: &str,
    root: &Path,
    relative: &str,
    batch: &str,
    was_main: bool,
) -> crate::tags::Result<TrashEntry> {
    let source = crate::files::project_path(root, relative)?;
    let tags = crate::tags::Store::project(root);
    let file_id = tags.tracked_id(relative)?;
    let mut source_file = File::open(&source)?;
    let source_metadata = source_file.metadata()?;
    if !source_metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_TRASH_FILES_ONLY.get(),
        ).into());
    }

    let trash_root = trash_directory()?;
    fs::create_dir_all(&trash_root)?;
    sync_parent(&trash_root)?;

    let id = Uuid::new_v4().to_string();
    let directory = trash_root.join(&id);
    fs::create_dir(&directory)?;
    sync_directory(&trash_root)?;

    let payload_path = directory.join(PAYLOAD_FILE);
    let mut payload = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&payload_path)?;
    io::copy(&mut source_file, &mut payload)?;
    payload.sync_all()?;
    drop(payload);
    sync_directory(&directory)?;

    let deleted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let entry = TrashEntry {
        id,
        batch: batch.to_owned(),
        project_id: project_id.to_owned(),
        project_root: root.to_path_buf(),
        relative: relative.to_owned(),
        deleted_at: deleted_at.as_secs(),
        deleted_at_nanos: deleted_at.subsec_nanos(),
        was_main,
        file_id,
    };
    persist_entry(&directory, &entry)?;

    // The durable trash entry carries the UUID even if the process stops
    // between removing the source and committing the registry transaction.
    if let Some(file_id) = &entry.file_id {
        tags.trash_file(file_id, &entry.id, || fs::remove_file(&source))?;
    } else {
        fs::remove_file(&source)?;
    }
    sync_parent(&source)?;
    let rag_db = root.join(".folio").join("rag.sqlite3");
    if let Some(mut rag) = crate::rag::store::RagStore::open_existing(&rag_db)? {
        rag.trash_document(&entry.relative, &entry.id)?;
    }
    if entry.relative.to_ascii_lowercase().ends_with(".canvas") {
        let meta_db = crate::canvas::index::metadata_db_path(Some(root))?;
        crate::canvas::index::trash_canvas_links(&meta_db, &entry.relative, &entry.id)?;
    } else if entry.relative.to_ascii_lowercase().ends_with(".md") {
        let meta_db = crate::markdown::links::metadata_db_path(Some(root))?;
        crate::markdown::links::trash_markdown_links(&meta_db, &entry.relative, &entry.id)?;
    }
    Ok(entry)
}

/// Restore into a new target file. The target and its parent are synced before
/// removing the stored copy, so a failed or interrupted restore keeps recoverable data.
pub fn restore(entry: &TrashEntry, target: &Path) -> crate::tags::Result<()> {
    let directory = entry_directory(&entry.id)?;
    read_entry(&directory)?;

    let mut payload = File::open(directory.join(PAYLOAD_FILE))?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    if let Err(error) =
        io::copy(&mut payload, &mut destination).and_then(|_| destination.sync_all())
    {
        drop(destination);
        fs::remove_file(target)?;
        return Err(error.into());
    }
    drop(destination);
    sync_parent(target)?;

    if let Some(file_id) = &entry.file_id {
        let relative = target.strip_prefix(&entry.project_root)?.to_string_lossy().replace('\\', "/");
        let tags = crate::tags::Store::project(&entry.project_root);
        if let Err(error) = tags.restore_file(file_id, &relative) {
            // This call created the target exclusively. Keep the durable
            // payload available so fixing the tag store allows a retry.
            fs::remove_file(target)?;
            return Err(error);
        }
    }

    fs::remove_file(directory.join(PAYLOAD_FILE))?;
    sync_directory(&directory)?;
    fs::remove_file(directory.join(METADATA_FILE))?;
    sync_directory(&directory)?;
    fs::remove_dir_all(&directory)?;
    sync_parent(&directory)?;

    let relative = target
        .strip_prefix(&entry.project_root)?
        .to_string_lossy()
        .replace('\\', "/");
    if relative.to_ascii_lowercase().ends_with(".canvas") {
        let meta_db = crate::canvas::index::metadata_db_path(Some(&entry.project_root))?;
        crate::canvas::index::restore_canvas_links(&meta_db, &entry.id, &relative, target)?;
    } else if relative.to_ascii_lowercase().ends_with(".md") {
        let meta_db = crate::markdown::links::metadata_db_path(Some(&entry.project_root))?;
        crate::markdown::links::restore_markdown_links(&meta_db, &entry.id, &relative, target)?;
    }
    let rag_db = entry.project_root.join(".folio").join("rag.sqlite3");
    if let Some(mut rag) = crate::rag::store::RagStore::open_existing(&rag_db)? {
        let meta = fs::metadata(target)?;
        let file_size = meta.len() as i64;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);
        rag.restore_document(&entry.id, &relative, mtime_ns, file_size)?;
    }
    Ok(())
}

/// Permanently remove one persisted trash item.
pub fn purge(entry: &TrashEntry) -> crate::tags::Result<()> {
    let directory = entry_directory(&entry.id)?;
    read_entry(&directory)?;
    let remove = || fs::remove_dir_all(&directory).and_then(|_| sync_parent(&directory));
    if let Some(file_id) = &entry.file_id {
        crate::tags::Store::project(&entry.project_root).purge_file(file_id, &entry.id, remove)?;
    } else {
        remove()?;
    }
    if entry.relative.to_ascii_lowercase().ends_with(".canvas") {
        let meta_db = crate::canvas::index::metadata_db_path(Some(&entry.project_root))?;
        crate::canvas::index::purge_trashed_canvas_links(&meta_db, &entry.id)?;
    } else if entry.relative.to_ascii_lowercase().ends_with(".md") {
        let meta_db = crate::markdown::links::metadata_db_path(Some(&entry.project_root))?;
        crate::markdown::links::purge_trashed_markdown_links(&meta_db, &entry.id)?;
    }
    let rag_db = entry.project_root.join(".folio").join("rag.sqlite3");
    if let Some(mut rag) = crate::rag::store::RagStore::open_existing(&rag_db)? {
        rag.purge_trashed_document(&entry.id)?;
    }
    Ok(())
}

/// Permanently remove entries whose recorded deletion time is more than seven days old.
pub fn cleanup() -> crate::tags::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let cutoff = now.saturating_sub(RETENTION_SECS);
    for entry in list()? {
        if entry.deleted_at <= cutoff {
            purge(&entry)?;
        }
    }
    Ok(())
}
