//! Bidirectional navigation, backlink indexing, and path relocation for `.canvas` files.

use super::*;
use crate::canvas::{
    CanvasBacklink, KnowledgeAnchor, index as canvas_index, model::CanvasDocument,
};

impl Workspace {
    pub(in crate::workspace) fn canvas_metadata_db_path(&self) -> Option<PathBuf> {
        let root = self
            .projects
            .directory()
            .map(|project| PathBuf::from(&project.folder));
        canvas_index::metadata_db_path(root.as_deref()).ok()
    }

    pub(in crate::workspace) fn sync_canvas_index_from_text(&mut self, rel_path: &str, text: &str) {
        if !Self::is_canvas_path(rel_path) {
            return;
        }
        let Some(db_path) = self.canvas_metadata_db_path() else {
            return;
        };
        if let Ok(doc) = CanvasDocument::from_json_str(text) {
            let _ = canvas_index::sync_canvas_links(&db_path, rel_path, &doc);
        }
        self.refresh_active_canvas_backlinks();
    }

    pub(in crate::workspace) fn refresh_active_canvas_backlinks(&mut self) {
        if self.documents.active.is_empty() || Self::is_canvas_path(&self.documents.active) {
            self.documents.active_canvas_backlinks.clear();
            return;
        }
        let Some(db_path) = self.canvas_metadata_db_path() else {
            self.documents.active_canvas_backlinks.clear();
            return;
        };
        self.documents.active_canvas_backlinks =
            canvas_index::backlinks_for(&db_path, &self.documents.active);
    }

    pub(in crate::workspace) fn open_canvas_backlink(
        &mut self,
        backlink: &CanvasBacklink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let full = self.document_path(&backlink.canvas_path);
        if !full.is_file() {
            self.report(crate::i18n::workspace::err_file_not_found(&full), cx);
            return;
        }
        let file = ProjectFile {
            kind: Self::file_kind(&full),
            path: backlink.canvas_path.clone(),
        };
        if let Err(error) = self.open_file(&file, window, cx) {
            self.report(error, cx);
            return;
        }
        if let Some(pane) = self
            .documents
            .canvas_panes
            .get(&PaneKey::tab(&backlink.canvas_path))
            .cloned()
        {
            let node_id = backlink.node_id.clone();
            pane.update(cx, |pane, cx| pane.focus_node(&node_id, window, cx));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn send_markdown_selection_to_canvas(
        &mut self,
        source_path: &str,
        quote: String,
        occurrence: usize,
        heading_path: Vec<String>,
        line_hint: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_canvas) = self.resolve_or_create_target_canvas(cx) else {
            self.report("无法确定或创建目标画布文件。", cx);
            return;
        };
        let full = self.document_path(&target_canvas);
        let file = ProjectFile {
            kind: Self::file_kind(&full),
            path: target_canvas.clone(),
        };

        // Prefer opening the target canvas in the right split pane when sent from a note.
        let target_key = if self.documents.tabs.contains(&target_canvas)
            && self.documents.active == target_canvas
        {
            PaneKey::tab(&target_canvas)
        } else {
            self.open_reading_pane(&file, window, cx);
            PaneKey::reading(&target_canvas)
        };

        let Some(canvas_pane) = self.documents.canvas_panes.get(&target_key).cloned() else {
            return;
        };

        let preview: String = quote.lines().next().unwrap_or("").chars().take(120).collect();
        let anchor = KnowledgeAnchor::MarkdownQuote {
            path: source_path.replace('\\', "/"),
            quote,
            occurrence,
            heading_path,
            line_hint,
        };
        canvas_pane.update(cx, |pane, cx| {
            pane.add_anchor_node(preview, anchor, cx);
        });
        if let Some(doc) = self.documents.open.get(&target_canvas) {
            let text = doc.buffer.text.clone();
            self.sync_canvas_index_from_text(&target_canvas, &text);
        }
        let canvas_name = Path::new(&target_canvas)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(target_canvas);
        self.report(format!("已添加到画布：{canvas_name}"), cx);
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn send_pdf_annotation_to_canvas(
        &mut self,
        pdf_full_path: &Path,
        annotation_id: u64,
        page_1based: usize,
        quote: String,
        rects: &[crate::annotations::AnnotationRect],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_canvas) = self.resolve_or_create_target_canvas(cx) else {
            self.report("无法确定或创建目标画布文件。", cx);
            return;
        };
        let rel_pdf = if let Some(project) = self.projects.directory()
            && let Ok(root) = dunce::canonicalize(&project.folder)
            && let Ok(canon_pdf) = dunce::canonicalize(pdf_full_path)
            && let Ok(rel) = canon_pdf.strip_prefix(&root)
        {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            pdf_full_path.to_string_lossy().into_owned()
        };
        let full = self.document_path(&target_canvas);
        let file = ProjectFile {
            kind: Self::file_kind(&full),
            path: target_canvas.clone(),
        };
        let target_key = if self.documents.tabs.contains(&target_canvas)
            && self.documents.active == target_canvas
        {
            PaneKey::tab(&target_canvas)
        } else {
            self.open_reading_pane(&file, window, cx);
            PaneKey::reading(&target_canvas)
        };
        let Some(canvas_pane) = self.documents.canvas_panes.get(&target_key).cloned() else {
            return;
        };
        let preview = if quote.trim().is_empty() {
            format!("PDF 第 {page_1based} 页批注")
        } else {
            quote.lines().next().unwrap_or("").chars().take(120).collect()
        };
        let norm_rects = rects
            .iter()
            .map(|r| crate::canvas::anchor::NormalizedRect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
            })
            .collect();
        let anchor = KnowledgeAnchor::PdfAnnotation {
            path: rel_pdf,
            annotation_id,
            page: page_1based,
            quote,
            rects: norm_rects,
        };
        canvas_pane.update(cx, |pane, cx| {
            pane.add_anchor_node(preview, anchor, cx);
        });
        if let Some(doc) = self.documents.open.get(&target_canvas) {
            let text = doc.buffer.text.clone();
            self.sync_canvas_index_from_text(&target_canvas, &text);
        }
        let canvas_name = Path::new(&target_canvas)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(target_canvas);
        self.report(format!("已添加到画布：{canvas_name}"), cx);
    }

