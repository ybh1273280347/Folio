//! Workspace coordination for the global knowledge graph tab, right-panel local graph,
//! and in-graph append-only link creation.

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

    pub(super) fn toggle_local_graph_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_panel.open && self.right_panel.view == RightPanelView::Graph {
            self.close_right_panel(window, cx);
        } else {
            self.open_right_panel(RightPanelView::Graph, window, cx);
        }
    }

    pub(in crate::workspace) fn sync_graph_active_document(&mut self, path: &str, cx: &mut Context<Self>) {
        self.graph
            .update(cx, |pane, cx| pane.set_active_document(path, cx));
        self.local_graph
            .update(cx, |pane, cx| pane.set_active_document(path, cx));
    }

    pub(in crate::workspace) fn mark_graph_dirty(&mut self, cx: &mut Context<Self>) {
        self.graph_dirty = true;
        self.local_graph_dirty = true;
        let local_visible = self.right_panel.open && self.right_panel.view == RightPanelView::Graph;
        if self.graph_open || local_visible {
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
        let local_visible = self.right_panel.open && self.right_panel.view == RightPanelView::Graph;

        if self.graph_open || (reset_fit && !local_visible) {
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

        if local_visible {
            let result = self.local_graph.update(cx, |pane, cx| {
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
                Ok(()) => self.local_graph_dirty = false,
                Err(error) => self.report(error, cx),
            }
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
            GraphPaneEvent::OpenSourceLocation {
                source_rel,
                label,
                line,
                node_id,
                split,
            } => {
                self.open_graph_source_location(
                    source_rel,
                    label,
                    *line,
                    node_id.as_deref(),
                    *split,
                    window,
                    cx,
                );
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
            GraphPaneEvent::AppendLink {
                source_rel,
                target_rel,
                target_title,
            } => {
                self.append_markdown_link_from_graph(source_rel, target_rel, target_title, window, cx);
            }
            GraphPaneEvent::CopyLink {
                markdown_link,
                title,
            } => {
                cx.write_to_clipboard(ClipboardItem::new_string(markdown_link.clone()));
                self.report(crate::i18n::graph::copied_link_notice(title), cx);
            }
            GraphPaneEvent::RevealInTree { rel_path } => {
                self.reveal_file_in_tree(rel_path, cx);
            }
            GraphPaneEvent::RefreshRequested => {
                self.sync_project_markdown_links(cx);
                self.refresh_graph_data(false, cx);
            }
        }
    }

    fn append_markdown_link_from_graph(
        &mut self,
        source_rel: &str,
        target_rel: &str,
        target_title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = crate::markdown::links::format_root_link_url(target_rel);
        let entry = format!("- [{target_title}]({url})");
        let source_name = Path::new(source_rel)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| source_rel.to_owned());

        let append_entry = |current: &str| -> String {
            let mut next = current.to_owned();
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(&entry);
            next.push('\n');
            next
        };

        if let Some(buffer) = self.documents.open_buffer(source_rel) {
            let was_dirty = buffer.is_dirty();
            let next_text = append_entry(&buffer.text);
            self.edit_document(source_rel, next_text);
            self.sync_markdown_panes(source_rel, window, cx);
            if !was_dirty {
                match self.save_file(source_rel, false, cx) {
                    Ok(true) => {
                        self.refresh_graph_data(false, cx);
                        self.report(
                            crate::i18n::graph::appended_link_notice(&source_name, target_title, true),
                            cx,
                        );
                    }
                    Ok(false) => {
                        self.report(
                            crate::i18n::graph::appended_link_notice(&source_name, target_title, false),
                            cx,
                        );
                    }
                    Err(error) => self.report(error, cx),
                }
            } else {
                self.report(
                    crate::i18n::graph::appended_link_notice(&source_name, target_title, false),
                    cx,
                );
            }
            return;
        }

        let Some(project) = self.projects.directory() else {
            return;
        };
        let root = PathBuf::from(&project.folder);
        let disk = match files::read_document(&root, source_rel) {
            Ok(doc) => doc,
            Err(error) => {
                self.report(error, cx);
                return;
            }
        };
        let next_text = append_entry(&disk.text);
        match files::save_document(&root, source_rel, &next_text, &disk.revision) {
            Ok(Ok(saved)) => {
                self.sync_markdown_links_from_text(source_rel, &saved.text, cx);
                self.refresh_graph_data(false, cx);
                self.report(
                    crate::i18n::graph::appended_link_notice(&source_name, target_title, true),
                    cx,
                );
            }
            Ok(Err(_disk)) => {
                self.report(crate::i18n::graph::ERR_APPEND_CONFLICT.get(), cx);
            }
            Err(error) => self.report(error, cx),
        }
    }

    pub(super) fn render_local_graph_panel(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.settings.theme.palette;
        let chrome_bg = if palette.is_dark() {
            let b = gpui_kit::rgb(palette.background);
            let s = gpui_kit::rgb(palette.surface);
            Rgba {
                r: b.r + (s.r - b.r) * 0.35,
                g: b.g + (s.g - b.g) * 0.35,
                b: b.b + (s.b - b.b) * 0.35,
                a: 1.0,
            }
        } else {
            let s = gpui_kit::rgb(palette.surface);
            let h = gpui_kit::rgb(palette.hover);
            Rgba {
                r: s.r + (h.r - s.r) * 0.55,
                g: s.g + (h.g - s.g) * 0.55,
                b: s.b + (h.b - s.b) * 0.55,
                a: 1.0,
            }
        };
        let title_size = (f32::from(self.settings.interface_font_size) - 1.0).max(11.0);
        let active = self.documents.active();
        let header_title = if active.is_empty() {
            crate::i18n::graph::LOCAL_PANEL_TITLE.get().to_owned()
        } else {
            Path::new(active)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| crate::i18n::graph::LOCAL_PANEL_TITLE.get().to_owned())
        };
        let depth = self.local_graph.read(cx).focus_depth();
        let has_target = self.local_graph.read(cx).has_local_target();

        let header = div()
            .h(px(crate::ui::PANE_HEADER_HEIGHT))
            .px_2()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .bg(chrome_bg)
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        crate::ui::panel_toolbar_button("local-graph-back-to-hub", palette, cx)
                            .icon(crate::ui::icon(IconName::ChevronLeft).size(px(15.)))
                            .tooltip(crate::i18n::common::BACK_TO_HUB.get())
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.open_right_panel(RightPanelView::Hub, window, cx);
                            })),
                    )
                    .child(
                        crate::ui::icon(IconName::ChartNetwork)
                            .w(px(15.))
                            .h(px(15.))
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(title_size))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.text))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(header_title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .when(has_target, |row| {
                        row.child(
                            div()
                                .id("local-graph-hop-1")
                                .h(px(22.))
                                .px_1p5()
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .when(depth == 1, |el| {
                                    el.bg(gpui_kit::rgb(palette.selection))
                                        .font_medium()
                                        .text_color(gpui_kit::rgb(palette.text))
                                })
                                .when(depth != 1, |el| {
                                    el.text_color(gpui_kit::rgb(palette.muted))
                                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.local_graph.update(cx, |pane, cx| pane.set_focus_depth(1, cx));
                                }))
                                .child(crate::i18n::graph::HOP_1.get()),
                        )
                        .child(
                            div()
                                .id("local-graph-hop-2")
                                .h(px(22.))
                                .px_1p5()
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .when(depth == 2, |el| {
                                    el.bg(gpui_kit::rgb(palette.selection))
                                        .font_medium()
                                        .text_color(gpui_kit::rgb(palette.text))
                                })
                                .when(depth != 2, |el| {
                                    el.text_color(gpui_kit::rgb(palette.muted))
                                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.local_graph.update(cx, |pane, cx| pane.set_focus_depth(2, cx));
                                }))
                                .child(crate::i18n::graph::HOP_2.get()),
                        )
                    })
                    .child(
                        crate::ui::panel_toolbar_button("local-graph-refresh", palette, cx)
                            .icon(crate::ui::icon(IconName::RefreshCw).size(px(14.)))
                            .tooltip(crate::i18n::graph::TOOLTIP_REFRESH.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.sync_project_markdown_links(cx);
                                app.refresh_graph_data(false, cx);
                            })),
                    )
                    .child(
                        crate::ui::panel_toolbar_button("local-graph-open-global", palette, cx)
                            .icon(crate::ui::icon(IconName::ExternalLink).size(px(14.)))
                            .tooltip(crate::i18n::graph::TOOLTIP_EXPAND_GLOBAL_GRAPH.get())
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.open_graph(window, cx);
                            })),
                    )
                    .child(
                        crate::ui::panel_toolbar_button("local-graph-maximize", palette, cx)
                            .icon(crate::ui::icon(IconName::Maximize2).size(px(15.)))
                            .tooltip(crate::i18n::ai::MAXIMIZE_RESTORE.get())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.right_panel.maximized = !app.right_panel.maximized;
                                cx.notify();
                            })),
                    ),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.local_graph.clone()),
            )
            .into_any_element()
    }
}
