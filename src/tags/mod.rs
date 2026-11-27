//! Folio-owned file tags. Directory inventories and filesystem identity are
//! inputs to reconciliation, while this database owns tag membership.

pub mod agent;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub color: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub tags: Vec<Tag>,
    pub assigned: BTreeMap<String, Vec<String>>,
    pub missing: Vec<(String, String)>,
}

impl Snapshot {
    pub fn has(&self, path: &str, tag: &str) -> bool {
        self.assigned
            .get(path)
            .is_some_and(|ids| ids.iter().any(|id| id == tag))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileRecord {
    id: String,
    path: String,
    disk: Option<String>,
    missing: bool,
    trash: Option<String>,
}

#[derive(Clone)]
pub struct Store {
    root: Option<PathBuf>,
    db: PathBuf,
}

pub enum Change<'a> {
    Create {
        name: &'a str,
        paths: &'a [String],
    },
    Assign {
        tag: &'a str,
        paths: &'a [String],
        add: bool,
    },
    Rename {
        tag: &'a str,
        name: &'a str,
    },
    Color {
        tag: &'a str,
        color: u8,
    },
    Delete(&'a str),
    Rebind {
        missing: &'a str,
        target: &'a str,
    },
}

/// A filesystem scan is prepared off the UI thread. Its writes are conditional
/// on the same registry being current when the scan returns.
pub struct Scan {
    origin: String,
    current_root: String,
    before: Vec<FileRecord>,
    after: Vec<FileRecord>,
}

pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn label(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(|c| c.is_control() || c == '/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::tags::ERR_INVALID_TAG_NAME.get(),
        )
        .into());
    }
    Ok(name)
}

impl Store {
    pub fn project(root: &Path) -> Self {
        Self {
            db: root.join(".folio").join("metadata.sqlite3"),
            root: Some(root.to_owned()),
        }
    }

    pub fn loose() -> io::Result<Self> {
        Ok(Self {
            root: None,
            db: crate::storage::data_directory()?.join("loose-metadata.sqlite3"),
        })
    }

