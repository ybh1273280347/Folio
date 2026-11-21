//! Per-directory annotation records, shared by manual edits and tool writes.

use super::AnnotationItem;
use crate::storage::{read_json, write_json};
use std::{io, path::{Path, PathBuf}};

fn annotations_store_path(target_file: &Path) -> PathBuf {
    target_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".folio")
        .join("annotations.json")
}

pub fn load_annotations(target_file: &Path) -> Vec<AnnotationItem> {
    try_load_annotations(target_file).unwrap_or_default()
}

/// Tool writes must distinguish an empty store from a damaged or unreadable existing store.
pub fn try_load_annotations(target_file: &Path) -> io::Result<Vec<AnnotationItem>> {
    let store = annotations_store_path(target_file);
    let fname = target_file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let all: Vec<AnnotationItem> = read_json(&store)?;
    Ok(all.into_iter().filter(|item| item.file == fname).collect())
}

pub fn save_annotations(target_file: &Path, items: &[AnnotationItem]) -> io::Result<()> {
    let store = annotations_store_path(target_file);
    let fname = target_file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut all: Vec<AnnotationItem> = read_json(&store)?;
    all.retain(|item| item.file != fname);
    for item in items {
        let mut entry = item.clone();
        entry.file = fname.clone();
        all.push(entry);
    }
    write_json(&store, &all)
}

pub fn relocate_annotations(source: &Path, target: &Path) -> io::Result<()> {
    let store = annotations_store_path(source);
    let old = source.file_name().expect("source file name").to_string_lossy();
    let new = target.file_name().expect("target file name").to_string_lossy();
    let mut entries: Vec<AnnotationItem> = read_json(&store)?;
    if !entries.iter().any(|item| item.file == old) {
        return Ok(());
    }
    if store == annotations_store_path(target) {
        // A rename within a directory replaces the filename in one sidecar write.
        entries.retain(|item| item.file != new);
        for item in entries.iter_mut().filter(|item| item.file == old) {
            item.file = new.to_string();
        }
        write_json(&store, &entries)
    } else {
        let (moved, remaining): (Vec<_>, Vec<_>) =
            entries.into_iter().partition(|item| item.file == old);
        // Preserve the source records if the destination cannot be written.
        save_annotations(target, &moved)?;
        write_json(&store, &remaining)
    }
}
