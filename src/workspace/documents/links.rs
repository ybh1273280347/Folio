//! Markdown link resolution and navigation into a tab or a reading pane.

use super::*;
use crate::markdown::links as md_links;
use crate::model::InternalLinkOpenTarget;

impl Workspace {
    pub(in crate::workspace) fn is_markdown_path(path: &str) -> bool {
        path.to_ascii_lowercase().ends_with(".md")
    }

    pub(in crate::workspace) fn sync_markdown_links_from_text(
        &mut self,
        rel_path: &str,
        text: &str,
        disk_synced: bool,
        cx: &mut Context<Self>,
    ) {
        if !Self::is_markdown_path(rel_path) {
            return;
        }
        let Some(db_path) = self.canvas_metadata_db_path() else {
            return;
        };
        let full = disk_synced.then(|| self.document_path(rel_path));
        if let Err(error) =
            md_links::sync_markdown_links(&db_path, rel_path, text, full.as_deref())
        {
            self.report(error, cx);
        }
    }

    pub(in crate::workspace) fn sync_project_markdown_links(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.projects.directory() else {
            return;
        };
        let Some(db_path) = self.canvas_metadata_db_path() else {
            return;
        };
        let root = PathBuf::from(&project.folder);
        let md_paths: Vec<String> = self
            .tree
            .files()
            .iter()
            .filter(|f| Self::is_markdown_path(&f.path))
            .map(|f| f.path.clone())
            .collect();
        if let Err(error) = md_links::sync_project_markdown_links(&db_path, &root, &md_paths) {
            self.report(error, cx);
        }
    }

    pub(in crate::workspace) fn remove_markdown_links_index(
        &mut self,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        if !Self::is_markdown_path(rel_path) {
            return;
        }
        if let Some(db_path) = self.canvas_metadata_db_path()
            && let Err(error) = md_links::remove_markdown_links(&db_path, rel_path)
        {
            self.report(error, cx);
        }
    }

    /// Automatically refactors project-internal Markdown links and image paths after an
    /// in-app file or folder rename/move. Updates both inbound references and outbound
    /// relative paths of moved Markdown documents.
    pub(in crate::workspace) fn relocate_markdown_links(
        &mut self,
        moves: &[(String, String)],
        is_folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if moves.is_empty() {
            return;
        }
        let Some(project) = self.projects.directory() else {
            return;
        };
        let root = PathBuf::from(&project.folder);
        let Some(db_path) = self.canvas_metadata_db_path() else {
            return;
        };

        let mut candidates = BTreeSet::new();
        match md_links::sources_referencing_moves(&db_path, moves, is_folder) {
            Ok(sources) => {
                for src in sources {
                    let current = md_links::remap_rel_path_compat(&src, moves, is_folder)
                        .unwrap_or(src);
                    if Self::is_markdown_path(&current) {
                        candidates.insert(current);
                    }
                }
            }
            Err(error) => self.report(error, cx),
        }
        for file in self.tree.files() {
            if Self::is_markdown_path(&file.path)
                && md_links::reverse_remap_rel_path(&file.path, moves, is_folder).is_some()
            {
                candidates.insert(file.path.clone());
            }
        }
        for path in self.documents.open.keys() {
            if Self::is_markdown_path(path) {
                candidates.insert(path.clone());
            }
        }

        if let Err(error) = md_links::relocate_source_paths(&db_path, moves, is_folder) {
            self.report(error, cx);
        }

        let mut updated_files = 0usize;
        let mut updated_links = 0usize;
        let mut conflict_files = Vec::new();

        for current_path in candidates {
            let old_source = md_links::reverse_remap_rel_path(&current_path, moves, is_folder)
                .unwrap_or_else(|| current_path.clone());

            if let Some(buffer) = self.documents.open_buffer(&current_path) {
                let current_text = buffer.text.clone();
                if let Some((next_text, count)) = md_links::rewrite_markdown_links(
                    &current_text,
                    &old_source,
                    &current_path,
                    moves,
                    is_folder,
                ) {
                    self.edit_document(&current_path, next_text.clone());
                    self.sync_markdown_panes(&current_path, window, cx);
                    if let Err(error) =
                        md_links::sync_markdown_links(&db_path, &current_path, &next_text, None)
                    {
                        self.report(error, cx);
                    }
                    updated_files += 1;
                    updated_links += count;
                }
                continue;
            }

            let Ok(disk) = files::read_document(&root, &current_path) else {
                continue;
            };
            let Some((next_text, count)) = md_links::rewrite_markdown_links(
                &disk.text,
                &old_source,
                &current_path,
                moves,
                is_folder,
            ) else {
                continue;
            };
            match files::save_document(&root, &current_path, &next_text, &disk.revision) {
                Ok(Ok(saved)) => {
                    let full = self.document_path(&current_path);
                    if let Err(error) = md_links::sync_markdown_links(
                        &db_path,
                        &current_path,
                        &saved.text,
                        Some(&full),
                    ) {
                        self.report(error, cx);
                    }
                    updated_files += 1;
                    updated_links += count;
                }
                Ok(Err(_)) => conflict_files.push(current_path),
                Err(error) => self.report(error, cx),
            }
        }

        if updated_links > 0 {
            self.schedule_rag_sync();
            let message = if conflict_files.is_empty() {
                crate::i18n::workspace::report_relocated_markdown_links(
                    updated_files,
                    updated_links,
                )
            } else {
                crate::i18n::workspace::report_relocated_markdown_links_with_conflicts(
                    updated_files,
                    updated_links,
                    conflict_files.len(),
                )
            };
            self.report(message, cx);
        } else if let Some(first) = conflict_files.first() {
            self.report(
                crate::i18n::workspace::report_relocated_markdown_links_conflict(first),
                cx,
            );
        }
    }