    fn resolve_or_create_target_canvas(&mut self, cx: &mut Context<Self>) -> Option<String> {
        if let Some(reading) = self
            .documents
            .reading_panes
            .iter()
            .rev()
            .find(|p| Self::is_canvas_path(p))
        {
            return Some(reading.clone());
        }
        if let Some(tab) = self
            .documents
            .tabs
            .iter()
            .rev()
            .find(|p| Self::is_canvas_path(p))
        {
            return Some(tab.clone());
        }
        if let Some(existing) = self
            .tree
            .files()
            .iter()
            .find(|f| Self::is_canvas_path(&f.path))
        {
            return Some(existing.path.clone());
        }
        let project = self.projects.directory()?;
        let root = PathBuf::from(&project.folder);
        let created = files::create_canvas_in(&root, None).ok()?;
        let _ = self.tree.reload_files(&root);
        cx.notify();
        Some(created.path)
    }

    pub(in crate::workspace) fn jump_from_canvas_anchor(
        &mut self,
        canvas_full_path: &Path,
        anchor: Option<&KnowledgeAnchor>,
        fallback_file: Option<&str>,
        fallback_subpath: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(raw_target) = anchor
            .map(KnowledgeAnchor::target_path)
            .or(fallback_file)
            .filter(|p| !p.trim().is_empty())
        else {
            return;
        };

        let workspace_root = self
            .projects
            .directory()
            .map(|project| PathBuf::from(&project.folder));
        let candidate = {
            let p = Path::new(raw_target);
            if p.is_absolute() {
                p.to_path_buf()
            } else if let Some(root) = &workspace_root {
                let from_root = root.join(raw_target.replace('/', "\\"));
                if from_root.exists() {
                    from_root
                } else {
                    canvas_full_path
                        .parent()
                        .unwrap_or(root)
                        .join(raw_target.replace('/', "\\"))
                }
            } else {
                canvas_full_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(raw_target.replace('/', "\\"))
            }
        };

        let Ok(resolved) = dunce::canonicalize(&candidate) else {
            self.report(crate::i18n::workspace::err_link_target_not_found(&candidate), cx);
            return;
        };
        if !resolved.is_file() || !files::is_relevant(&resolved) {
            self.report(crate::i18n::workspace::err_unsupported_open(&resolved), cx);
            return;
        }

        let file = if let Some(root) = &workspace_root
            && let Ok(canonical_root) = dunce::canonicalize(root)
            && let Ok(relative) = resolved.strip_prefix(&canonical_root)
        {
            ProjectFile {
                kind: Self::file_kind(&resolved),
                path: relative.to_string_lossy().replace('\\', "/"),
            }
        } else {
            match self.prepare_standalone_file(&resolved) {
                Ok(f) => f,
                Err(error) => {
                    self.report(error, cx);
                    return;
                }
            }
        };

        // Determine target pane key: open in reading pane if settings prefer split pane and supported.
        let use_split = self.settings.internal_link_open_target
            == crate::model::InternalLinkOpenTarget::SplitPane
            && Self::supports_split_pane(&file.path);
        let pane_key = if use_split {
            self.open_reading_pane(&file, window, cx);
            PaneKey::reading(&file.path)
        } else {
            if let Err(error) = self.open_file(&file, window, cx) {
                self.report(error, cx);
                return;
            }
            PaneKey::tab(&file.path)
        };

        match anchor {
            Some(KnowledgeAnchor::MarkdownFile { .. }) => {}
            Some(KnowledgeAnchor::MarkdownHeading {
                heading,
                heading_path,
                line_hint,
                ..
            }) => {
                let target_heading = if !heading.trim().is_empty() {
                    Some(heading.as_str())
                } else {
                    heading_path.last().map(String::as_str)
                };
                if let Some(md) = self.documents.markdown_panes.get(&pane_key).cloned() {
                    md.update(cx, |pane, cx| {
                        if let Some(h) = target_heading {
                            pane.navigate_to_anchor(h, window, cx);
                        } else if let Some(line) = line_hint {
                            pane.jump_to_search_hit(Some(*line), None, None, window, cx);
                        }
                    });
                }
            }
            Some(KnowledgeAnchor::MarkdownQuote {
                quote,
                occurrence,
                heading_path,
                line_hint,
                ..
            }) => {
                let fallback_heading = heading_path.last().map(String::as_str);
                if let Some(md) = self.documents.markdown_panes.get(&pane_key).cloned() {
                    let exact = md.update(cx, |pane, cx| {
                        pane.jump_to_canvas_quote(
                            quote,
                            *occurrence,
                            fallback_heading,
                            *line_hint,
                            window,
                            cx,
                        )
                    });
                    if !exact {
                        self.report("原文片段已变更，已定位到所在章节。", cx);
                    }
                }
            }
            Some(KnowledgeAnchor::PdfPage { page, .. })
            | Some(KnowledgeAnchor::PdfAnnotation { page, .. }) => {
                if let Some(pdf) = self.documents.pdf_panes.get(&pane_key).cloned() {
                    pdf.update(cx, |pane, cx| pane.jump_to_page(*page, cx));
                }
            }
            Some(KnowledgeAnchor::KbChunk {
                line_start,
                page,
                heading,
                snippet,
                ..
            }) => {
                if let Some(page) = page
                    && let Some(pdf) = self.documents.pdf_panes.get(&pane_key).cloned()
                {
                    pdf.update(cx, |pane, cx| pane.jump_to_page(*page, cx));
                } else if let Some(md) = self.documents.markdown_panes.get(&pane_key).cloned() {
                    let clean_heading = heading
                        .as_deref()
                        .and_then(|h| h.rsplit(" > ").next())
                        .map(crate::rag::pipeline::clean_heading_text);
                    md.update(cx, |pane, cx| {
                        pane.jump_to_search_hit(
                            *line_start,
                            clean_heading.as_deref(),
                            snippet.as_deref(),
                            window,
                            cx,
                        );
                    });
                }
            }
            None => {
                if let Some(sub) = fallback_subpath.map(|s| s.trim_start_matches('#')) {
                    if let Some(page_num) = sub
                        .strip_prefix("page=")
                        .and_then(|n| n.parse::<usize>().ok())
                        && let Some(pdf) = self.documents.pdf_panes.get(&pane_key).cloned()
                    {
                        pdf.update(cx, |pane, cx| pane.jump_to_page(page_num, cx));
                    } else if let Some(md) = self.documents.markdown_panes.get(&pane_key).cloned() {
                        md.update(cx, |pane, cx| pane.navigate_to_anchor(sub, window, cx));
                    }
                }
            }
        }
        self.refresh_active_canvas_backlinks();
        cx.notify();
    }