    fn root_key(&self) -> String {
        self.root
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn absolute(&self, path: &str) -> PathBuf {
        match &self.root {
            Some(root) => root.join(path),
            None => PathBuf::from(path),
        }
    }

    fn open(&self, create: bool) -> Result<Option<Connection>> {
        if !self.db.try_exists()? && !create {
            return Ok(None);
        }
        if create {
            fs::create_dir_all(self.db.parent().expect("metadata database has a parent"))?;
        }
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let mut db = Connection::open_with_flags(&self.db, flags)?;
        db.busy_timeout(Duration::from_millis(250))?;
        db.pragma_update(None, "foreign_keys", true)?;
        let version = db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
        match version {
            1 => {}
            0 => {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(
                    "CREATE TABLE IF NOT EXISTS workspace (id INTEGER PRIMARY KEY CHECK (id = 1), origin TEXT NOT NULL);
                     CREATE TABLE IF NOT EXISTS files (
                        id TEXT PRIMARY KEY, path TEXT NOT NULL, disk TEXT,
                        missing INTEGER NOT NULL DEFAULT 0, trash TEXT
                     );
                     CREATE UNIQUE INDEX IF NOT EXISTS active_path ON files(path) WHERE missing = 0 AND trash IS NULL;
                     CREATE TABLE IF NOT EXISTS tags (
                        id TEXT PRIMARY KEY, name TEXT NOT NULL, name_key TEXT NOT NULL UNIQUE,
                        color INTEGER NOT NULL DEFAULT 0
                     );
                     CREATE TABLE IF NOT EXISTS file_tags (
                        file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                        tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                        PRIMARY KEY (file_id, tag_id)
                     );
                     CREATE INDEX IF NOT EXISTS tag_files ON file_tags(tag_id);
                     PRAGMA user_version = 1;",
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO workspace VALUES (1, ?1)",
                    [self.root_key()],
                )?;
                tx.commit()?;
            }
            _ => return Err(io::Error::other(crate::i18n::tags::ERR_NEWER_TAG_DB.get()).into()),
        }
        Ok(Some(db))
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let Some(db) = self.open(false)? else {
            return Ok(Snapshot::default());
        };
        snapshot(&db)
    }

    pub fn change(&self, change: Change<'_>) -> Result<Snapshot> {
        let mut db = self
            .open(matches!(change, Change::Create { .. }))?
            .ok_or_else(|| io::Error::other(crate::i18n::tags::ERR_TAG_DB_NOT_FOUND.get()))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.apply_change(&tx, change)?;
        let result = snapshot_after_changes(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    /// Both the sidebar and an approved AI batch use these same write rules.
    fn apply_change(&self, db: &Connection, change: Change<'_>) -> Result<()> {
        match change {
            Change::Create { name, paths } => {
                let name = label(name)?;
                let key = name_key(name);
                db.execute("INSERT INTO tags (id, name, name_key) VALUES (?1, ?2, ?3) ON CONFLICT(name_key) DO NOTHING",
                    params![Uuid::new_v4().to_string(), name, key])?;
                let id: String =
                    db.query_row("SELECT id FROM tags WHERE name_key = ?1", [key], |row| {
                        row.get(0)
                    })?;
                self.assign(db, &id, paths, true)?;
            }
            Change::Assign { tag, paths, add } => self.assign(db, tag, paths, add)?,
            Change::Rename { tag, name } => {
                let name = label(name)?;
                let key = name_key(name);
                if db
                    .query_row(
                        "SELECT 1 FROM tags WHERE name_key = ?1 AND id <> ?2",
                        params![key, tag],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some()
                {
                    return Err(io::Error::other(crate::i18n::tags::ERR_TAG_ALREADY_EXISTS.get()).into());
                }
                db.execute(
                    "UPDATE tags SET name = ?1, name_key = ?2 WHERE id = ?3",
                    params![name, key, tag],
                )?;
            }
            Change::Color { tag, color } => {
                db.execute(
                    "UPDATE tags SET color = ?1 WHERE id = ?2",
                    params![color, tag],
                )?;
            }
            Change::Delete(tag) => {
                db.execute("DELETE FROM tags WHERE id = ?1", [tag])?;
            }
            Change::Rebind { missing, target } => {
                db.query_row(
                    "SELECT 1 FROM files WHERE id = ?1 AND missing = 1 AND trash IS NULL",
                    [missing],
                    |_| Ok(()),
                )?;
                let id = self.bind(db, target, Some(missing))?;
                if id != missing {
                    db.execute("INSERT OR IGNORE INTO file_tags SELECT ?1, tag_id FROM file_tags WHERE file_id = ?2", params![id, missing])?;
                    db.execute("DELETE FROM files WHERE id = ?1", [missing])?;
                }
            }
        }
        Ok(())
    }

    fn assign(&self, db: &Connection, tag: &str, paths: &[String], add: bool) -> Result<()> {
        for path in paths {
            if add {
                let id = self.bind(db, path, None)?;
                db.execute(
                    "INSERT OR IGNORE INTO file_tags VALUES (?1, ?2)",
                    params![id, tag],
                )?;
            } else {
                db.execute("DELETE FROM file_tags WHERE tag_id = ?1 AND file_id IN
                            (SELECT id FROM files WHERE path = ?2 AND missing = 0 AND trash IS NULL)", params![tag, path])?;
            }
        }
        Ok(())
    }

    fn bind(&self, db: &Connection, path: &str, recover: Option<&str>) -> Result<String> {
        let full = match &self.root {
            Some(root) => crate::files::project_path(root, path)?,
            None => dunce::canonicalize(path)?,
        };
        if !fs::metadata(&full)?.is_file() || !crate::files::is_relevant(&full) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::tags::ERR_SELECT_SUPPORTED_FILE.get()).into());
        }
        let disk = disk_identity(&full);
        if let Some(existing) = active_file(db, path)? {
            if same_or_unavailable(&existing.disk, &disk) {
                db.execute(
                    "UPDATE files SET disk = COALESCE(?1, disk) WHERE id = ?2",
                    params![disk, existing.id],
                )?;
                return Ok(existing.id);
            }
            db.execute("UPDATE files SET missing = 1 WHERE id = ?1", [&existing.id])?;
        }
        if let Some(id) = recover {
            db.execute(
                "UPDATE files SET path = ?1, disk = ?2, missing = 0 WHERE id = ?3",
                params![path, disk, id],
            )?;
            return Ok(id.to_owned());
        }
        let id = Uuid::new_v4().to_string();
        db.execute(
            "INSERT INTO files (id, path, disk) VALUES (?1, ?2, ?3)",
            params![id, path, disk],
        )?;
        Ok(id)
    }

    /// Apply successful in-app filesystem moves. Folder moves remap their
    /// descendants by component prefix, including open and closed files.
    pub fn relocate(&self, moves: &[(String, String)], folder: bool) -> Result<Snapshot> {
        let Some(mut db) = self.open(false)? else {
            return Ok(Snapshot::default());
        };
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (old, new) in moves {
            if folder {
                for file in records(&tx)?
                    .into_iter()
                    .filter(|file| file.trash.is_none())
                {
                    if let Some(suffix) = file
                        .path
                        .strip_prefix(old)
                        .and_then(|rest| rest.strip_prefix('/'))
                    {
                        self.relocate_file(
                            &tx,
                            file.id,
                            &format!("{new}/{suffix}"),
                            file.disk,
                            file.missing,
                        )?;
                    }
                }
            } else if let Some(file) = active_file(&tx, old)? {
                self.relocate_file(&tx, file.id, new, file.disk, false)?;
            }
        }
        let result = snapshot(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    fn relocate_file(
        &self,
        db: &Connection,
        id: String,
        new: &str,
        old_disk: Option<String>,
        missing: bool,
    ) -> Result<()> {
        let disk = disk_identity(&self.absolute(new));
        if missing || !same_or_unavailable(&old_disk, &disk) {
            db.execute("UPDATE files SET missing = 1 WHERE id = ?1", [&id])?;
        } else {
            db.execute("UPDATE files SET missing = 1 WHERE path = ?1 AND missing = 0 AND trash IS NULL AND id <> ?2",
                params![new, id])?;
            db.execute(
                "UPDATE files SET path = ?1, disk = COALESCE(?2, disk) WHERE id = ?3",
                params![new, disk, id],
            )?;
        }
        Ok(())
    }

    pub fn tracked_id(&self, path: &str) -> Result<Option<String>> {
        let Some(db) = self.open(false)? else {
            return Ok(None);
        };
        Ok(active_file(&db, path)?
            .filter(|file| same_or_unavailable(&file.disk, &disk_identity(&self.absolute(path))))
            .map(|file| file.id))
    }

    pub fn trash_file(
        &self,
        file: &str,
        trash: &str,
        remove_source: impl FnOnce() -> io::Result<()>,
    ) -> Result<()> {
        let mut db = self
            .open(false)?
            .ok_or_else(|| io::Error::other(crate::i18n::tags::ERR_TAG_DB_NOT_FOUND.get()))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE files SET trash = ?1, missing = 0 WHERE id = ?2",
            params![trash, file],
        )?;
        remove_source()?;
        tx.commit()?;
        Ok(())
    }

    pub fn restore_file(&self, file: &str, path: &str) -> Result<()> {
        let Some(mut db) = self.open(false)? else {
            return Ok(());
        };
        let disk = disk_identity(&self.absolute(path));
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE files SET missing = 1 WHERE path = ?1 AND missing = 0 AND trash IS NULL AND id <> ?2",
            params![path, file])?;
        tx.execute(
            "UPDATE files SET path = ?1, disk = ?2, missing = 0, trash = NULL WHERE id = ?3",
            params![path, disk, file],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn purge_file(
        &self,
        file: &str,
        trash: &str,
        remove: impl FnOnce() -> io::Result<()>,
    ) -> Result<()> {
        if let Some(mut db) = self.open(false)? {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "DELETE FROM files WHERE id = ?1 AND trash = ?2",
                params![file, trash],
            )?;
            remove()?;
            tx.commit()?;
        } else {
            remove()?;
        }
        Ok(())
    }

    /// Probe existing bindings first. The candidate inventory is only walked
    /// if at least one missing record has a usable filesystem identity.
    pub fn scan(&self, visible: &[String]) -> Result<Option<Scan>> {
        let Some(mut db) = self.open(false)? else {
            return Ok(None);
        };
        let read = db.transaction()?;
        let origin: String =
            read.query_row("SELECT origin FROM workspace WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        let before = records(&read)?;
        read.commit()?;
        let current_root = self.root_key();
        let migrated = origin != current_root;
        let mut after = before.clone();
        let mut claimed = BTreeSet::new();
        let mut seen = BTreeMap::new();
        for file in after
            .iter_mut()
            .filter(|file| file.trash.is_none() && !file.missing)
        {
            let path = self.absolute(&file.path);
            // The inventory is a discovery filter. An extension setting can hide
            // a tracked file without changing its identity or tag membership.
            let exists = file_exists(&path)?;
            let disk = exists.then(|| disk_identity(&path)).flatten();
            if exists && (migrated || same_or_unavailable(&file.disk, &disk)) {
                claimed.insert(file.path.clone());
                if disk.is_some() || migrated {
                    file.disk = disk.clone();
                }
            } else {
                file.missing = true;
            }
            seen.insert(file.path.clone(), disk);
        }
        if after
            .iter()
            .any(|file| file.missing && file.trash.is_none() && file.disk.is_some())
        {
            let mut paths: BTreeSet<_> = visible.iter().cloned().collect();
            // Reconcile previously missing records even if their last path has
            // since been excluded from the file tree.
            for file in after.iter().filter(|file| file.missing && file.trash.is_none() && file.disk.is_some()) {
                if file_exists(&self.absolute(&file.path))? {
                    paths.insert(file.path.clone());
                }
            }
            if self.root.is_none() {
                paths.extend(claimed.iter().cloned());
                let parents: BTreeSet<_> = after
                    .iter()
                    .filter(|file| file.trash.is_none())
                    .filter_map(|file| Path::new(&file.path).parent())
                    .collect();
                for parent in parents {
                    let entries = match fs::read_dir(parent) {
                        Ok(entries) => entries,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error.into()),
                    };
                    for entry in entries {
                        let entry = entry?;
                        if entry.file_type()?.is_file() && crate::files::is_relevant(&entry.path())
                        {
                            paths.insert(entry.path().to_string_lossy().into_owned());
                        }
                    }
                }
            }
            let mut by_disk = BTreeMap::<String, Vec<String>>::new();
            for path in paths {
                let disk = seen
                    .remove(&path)
                    .unwrap_or_else(|| disk_identity(&self.absolute(&path)));
                if let Some(disk) = disk {
                    by_disk.entry(disk).or_default().push(path);
                }
            }
            let mut missing_by_disk = BTreeMap::<String, usize>::new();
            for disk in after
                .iter()
                .filter(|file| file.missing && file.trash.is_none())
                .filter_map(|file| file.disk.as_ref())
            {
                *missing_by_disk.entry(disk.clone()).or_default() += 1;
            }
            for file in after
                .iter_mut()
                .filter(|file| file.missing && file.trash.is_none())
            {
                if let Some(disk) = &file.disk
                    && missing_by_disk[disk] == 1
                    && let Some([path]) = by_disk.get(disk).map(Vec::as_slice)
                    && claimed.insert(path.clone())
                {
                    file.path.clone_from(path);
                    file.missing = false;
                }
            }
        }
        Ok(Some(Scan {
            origin,
            current_root,
            before,
            after,
        }))
    }

    pub fn apply_scan(&self, scan: Option<Scan>) -> Result<Snapshot> {
        let Some(scan) = scan else {
            return self.snapshot();
        };
        if scan.before == scan.after && scan.origin == scan.current_root {
            return self.snapshot();
        }
        let Some(mut db) = self.open(false)? else {
            return Ok(Snapshot::default());
        };
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let origin: String =
            tx.query_row("SELECT origin FROM workspace WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        if origin == scan.origin && records(&tx)? == scan.before {
            // Release every changed binding before writing new locations. Two
            // tagged files can trade names between consecutive watcher scans.
            for (old, _) in scan
                .before
                .iter()
                .zip(&scan.after)
                .filter(|(old, new)| old != new)
            {
                tx.execute("UPDATE files SET missing = 1 WHERE id = ?1", [&old.id])?;
            }
            for (old, new) in scan
                .before
                .iter()
                .zip(&scan.after)
                .filter(|(old, new)| old != new)
            {
                tx.execute(
                    "UPDATE files SET path = ?1, disk = ?2, missing = ?3 WHERE id = ?4",
                    params![new.path, new.disk, new.missing, old.id],
                )?;
            }
            if origin != scan.current_root {
                tx.execute(
                    "UPDATE workspace SET origin = ?1 WHERE id = 1",
                    [scan.current_root],
                )?;
            }
        }
        let result = snapshot(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}

fn same_or_unavailable(stored: &Option<String>, current: &Option<String>) -> bool {
    stored
        .as_ref()
        .zip(current.as_ref())
        .is_none_or(|(old, new)| old == new)
}

fn file_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn file_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        disk: row.get(2)?,
        missing: row.get(3)?,
        trash: row.get(4)?,
    })
}

fn records(db: &Connection) -> rusqlite::Result<Vec<FileRecord>> {
    db.prepare("SELECT id, path, disk, missing, trash FROM files ORDER BY id")?
        .query_map([], file_record)?
        .collect()
}

fn active_file(db: &Connection, path: &str) -> rusqlite::Result<Option<FileRecord>> {
    db.query_row("SELECT id, path, disk, missing, trash FROM files WHERE path = ?1 AND missing = 0 AND trash IS NULL",
        [path], file_record).optional()
}

fn snapshot_after_changes(db: &Connection) -> Result<Snapshot> {
    // Only records with tag assignments need filesystem reconciliation.
    db.execute("DELETE FROM files WHERE NOT EXISTS
                (SELECT 1 FROM file_tags WHERE file_id = files.id)", [])?;
    snapshot(db)
}

fn snapshot(db: &Connection) -> Result<Snapshot> {
    let mut result = Snapshot::default();
    result.tags = db
        .prepare("SELECT id, name, color FROM tags ORDER BY name_key")?
        .query_map([], |row| {
            Ok(Tag {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut assigned = db.prepare(
        "SELECT files.path, tags.id FROM file_tags
         JOIN files ON files.id = file_tags.file_id JOIN tags ON tags.id = file_tags.tag_id
         WHERE files.missing = 0 AND files.trash IS NULL ORDER BY tags.name_key",
    )?;
    for row in assigned.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (path, tag) = row?;
        result.assigned.entry(path).or_default().push(tag);
    }
    result.missing = db
        .prepare(
            "SELECT id, path FROM files WHERE missing = 1 AND trash IS NULL AND
         EXISTS (SELECT 1 FROM file_tags WHERE file_id = files.id) ORDER BY path, id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(result)
}

#[cfg(windows)]
fn disk_identity(path: &Path) -> Option<String> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_BASIC_INFO, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_REMOTE_PROTOCOL_INFO,
            FileBasicInfo, FileIdInfo, FileRemoteProtocolInfo, GetFileInformationByHandleEx,
            GetVolumeInformationByHandleW,
        },
    };

    // Metadata-only handles preserve sharing with readers, writers and
    // movers. Unavailable IDs leave the path usable without move inference.
    let file = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .open(path)
        .ok()?;
    let handle = HANDLE(file.as_raw_handle());
    let mut system = [0_u16; 32];
    let mut id = FILE_ID_INFO::default();
    let mut basic = FILE_BASIC_INFO::default();
    let mut remote = FILE_REMOTE_PROTOCOL_INFO::default();
    // The borrowed handle is live and each output buffer matches its info class.
    unsafe {
        GetVolumeInformationByHandleW(handle, None, None, None, None, Some(&mut system)).ok()?;
        const NTFS: [u16; 5] = [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0];
        if !system.starts_with(&NTFS)
            || (GetFileInformationByHandleEx(
                handle,
                FileRemoteProtocolInfo,
                (&mut remote as *mut FILE_REMOTE_PROTOCOL_INFO).cast(),
                size_of::<FILE_REMOTE_PROTOCOL_INFO>() as u32,
            )
            .is_ok()
                && remote.Protocol != 0)
        {
            return None;
        }
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
        .ok()?;
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
        .ok()?;
    }
    let identifier = u128::from_le_bytes(id.FileId.Identifier);
    (identifier != 0 && basic.CreationTime > 0).then(|| {
        format!(
            "{:016x}:{identifier:032x}:{:016x}",
            id.VolumeSerialNumber, basic.CreationTime
        )
    })
}

#[cfg(not(windows))]
fn disk_identity(_path: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_format_labels_persist_and_failed_batch_rolls_back() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("讲义.md"), "笔记")?;
        fs::write(root.path().join("习题.pdf"), b"%PDF-")?;
        let store = Store::project(root.path());
        let paths = ["讲义.md".to_owned(), "习题.pdf".to_owned()];
        let initial = store.change(Change::Create {
            name: "待复习",
            paths: &paths,
        })?;
        let tag = &initial.tags[0].id;
        let invalid = ["讲义.md".to_owned(), "找不到.pdf".to_owned()];
        assert!(
            store
                .change(Change::Create {
                    name: "重点",
                    paths: &invalid
                })
                .is_err()
        );

        let reopened = Store::project(root.path()).snapshot()?;
        assert_eq!(reopened.tags.len(), 1);
        assert!(reopened.has("讲义.md", tag));
        assert!(reopened.has("习题.pdf", tag));
        assert_eq!(fs::read_to_string(root.path().join("讲义.md"))?, "笔记");
        assert_eq!(fs::read(root.path().join("习题.pdf"))?, b"%PDF-");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn external_rename_keeps_identity_and_same_name_replacement_does_not() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("旧.md"), b"before")?;
        let store = Store::project(root.path());
        let initial = store.change(Change::Create {
            name: "重点",
            paths: &["旧.md".into()],
        })?;
        let tag = &initial.tags[0].id;
        let file = store.tracked_id("旧.md")?.expect("tagged file has a UUID");

        fs::rename(root.path().join("旧.md"), root.path().join("改名.md"))?;
        fs::write(root.path().join("旧.md"), b"another file")?;
        let renamed = store.apply_scan(store.scan(&["旧.md".into(), "改名.md".into()])?)?;
        assert!(renamed.has("改名.md", tag));
        assert!(!renamed.has("旧.md", tag));
        assert_eq!(store.tracked_id("改名.md")?, Some(file.clone()));

        fs::rename(
            root.path().join("改名.md"),
            root.path().join("old.unsupported"),
        )?;
        fs::write(root.path().join("改名.md"), b"replacement")?;
        let replaced = store.apply_scan(store.scan(&["旧.md".into(), "改名.md".into()])?)?;
        assert!(!replaced.has("改名.md", tag));
        assert_eq!(replaced.missing, [(file.clone(), "改名.md".into())]);
        let restored = store.change(Change::Rebind {
            missing: &file,
            target: "改名.md",
        })?;
        assert!(restored.has("改名.md", tag));
        assert_eq!(store.tracked_id("改名.md")?, Some(file));
        Ok(())
    }

    #[test]
    fn a_companion_database_copies_with_the_project() -> Result<()> {
        let original = tempfile::tempdir()?;
        let copy = tempfile::tempdir()?;
        fs::write(original.path().join("研读.md"), b"document")?;
        let source = Store::project(original.path());
        let initial = source.change(Change::Create {
            name: "待复习",
            paths: &["研读.md".into()],
        })?;
        let tag = &initial.tags[0].id;
        let destination = Store::project(copy.path());
        assert!(destination.snapshot()?.tags.is_empty());
        fs::create_dir(copy.path().join(".folio"))?;
        fs::copy(&source.db, &destination.db)?;
        fs::copy(original.path().join("研读.md"), copy.path().join("研读.md"))?;

        let copied = destination.apply_scan(destination.scan(&["研读.md".into()])?)?;
        assert!(copied.has("研读.md", tag));
        destination.change(Change::Delete(tag))?;
        assert!(source.snapshot()?.has("研读.md", tag));
        Ok(())
    }

    #[test]
    fn trash_restores_membership_to_a_new_filename_and_purge_removes_it() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store::project(root.path());
        let source = root.path().join("课后题.md");
        fs::write(&source, "课后题")?;
        let original = store.change(Change::Create {
            name: "待复习",
            paths: &["课后题.md".into()],
        })?;
        let tag = &original.tags[0].id;
        let file = store
            .tracked_id("课后题.md")?
            .expect("tagged file has a UUID");

        store.trash_file(&file, "first", || fs::remove_file(&source))?;
        let trashed = store.snapshot()?;
        assert!(trashed.assigned.is_empty());
        assert!(trashed.missing.is_empty());
        let restored = root.path().join("课后题 (1).md");
        fs::write(&restored, "课后题")?;
        store.restore_file(&file, "课后题 (1).md")?;
        assert!(store.snapshot()?.has("课后题 (1).md", tag));
        assert_eq!(store.tracked_id("课后题 (1).md")?, Some(file.clone()));

        store.trash_file(&file, "second", || fs::remove_file(&restored))?;
        store.purge_file(&file, "second", || Ok(()))?;
        fs::write(&restored, "新文件")?;
        store.restore_file(&file, "课后题 (1).md")?;
        assert!(store.snapshot()?.assigned.is_empty());
        Ok(())
    }
}