    fn open_internal_link_file(
        &mut self,
        file: &ProjectFile,
        anchor: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.settings.internal_link_open_target == InternalLinkOpenTarget::SplitPane
            && Self::supports_split_pane(&file.path)
        {
            self.open_reading_pane(file, window, cx);
            let reading_key = PaneKey::reading(&file.path);
            if let Some(pane) = self.documents.markdown_panes.get(&reading_key).cloned() {
                pane.update(cx, |pane, cx| pane.focus(window, cx));
            }
            self.jump_markdown_anchor(&reading_key, anchor, window, cx);
            return Ok(());
        }
        self.open_file(file, window, cx)?;
        self.jump_markdown_anchor(&PaneKey::tab(&file.path), anchor, window, cx);
        Ok(())
    }

    pub(super) fn open_markdown_link(
        &mut self,
        source_file: &Path,
        raw_url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_root = self.projects.directory()
            .map(|project| PathBuf::from(&project.folder));
        let target = match classify_markdown_link(source_file, workspace_root.as_deref(), raw_url) {
            MarkdownLinkTarget::External(url) => {
                cx.open_url(&url);
                return;
            }
            MarkdownLinkTarget::Local { path, anchor } => (path, anchor),
            MarkdownLinkTarget::Invalid(message) => {
                self.report(message, cx);
                return;
            }
        };
        let (candidate, anchor) = target;
        let Ok(resolved) = dunce::canonicalize(&candidate) else {
            self.report(
                crate::i18n::workspace::err_link_target_not_found(&candidate),
                cx,
            );
            return;
        };
        if resolved.is_dir() {
            match url::Url::from_directory_path(&resolved) {
                Ok(dir_url) => cx.open_url(dir_url.as_str()),
                Err(()) => self.report(crate::i18n::workspace::err_open_dir(&resolved), cx),
            }
            return;
        }
        if let Some(root) = workspace_root
            && let Ok(canonical_root) = dunce::canonicalize(&root)
            && let Ok(relative) = resolved.strip_prefix(&canonical_root)
            && files::is_relevant(&resolved)
        {
            let key = relative.to_string_lossy().replace('\\', "/");
            let file = ProjectFile {
                kind: Self::file_kind(&resolved),
                path: key,
            };
            if let Err(error) =
                self.open_internal_link_file(&file, anchor.as_deref(), window, cx)
            {
                self.report(error, cx);
            }
            return;
        }
        if self.projects.directory().is_none() && files::is_relevant(&resolved) {
            match self.prepare_standalone_file(&resolved).and_then(|file| {
                self.open_internal_link_file(&file, anchor.as_deref(), window, cx)
            }) {
                Ok(()) => self.refresh_tags(cx),
                Err(error) => self.report(error, cx),
            }
            return;
        }
        match url::Url::from_file_path(&resolved) {
            Ok(file_url) => cx.open_url(file_url.as_str()),
            Err(()) => self.report(crate::i18n::workspace::err_open_file(&resolved), cx),
        }
    }