    pub(in crate::workspace) fn relocate_canvas_links(
        &mut self,
        moves: &[(String, String)],
        is_folder: bool,
        cx: &mut Context<Self>,
    ) {
        if moves.is_empty() {
            return;
        }
        let mut updated_open = std::collections::HashSet::new();
        for (key, pane) in &self.documents.canvas_panes {
            let changed = pane.update(cx, |pane, cx| pane.relocate_paths(moves, is_folder, cx));
            if changed {
                updated_open.insert(key.path.clone());
            }
        }
        if let Some(project) = self.projects.directory() {
            let root = PathBuf::from(&project.folder);
            let canvas_files: Vec<String> = self
                .tree
                .files()
                .iter()
                .filter(|f| Self::is_canvas_path(&f.path) && !updated_open.contains(&f.path))
                .map(|f| f.path.clone())
                .collect();
            for rel_path in canvas_files {
                if let Ok(disk) = files::read_document(&root, &rel_path)
                    && let Ok(mut doc) = CanvasDocument::from_json_str(&disk.text)
                    && doc.relocate_paths(moves, is_folder)
                    && let Ok(next_text) = doc.to_json_pretty()
                {
                    let _ = files::save_document(&root, &rel_path, &next_text, &disk.revision);
                }
            }
        }
        if let Some(db_path) = self.canvas_metadata_db_path() {
            let _ = canvas_index::relocate_index_paths(&db_path, moves, is_folder);
        }
        self.refresh_active_canvas_backlinks();
    }
}
