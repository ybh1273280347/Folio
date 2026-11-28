//! Workspace coordination for the native knowledge graph virtual tab.

use super::*;
use crate::graph::GraphPaneEvent;

impl Workspace {
    pub(super) fn open_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.trash.dismiss();
        self.graph_open = true;
        let active = self.documents.active().to_owned();
        self.graph.update(cx, |pane, cx| {
            pane.set_active_document(&active, cx);
            pane.focus(window, cx);
        });
        if self.graph_dirty {
            self.refresh_graph_data(true, cx);
        }
        cx.notify();
    }

    pub(super) fn close_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.graph_open {
            return;
        }
        self.graph_open = false;
        if self.promote_first_reading_pane_if_tabs_empty(true, window, cx) {
            if let Err(error) = self.remember_session() {
                self.report(error, cx);
            }
        } else {
            self.focus_active_or_workspace(window, cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.graph_open {
            self.close_graph(window, cx);
        } else {
            self.open_graph(window, cx);
        }
    }

    pub(in crate::workspace) fn mark_graph_dirty(&mut self, cx: &mut Context<Self>) {
        self.graph_dirty = true;
        if self.graph_open {
            self.refresh_graph_data(false, cx);
        }
    }

    pub(super) fn refresh_graph_data(&mut self, reset_fit: bool, cx: &mut Context<Self>) {
        self.sync_project_canvas_links(cx);
        let db_path = self.canvas_metadata_db_path();
        let project_files = self.tree.files().to_vec();
        let tag_snapshot = self.tags.snapshot.clone();
        let loose_paths = if self.projects.directory().is_none() {
            self.documents.tabs().to_vec()
        } else {
            Vec::new()
        };
        let result = self.graph.update(cx, |pane, cx| {
            pane.load_data(
                db_path.as_deref(),
                &project_files,
                &tag_snapshot,
                &loose_paths,
                reset_fit,
                cx,
            )
        });
        match result {
            Ok(()) => self.graph_dirty = false,
            Err(error) => self.report(error, cx),
        }
    }

    pub(super) fn handle_graph_event(
        &mut self,
        event: &GraphPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            GraphPaneEvent::OpenFile { rel_path, split } => {
                let full = self.document_path(rel_path);
                if !full.is_file() {
                    self.report(crate::i18n::workspace::err_file_not_found(&full), cx);
                    self.refresh_graph_data(false, cx);
                    return;
                }
                let file = ProjectFile {
                    kind: Self::file_kind(&full),
                    path: rel_path.clone(),
                };
                if *split && Self::supports_split_pane(&file.path) {
                    self.open_reading_pane(&file, window, cx);
                } else if let Err(error) = self.open_file(&file, window, cx) {
                    self.report(error, cx);
                }
            }
            GraphPaneEvent::CreateMarkdown { rel_path } => {
                let Some(project) = self.projects.directory() else {
                    return;
                };
                let root = PathBuf::from(&project.folder);
                let normalized = rel_path.trim().replace('\\', "/");
                if normalized.is_empty()
                    || Path::new(&normalized)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)))
                {
                    return;
                }
                let target_rel = if normalized.to_ascii_lowercase().ends_with(".md") {
                    normalized
                } else {
                    format!("{normalized}.md")
                };
                let full = root.join(&target_rel);
                if !full.exists() {
                    if let Some(parent) = full.parent()
                        && let Err(error) = std::fs::create_dir_all(parent)
                    {
                        self.report(error.to_string(), cx);
                        return;
                    }
                    let title = full
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Untitled".to_owned());
                    if let Err(error) = std::fs::write(&full, format!("# {title}\n\n")) {
                        self.report(error.to_string(), cx);
                        return;
                    }
                }
                if let Ok(files) = files::list_files(&root) {
                    let folders = files::list_folders(&root).unwrap_or_default();
                    self.tree.replace_listing(files, folders);
                }
                self.expand_ancestors(&target_rel);
                self.sync_project_markdown_links(cx);
                let file = ProjectFile {
                    kind: Self::file_kind(&full),
                    path: target_rel,
                };
                if let Err(error) = self.open_file(&file, window, cx) {
                    self.report(error, cx);
                }
            }
            GraphPaneEvent::RefreshRequested => {
                self.sync_project_markdown_links(cx);
                self.refresh_graph_data(false, cx);
            }
        }
    }
}
