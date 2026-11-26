//! Project-local source fingerprints written by exporters and read by the knowledge index.
//! Match exact output bytes so a renamed PDF stays recognizable and edited Markdown can be indexed independently.

use crate::files::sha256_hex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const DERIVED_PDF_KEYWORD: &str = "folio-derived";

// Parallel build workers must finish each read-modify-write before another worker reads.
static MANIFEST_WRITES: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DerivedPdfEntry {
    pub pdf_hash: String,
    pub source_path: String,
    pub pdf_path: String,
    pub origin: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DerivedMarkdownEntry {
    pub source_pdf_path: String,
    pub source_pdf_hash: String,
    pub markdown_hash: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DerivedPdfManifest {
    #[serde(default)]
    entries: Vec<DerivedPdfEntry>,
}

fn derived_manifest_path(project_root: &Path) -> PathBuf {
    project_root.join(".folio").join("derived-pdfs.json")
}

fn project_rel_slash(project_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(project_root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() {
        None
    } else {
        Some(rel_str)
    }
}

fn write_manifest(path: &Path, value: &impl Serialize) -> io::Result<()> {
    // Indexing can read while exports write. Publish a complete replacement from the same directory.
    let parent = path.parent().expect("manifest parent");
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(staged.as_file_mut(), value).map_err(io::Error::other)?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn record_derived_pdf(
    project_root: &Path,
    source_path: &Path,
    pdf_path: &Path,
    pdf_bytes: &[u8],
    origin: &str,
) -> io::Result<()> {
    let Some(pdf_rel) = project_rel_slash(project_root, pdf_path) else {
        return Ok(());
    };
    let source_rel = project_rel_slash(project_root, source_path).unwrap_or_default();
    let pdf_hash = sha256_hex(pdf_bytes);
    let manifest_path = derived_manifest_path(project_root);

    let _guard = MANIFEST_WRITES.lock().expect("manifest write lock poisoned");
    let mut manifest: DerivedPdfManifest = fs::read(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();

    manifest.entries.retain(|e| e.pdf_hash != pdf_hash);
    manifest.entries.push(DerivedPdfEntry {
        pdf_hash,
        source_path: source_rel,
        pdf_path: pdf_rel,
        origin: origin.to_owned(),
    });

    write_manifest(&manifest_path, &manifest)
}

pub fn load_derived_pdf_hashes(project_root: &Path) -> HashSet<String> {
    let manifest_path = derived_manifest_path(project_root);
    let Ok(raw) = fs::read(&manifest_path) else {
        return HashSet::new();
    };
    let Ok(manifest) = serde_json::from_slice::<DerivedPdfManifest>(&raw) else {
        return HashSet::new();
    };
    manifest.entries.into_iter().map(|e| e.pdf_hash).collect()
}

fn derived_markdown_manifest_path(project_root: &Path) -> PathBuf {
    project_root.join(".folio").join("derived-markdowns.json")
}

/// Record the PDF source and exact exported text so later indexing can distinguish
/// an untouched convenience export from a Markdown file the user edited.
pub fn record_derived_markdown(
    project_root: &Path,
    markdown_path: &Path,
    source_pdf_path: &Path,
    source_pdf_hash: &str,
    markdown: &str,
) -> io::Result<()> {
    let Some(path) = project_rel_slash(project_root, markdown_path) else {
        return Ok(());
    };
    let manifest_path = derived_markdown_manifest_path(project_root);
    let _guard = MANIFEST_WRITES.lock().expect("manifest write lock poisoned");
    let mut entries: HashMap<String, DerivedMarkdownEntry> = fs::read(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    entries.insert(
        path,
        DerivedMarkdownEntry {
            source_pdf_path: project_rel_slash(project_root, source_pdf_path).unwrap_or_default(),
            source_pdf_hash: source_pdf_hash.to_owned(),
            markdown_hash: sha256_hex(markdown.as_bytes()),
        },
    );
    write_manifest(&manifest_path, &entries)
}

pub fn load_derived_markdown_entries(project_root: &Path) -> HashMap<String, DerivedMarkdownEntry> {
    fs::read(derived_markdown_manifest_path(project_root))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

pub fn has_folio_pdf_fingerprint(bytes: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"/Keywords (folio-derived)",
        b"/Keywords(folio-derived)",
        b"<pdf:Keywords>folio-derived</pdf:Keywords>",
    ];
    MARKERS
        .iter()
        .any(|needle| bytes.windows(needle.len()).any(|w| w == *needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread};

    #[test]
    fn concurrent_pdf_exports_preserve_both_fingerprints() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let start = Barrier::new(2);
        thread::scope(|scope| {
            let first = scope.spawn(|| {
                start.wait();
                record_derived_pdf(
                    root,
                    &root.join("a.tex"),
                    &root.join("a.pdf"),
                    b"pdf A",
                    "tex",
                )
            });
            start.wait();
            record_derived_pdf(
                root,
                &root.join("b.tex"),
                &root.join("b.pdf"),
                b"pdf B",
                "tex",
            )
            .unwrap();
            first.join().unwrap().unwrap();
        });
        assert_eq!(
            load_derived_pdf_hashes(root),
            HashSet::from([sha256_hex(b"pdf A"), sha256_hex(b"pdf B")]),
        );
    }
}
