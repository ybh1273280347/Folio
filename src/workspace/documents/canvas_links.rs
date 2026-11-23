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
        let preview: String = quote.lines().next().unwrap_or("").chars().take(120).collect();
        let norm_source = source_path.replace('\\', "/");
        let anchor = KnowledgeAnchor::MarkdownQuote {
            path: norm_source.clone(),
            quote,
            occurrence,
            heading_path,
            line_hint,
        };
        let source_folder = Path::new(&norm_source)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|s| !s.is_empty());
        let default_stem = Path::new(&norm_source)
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("notes");
        self.dispatch_anchor_to_canvas(preview, anchor, source_folder, default_stem, window, cx);
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
        let rel_pdf = if let Some(project) = self.projects.directory()
            && let Ok(root) = dunce::canonicalize(&project.folder)
            && let Ok(canon_pdf) = dunce::canonicalize(pdf_full_path)
            && let Ok(rel) = canon_pdf.strip_prefix(&root)
        {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            pdf_full_path.to_string_lossy().into_owned()
        };
        let preview = if quote.trim().is_empty() {
            crate::i18n::canvas::pdf_page_annotation_preview(page_1based)
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
            path: rel_pdf.clone(),
            annotation_id,
            page: page_1based,
            quote,
            rects: norm_rects,
        };
        let source_folder = if !Path::new(&rel_pdf).is_absolute() {
            Path::new(&rel_pdf)
                .parent()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let default_stem = Path::new(&rel_pdf)
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("notes");
        self.dispatch_anchor_to_canvas(preview, anchor, source_folder, default_stem, window, cx);
    }

    fn dispatch_anchor_to_canvas(
        &mut self,
        preview: String,
        anchor: KnowledgeAnchor,
        source_folder: Option<String>,
        default_stem: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open_split) = self
            .documents
            .reading_panes
            .iter()
            .rev()
            .find(|p| Self::is_canvas_path(p))
            .cloned()
        {
            self.insert_anchor_into_canvas(&open_split, preview, anchor, window, cx);
            return;
        }

        let default_name = format!("{default_stem}.canvas");
        let name_input = cx.new(|cx| {
            let mut state = gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(crate::i18n::canvas::PICKER_NEW_PLACEHOLDER.get());
            state.set_value(default_name, window, cx);
            state.focus(window, cx);
            state
        });
        let submit_sub = cx.subscribe_in(
            &name_input,
            window,
            |app, _, event: &gpui_kit::component::input::InputEvent, window, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::PressEnter { .. }) {
                    app.create_and_insert_pending_canvas(window, cx);
                }
            },
        );
        self.documents.pending_canvas_insert = Some(super::PendingCanvasInsert {
            preview,
            anchor,
            source_folder,
            name_input,
            _submit_sub: submit_sub,
        });
        cx.notify();
    }

    fn insert_anchor_into_canvas(
        &mut self,
        target_canvas: &str,
        preview: String,
        anchor: KnowledgeAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let full = self.document_path(target_canvas);
        let file = ProjectFile {
            kind: Self::file_kind(&full),
            path: target_canvas.to_owned(),
        };
        let target_key = if self.documents.tabs.contains(&file.path)
            && self.documents.active == file.path
        {
            PaneKey::tab(&file.path)
        } else {
            self.open_reading_pane(&file, window, cx);
            PaneKey::reading(&file.path)
        };
        let Some(canvas_pane) = self.documents.canvas_panes.get(&target_key).cloned() else {
            return;
        };
        canvas_pane.update(cx, |pane, cx| {
            pane.add_anchor_node(preview, anchor, cx);
        });
        if let Some(doc) = self.documents.open.get(&file.path) {
            let text = doc.buffer.text.clone();
            self.sync_canvas_index_from_text(&file.path, &text);
        }
        let canvas_name = Path::new(&file.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.path.clone());
        self.report(crate::i18n::canvas::toast_added_to_canvas(&canvas_name), cx);
    }

    pub(in crate::workspace) fn create_and_insert_pending_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.documents.pending_canvas_insert.as_ref() else {
            return;
        };
        let raw_name = pending.name_input.read(cx).value().to_string();
        let source_folder = pending.source_folder.clone();
        let Some(project) = self.projects.directory() else {
            return;
        };
        let root = PathBuf::from(&project.folder);
        let created = match files::create_named_canvas_in(&root, source_folder.as_deref(), &raw_name)
        {
            Ok(file) => file,
            Err(err) => {
                self.report(err.to_string(), cx);
                return;
            }
        };
        let _ = self.tree.reload_files(&root);
        let Some(pending) = self.documents.pending_canvas_insert.take() else {
            return;
        };
        self.insert_anchor_into_canvas(&created.path, pending.preview, pending.anchor, window, cx);
        cx.notify();
    }

    pub(in crate::workspace) fn render_canvas_target_picker(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pending = self.documents.pending_canvas_insert.as_ref()?;
        let palette = crate::theme::palette();
        let source_folder = pending.source_folder.as_deref();

        let mut canvases: Vec<(String, bool, bool)> = self
            .tree
            .files()
            .iter()
            .filter(|f| Self::is_canvas_path(&f.path))
            .map(|f| {
                let is_open = self.documents.tabs.contains(&f.path);
                let same_dir = source_folder.is_some_and(|dir| {
                    Path::new(&f.path)
                        .parent()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .as_deref()
                        == Some(dir)
                });
                (f.path.clone(), is_open, same_dir)
            })
            .collect();
        canvases.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });

        let mut list = div()
            .id("canvas-picker-list")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(220.))
            .overflow_y_scroll();

        if canvases.is_empty() {
            list = list.child(
                div()
                    .px_2p5()
                    .py_3()
                    .text_xs()
                    .text_color(rgb(palette.muted))
                    .child(crate::i18n::canvas::PICKER_EMPTY_LIST.get()),
            );
        } else {
            for (idx, (path, is_open, _)) in canvases.into_iter().enumerate() {
                let target_path = path.clone();
                list = list.child(
                    div()
                        .id(("canvas-picker-item", idx))
                        .px_2p5()
                        .py_1p5()
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.hover)))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            if let Some(pending) = app.documents.pending_canvas_insert.take() {
                                app.insert_anchor_into_canvas(
                                    &target_path,
                                    pending.preview,
                                    pending.anchor,
                                    window,
                                    cx,
                                );
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    crate::ui::icon(IconName::LayoutGrid)
                                        .size(px(14.))
                                        .text_color(rgb(palette.accent)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .text_xs()
                                        .text_color(rgb(palette.text))
                                        .child(path),
                                ),
                        )
                        .when(is_open, |row| {
                            row.child(
                                div()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(rgb(palette.selection))
                                    .text_xs()
                                    .text_color(rgb(palette.accent))
                                    .child(crate::i18n::canvas::PICKER_OPEN_TAB_BADGE.get()),
                            )
                        }),
                );
            }
        }

        Some(
            div()
                .id("canvas-target-picker-backdrop")
                .absolute()
                .inset_0()
                .bg(rgba(0x0000004d))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|app, _, _, cx| {
                    app.documents.pending_canvas_insert = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("canvas-target-picker-card")
                        .w(px(420.))
                        .p_4()
                        .rounded(px(10.))
                        .bg(rgb(palette.surface))
                        .border_1()
                        .border_color(rgb(palette.border))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette.text))
                                                .child(crate::i18n::canvas::PICKER_TITLE.get()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette.muted))
                                                .child(crate::i18n::canvas::PICKER_SUBTITLE.get()),
                                        ),
                                )
                                .child(
                                    Button::new("canvas-picker-close")
                                        .ghost()
                                        .compact()
                                        .h(px(22.))
                                        .w(px(22.))
                                        .icon(crate::ui::icon(IconName::Close))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.documents.pending_canvas_insert = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(list)
                        .child(
                            div()
                                .pt_2()
                                .border_t_1()
                                .border_color(rgb(palette.border))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        gpui_kit::component::input::Input::new(&pending.name_input),
                                    ),
                                )
                                .child(
                                    crate::ui::button_primary(
                                        "canvas-picker-create",
                                        crate::i18n::canvas::PICKER_BTN_CREATE_AND_ADD.get(),
                                    )
                                    .h(px(28.))
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.create_and_insert_pending_canvas(window, cx);
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
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