    fn jump_markdown_anchor(
        &mut self,
        key: &PaneKey,
        anchor: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = anchor.filter(|a| !a.is_empty()) else {
            return;
        };
        if !key.path.to_ascii_lowercase().ends_with(".md") {
            return;
        }
        if let Some(pane) = self.documents.markdown_panes.get(key).cloned() {
            pane.update(cx, |pane, cx| pane.navigate_to_anchor(anchor, window, cx));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MarkdownLinkTarget {
    External(String),
    Local {
        path: PathBuf,
        anchor: Option<String>,
    },
    Invalid(String),
}

fn classify_markdown_link(
    source_file: &Path,
    workspace_root: Option<&Path>,
    raw_url: &str,
) -> MarkdownLinkTarget {
    let raw = raw_url.trim();
    if raw.is_empty() {
        return MarkdownLinkTarget::Invalid(crate::i18n::workspace::ERR_EMPTY_LINK.get().to_owned());
    }
    if let Ok(parsed) = url::Url::parse(raw) {
        if parsed.scheme().eq_ignore_ascii_case("file") {
            return match parsed.to_file_path() {
                Ok(path) => MarkdownLinkTarget::Local {
                    path,
                    anchor: parsed.fragment().map(str::to_owned),
                },
                Err(()) => MarkdownLinkTarget::Invalid(
                    crate::i18n::workspace::err_resolve_local_link(raw),
                ),
            };
        }
        // Ignore 1-character schemes so Windows drive paths like `C:\...` stay local.
        if parsed.scheme().len() > 1 {
            return MarkdownLinkTarget::External(raw.to_owned());
        }
    }
    let (path_part, anchor) = match raw.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment.to_owned())),
        None => (raw, None),
    };
    let decoded = files::percent_decode(path_part);
    if decoded.is_empty() {
        return MarkdownLinkTarget::Invalid(crate::i18n::workspace::err_invalid_link(raw));
    }
    let candidate = if let Some(ws_rel) = decoded.strip_prefix('/')
        && !ws_rel.starts_with('/')
        && let Some(root) = workspace_root
    {
        let from_root = root.join(ws_rel);
        if from_root.exists() {
            from_root
        } else {
            source_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&decoded)
        }
    } else {
        let path = PathBuf::from(&decoded);
        if path.is_absolute() {
            path
        } else {
            source_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(path)
        }
    };
    MarkdownLinkTarget::Local {
        path: candidate,
        anchor,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_markdown_link, MarkdownLinkTarget};
    use std::path::PathBuf;

    #[test]
    fn classify_markdown_link_handles_http_relative_and_windows_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let source = docs.join("intro.md");
        std::fs::write(&source, "# Intro").unwrap();
        std::fs::write(root.join("guide.md"), "# Guide").unwrap();

        assert_eq!(
            classify_markdown_link(&source, Some(root), "https://example.com/a?b=1"),
            MarkdownLinkTarget::External("https://example.com/a?b=1".to_owned())
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), "mailto:test@example.com"),
            MarkdownLinkTarget::External("mailto:test@example.com".to_owned())
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), "../guide.md#快速%20开始"),
            MarkdownLinkTarget::Local {
                path: docs.join("../guide.md"),
                anchor: Some("快速%20开始".to_owned()),
            }
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), "/guide.md"),
            MarkdownLinkTarget::Local {
                path: root.join("guide.md"),
                anchor: None,
            }
        );
        assert_eq!(
            classify_markdown_link(&source, Some(root), r"C:\notes\my%20file.md"),
            MarkdownLinkTarget::Local {
                path: PathBuf::from(r"C:\notes\my file.md"),
                anchor: None,
            }
        );
    }
}
