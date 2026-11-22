//! Tag controls and the active workspace's read-only sidebar projection.

use super::*;
use crate::tags::{self, Change, Snapshot, Store};
use crate::ui::TAG_COLORS as COLORS;
use gpui_kit::component::tooltip::Tooltip;

pub(super) struct TagPanel {
    pub snapshot: Snapshot,
    pub filter: Option<String>,
    input: Entity<InputState>,
    open: bool,
    mode: Mode,
    scan: Option<Task<()>>,
}

#[derive(Default)]
enum Mode {
    #[default]
    Browse,
    Edit {
        paths: Vec<String>,
        folder: Option<String>,
    },
    Rename(String),
    Colors(String),
    Recover(String),
}

impl TagPanel {
    pub(super) fn is_editing(&self) -> bool {
        self.open && matches!(self.mode, Mode::Edit { .. })
    }

    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::common::SEARCH_ELLIPSIS.get()));
        cx.subscribe_in(&input, window, |app, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                app.submit_tag_input(window, cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            snapshot: Snapshot::default(),
            filter: None,
            input,
            open: false,
            mode: Mode::Browse,
            scan: None,
        }
    }

    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::common::SEARCH_ELLIPSIS.get(), window, cx);
        });
    }

    pub(super) fn reset(&mut self) {
        self.scan = None;
        self.snapshot = Snapshot::default();
        self.filter = None;
        self.open = false;
        self.mode = Mode::Browse;
    }
}

fn tag_control(id: impl Into<ElementId>, icon: IconName, title: &'static str) -> Stateful<Div> {
    crate::ui::control(id, icon)
        .size(px(23.))
        .tooltip(move |window, cx| Tooltip::new(title).build(window, cx))
}

impl Workspace {
    fn tag_store(&self) -> io::Result<Store> {
        match self.projects.directory() {
            Some(project) => Ok(Store::project(Path::new(&project.folder))),
            None => Store::loose(),
        }
    }

    /// Only the latest scan for this workspace can publish or write back.
    /// Dropped GPUI tasks may finish probing, but they cannot change the store.
    pub(super) fn refresh_tags(&mut self, cx: &mut Context<Self>) {
        let store = match self.tag_store() {
            Ok(store) => store,
            Err(error) => {
                self.report(crate::i18n::tags::err_load_tags(&error), cx);
                return;
            }
        };
        let visible = if self.projects.directory().is_some() {
            self.tree.files()
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
        } else {
            self.documents.tabs()
                .iter()
                .chain(self.documents.reading_panes())
                .cloned()
                .collect()
        };
        let source = store.clone();
        let scan = cx.background_spawn(async move { source.scan(&visible) });
        self.tags.scan = Some(cx.spawn(async move |this, cx| {
            let result = scan.await;
            let _ = this.update(cx, |app, cx| {
                match result.and_then(|scan| store.apply_scan(scan)) {
                    Ok(snapshot) => app.use_tag_snapshot(snapshot, cx),
                    Err(error) => app.report(crate::i18n::tags::err_refresh_tags(&error), cx),
                }
            });
        }));
    }

    pub(super) fn relocate_tags(
        &mut self,
        moves: &[(String, String)],
        folder: bool,
        cx: &mut Context<Self>,
    ) {
        if moves.is_empty() {
            return;
        }
        self.tags.scan = None;
        match self
            .tag_store()
            .map_err(Into::into)
            .and_then(|store| store.relocate(moves, folder))
        {
            Ok(snapshot) => self.use_tag_snapshot(snapshot, cx),
            Err(error) => {
                self.report(crate::i18n::tags::err_moved_tags_not_updated(&error), cx);
                self.refresh_tags(cx);
            }
        }
    }

    pub(super) fn use_tag_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.tags.snapshot = snapshot;
        if self.projects.directory().is_none() {
            // Tagged loose files remain discoverable after closing their tabs.
            let paths: Vec<_> = self
                .tags
                .snapshot
                .assigned
                .keys()
                .chain(self.tags.snapshot.missing.iter().map(|(_, path)| path))
                .map(PathBuf::from)
                .collect();
            for path in paths {
                let parent = path.parent().expect("tagged loose file has a parent");
                if parent.is_dir()
                    && let Err(error) = self.watch_standalone_parent(parent)
                {
                    self.report(crate::i18n::tags::err_watch_loose_dir(&error), cx);
                }
            }
        }
        if let Some(filter) = &self.tags.filter {
            if !self.tags.snapshot.tags.iter().any(|tag| &tag.id == filter) {
                self.tags.filter = None;
            }
            self.prune_tag_selection(cx);
        }
        if let Mode::Recover(id) = &self.tags.mode
            && !self
                .tags
                .snapshot
                .missing
                .iter()
                .any(|(missing, _)| missing == id)
        {
            self.tags.mode = Mode::Browse;
        }
        cx.notify();
    }

    pub(super) fn change_tags(&mut self, change: Change<'_>, cx: &mut Context<Self>) -> bool {
        match self
            .tag_store()
            .map_err(Into::into)
            .and_then(|store| store.change(change))
        {
            Ok(snapshot) => {
                self.use_tag_snapshot(snapshot, cx);
                true
            }
            Err(error) => {
                self.report(crate::i18n::tags::err_save_tags(&error), cx);
                false
            }
        }
    }

    pub(super) fn tag_matches(&self, path: &str) -> bool {
        self.tags
            .filter
            .as_deref()
            .is_none_or(|tag| self.tags.snapshot.has(path, tag))
    }

    fn select_tag(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        self.tags.filter = filter;
        self.apply_tree_filter(cx);
        cx.notify();
    }

    fn show_tag_panel(
        &mut self,
        mode: Mode,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = !matches!(mode, Mode::Colors(_));
        self.tags.open = true;
        self.tags.mode = mode;
        self.tags.input.update(cx, |input, cx| {
            input.set_value(value.to_owned(), window, cx)
        });
        if !self.tags.is_editing() {
            self.tree.scroll_to_tags();
        }
        if focus {
            let input = self.tags.input.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |input, cx| input.focus(window, cx))
            });
        }
        cx.notify();
    }

    pub(super) fn edit_file_tags(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !paths.is_empty() {
            self.show_tag_panel(
                Mode::Edit {
                    paths,
                    folder: None,
                },
                "",
                window,
                cx,
            );
        }
    }

    pub(super) fn edit_folder_tags(
        &mut self,
        folder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<_> = self
            .tree.files()
            .iter()
            .filter(|file| Path::new(&file.path).starts_with(folder))
            .map(|file| file.path.clone())
            .collect();
        if paths.is_empty() {
            self.report(crate::i18n::tags::ERR_NO_SUPPORTED_FILES_IN_FOLDER.get(), cx);
            return;
        }
        self.show_tag_panel(
            Mode::Edit {
                paths,
                folder: Some(folder.to_owned()),
            },
            "",
            window,
            cx,
        );
    }

    pub(super) fn edit_tags_from_tree(
        &mut self,
        _: &EditTags,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tree.selected_files().is_empty() {
            self.edit_file_tags(self.tree.selected_files().iter().cloned().collect(), window, cx);
        } else if let Some(folder) = self.tree.focused_folder().map(str::to_owned) {
            self.edit_folder_tags(&folder, window, cx);
        } else if self.documents.tabs().iter().any(|tab| tab == self.documents.active())
            || self.tree.files().iter().any(|file| file.path == self.documents.active())
        {
            self.edit_file_tags(vec![self.documents.active().to_owned()], window, cx);
        }
    }

    pub(super) fn close_tag_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tags.is_editing() {
            self.tree.focus(window, cx);
        }
        self.tags.open = false;
        cx.notify();
    }

    fn submit_tag_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tags.open {
            return;
        }
        let value = self.tags.input.read(cx).value().trim().to_owned();
        if value.is_empty() {
            return;
        }
        let (change, rename) = match &self.tags.mode {
            Mode::Browse => (
                Change::Create {
                    name: &value,
                    paths: &[],
                },
                false,
            ),
            Mode::Edit { paths, .. } => (
                Change::Create {
                    name: &value,
                    paths,
                },
                false,
            ),
            Mode::Rename(tag) => (Change::Rename { tag, name: &value }, true),
            Mode::Colors(_) | Mode::Recover(_) => return,
        };
        // The mode contains the files selected when the editor was opened.
        let store = self
            .tag_store()
            .map_err(Into::into)
            .and_then(|store| store.change(change));
        match store {
            Ok(snapshot) => {
                self.use_tag_snapshot(snapshot, cx);
                if rename {
                    self.tags.mode = Mode::Browse;
                }
                self.tags
                    .input
                    .update(cx, |input, cx| input.set_value(String::new(), window, cx));
            }
            Err(error) => self.report(crate::i18n::tags::err_save_tags(&error), cx),
        }
    }

    fn delete_tag(
        &mut self,
        id: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let retry_id = id.clone();
        if self.ask(
            Dialog::DeleteTag,
            crate::i18n::tags::dialog_delete_tag_title(&name),
            crate::i18n::tags::DIALOG_DELETE_TAG_BODY.get().into(),
            DialogButtons::Confirm(crate::i18n::tags::DIALOG_DELETE_TAG_CONFIRM.get().into()),
            cx,
            move |app, _, cx| app.delete_tag(retry_id, name, cx),
        ) == Some(rfd::MessageDialogResult::Ok)
        {
            self.change_tags(Change::Delete(&id), cx);
        }
    }

    pub(super) fn render_tag_toggle(&self, cx: &Context<Self>) -> Stateful<Div> {
        tag_control("tags-toggle", IconName::Tag, crate::i18n::tags::TAGS_TITLE.get())
            .when(self.tags.open, |control| control.bg(rgb(0xe8e7e4)))
            .on_click(cx.listener(|app, _, window, cx| {
                if app.tags.open {
                    app.tags.open = false;
                    cx.notify();
                } else {
                    app.show_tag_panel(Mode::Browse, "", window, cx);
                }
            }))
    }

    pub(super) fn render_tag_filter(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let filter = self.tags.filter.as_ref()?;
        let tag = self
            .tags
            .snapshot
            .tags
            .iter()
            .find(|tag| &tag.id == filter)?;
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mx_2()
                .mb_1()
                .px_2()
                .rounded(px(5.))
                .text_xs()
                .bg(rgb(0xefeeeb))
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .child(crate::i18n::tags::filter_tag(&tag.name)),
                )
                .child(
                    tag_control("clear-tag-filter", IconName::Close, crate::i18n::tags::CLEAR_TAG_FILTER.get())
                        .on_click(cx.listener(|app, _, _, cx| app.select_tag(None, cx))),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_file_tags(
        &self,
        path: &str,
        depth: usize,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let ids = self.tags.snapshot.assigned.get(path)?;
        let tags: Vec<_> = ids
            .iter()
            .filter_map(|id| self.tags.snapshot.tags.iter().find(|tag| &tag.id == id))
            .collect();
        let full = SharedString::from(
            tags.iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>()
                .join("、"),
        );
        let file = path.to_owned();
        let width = self.settings.sidebar_width - depth as f32 * 15.;
        if width < 205. {
            return Some(
                crate::ui::control(
                    SharedString::from(format!("file-tags-{path}")),
                    IconName::Tag,
                )
                .size(px(20.))
                .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                .on_click(cx.listener(move |app, _, window, cx| {
                    cx.stop_propagation();
                    app.edit_file_tags(vec![file.clone()], window, cx);
                }))
                .into_any_element(),
            );
        }
        let (limit, max_chip_width) = if width >= 305. { (2, 56.) } else { (1, 42.) };
        let overflow = tags.len().saturating_sub(limit);
        let mut chips = div()
            .id(SharedString::from(format!("file-tags-{path}")))
            .flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .text_xs();
        for tag in tags.iter().take(limit) {
            let id = tag.id.clone();
            let (_, bg, fg) = COLORS.get(tag.color as usize).copied().unwrap_or(COLORS[0]);
            let title = SharedString::from(tag.name.clone());
            chips = chips.child(
                div()
                    .id(SharedString::from(id.clone()))
                    .cursor_pointer()
                    .max_w(px(max_chip_width))
                    .px_1p5()
                    .rounded(px(8.))
                    .bg(rgb(bg))
                    .text_color(rgb(fg))
                    .text_ellipsis()
                    .hover(|style| style.opacity(0.8))
                    .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
                    .child(tag.name.clone())
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.select_tag(Some(id.clone()), cx);
                    })),
            );
        }
        if overflow > 0 {
            chips = chips.child(
                div()
                    .id("more-tags")
                    .px_1()
                    .cursor_pointer()
                    .rounded(px(6.))
                    .bg(rgb(0xefeeeb))
                    .text_color(rgb(MUTED))
                    .child(format!("+{overflow}"))
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.edit_file_tags(vec![file.clone()], window, cx);
                    })),
            );
        }
        Some(chips.into_any_element())
    }

    pub(super) fn render_tag_editor(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000030))
            .occlude()
            .child(
                div()
                    .w(px(420.))
                    .max_w_full()
                    .children(self.render_tag_panel(cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_tag_panel(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.tags.open {
            return None;
        }
        let input = self.tags.input.read(cx).value().trim().to_owned();
        let query = tags::name_key(&input);
        let editing = self.tags.is_editing();
        let header = match &self.tags.mode {
            Mode::Browse => crate::i18n::tags::TAGS_TITLE.get().to_owned(),
            Mode::Edit {
                paths,
                folder: None,
            } if paths.len() == 1 => Path::new(&paths[0])
                .file_name()
                .expect("file has name")
                .to_string_lossy()
                .into_owned(),
            Mode::Edit { paths, .. } => crate::i18n::tags::mode_edit_files_tags(paths.len()),
            Mode::Rename(_) => crate::i18n::tags::MODE_RENAME_TAG.get().into(),
            Mode::Colors(_) => crate::i18n::tags::MODE_TAG_COLORS.get().into(),
            Mode::Recover(_) => crate::i18n::tags::MODE_RECOVER_FILE.get().into(),
        };
        let in_detail = !editing && !matches!(self.tags.mode, Mode::Browse);
        let folder = match &self.tags.mode {
            Mode::Edit { folder, .. } => folder.as_deref(),
            _ => None,
        };
        let mut panel = div()
            .flex()
            .flex_col()
            .mx_2()
            .mb_2()
            .p_2()
            .gap_1()
            .min_w_0()
            .border_1()
            .border_color(rgb(BORDER))
            .rounded(px(6.))
            .bg(rgb(BG))
            .when(editing, |panel| {
                panel
                    .p_4()
                    .gap_2()
                    .rounded(px(8.))
                    .bg(rgb(0xffffff))
                    .shadow_lg()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .when(in_detail, |row| {
                        row.child(
                            tag_control("tag-back", IconName::ChevronLeft, crate::i18n::tags::BACK_TO_TAG_LIST.get())
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.show_tag_panel(Mode::Browse, "", window, cx)
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .when(editing, |title| title.text_sm().font_semibold())
                            .child(header),
                    )
                    .child(
                        tag_control(
                            "tags-close",
                            IconName::Close,
                            if editing {
                                crate::i18n::tags::CLOSE_TAG_EDIT.get()
                            } else {
                                crate::i18n::tags::COLLAPSE_TAG_PANEL.get()
                            },
                        )
                        .on_click(
                            cx.listener(|app, _, window, cx| app.close_tag_panel(window, cx)),
                        ),
                    ),
            )
            .when_some(folder, |panel, folder| {
                let scope = SharedString::from(crate::i18n::tags::folder_with_subfolders(folder));
                panel.child(
                    div()
                        .id("tag-folder-scope")
                        .text_xs()
                        .text_ellipsis()
                        .text_color(rgb(MUTED))
                        .child(scope.clone())
                        .tooltip(move |window, cx| Tooltip::new(scope.clone()).build(window, cx)),
                )
            });

        match &self.tags.mode {
            Mode::Rename(_) => {
                panel = panel.child(
                    div()
                        .flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Input::new(&self.tags.input)
                                .xsmall()
                                .h(px(22.))
                                .text_size(px(12.))
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            tag_control("save-tag-name", IconName::Check, crate::i18n::tags::SAVE_TAG_NAME.get()).on_click(
                                cx.listener(|app, _, window, cx| app.submit_tag_input(window, cx)),
                            ),
                        ),
                );
            }
            Mode::Colors(id) => {
                let selected = self
                    .tags
                    .snapshot
                    .tags
                    .iter()
                    .find(|tag| &tag.id == id)
                    .map(|tag| tag.color);
                panel = panel.child(div().flex().items_center().gap_1().children(
                    COLORS.iter().enumerate().map(|(color, &(title, bg, fg))| {
                        let id = id.clone();
                        let color = color as u8;
                        div()
                            .id(SharedString::from(format!("tag-color-{color}")))
                            .size(px(20.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(bg))
                            .text_color(rgb(fg))
                            .cursor_pointer()
                            .hover(|style| style.border_1().border_color(rgb(fg)))
                            .when(selected == Some(color), |swatch| {
                                swatch.child(crate::ui::icon(IconName::Check))
                            })
                            .tooltip(move |window, cx| Tooltip::new(title.get()).build(window, cx))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                if app.change_tags(Change::Color { tag: &id, color }, cx) {
                                    app.show_tag_panel(Mode::Browse, "", window, cx);
                                }
                            }))
                    }),
                ));
            }
            Mode::Recover(id) => {
                let id = id.clone();
                let mut paths: Vec<_> = self.tree.files().iter().map(|file| &file.path).collect();
                if self.projects.directory().is_none() {
                    paths.extend(self.documents.tabs().iter().chain(self.tags.snapshot.assigned.keys()));
                    paths.sort_unstable();
                    paths.dedup();
                }
                panel = panel
                    .child(
                        Input::new(&self.tags.input)
                            .xsmall()
                            .h(px(22.))
                            .text_size(px(12.))
                            .flex_1()
                            .min_w_0(),
                    )
                    .when(self.projects.directory().is_none(), |panel| {
                        panel.child(button("tag-open-target", crate::i18n::tags::OPEN_TARGET_FILE.get()).on_click(
                            cx.listener(|app, _, window, cx| app.open_file_dialog(window, cx)),
                        ))
                    })
                    .child(
                        div()
                            .id("tag-recovery-targets")
                            .max_h(px(185.))
                            .overflow_y_scroll()
                            .children(
                                paths
                                    .into_iter()
                                    .filter(|path| path.to_lowercase().contains(&query))
                                    .map(|path| {
                                        let id = id.clone();
                                        let target = path.clone();
                                        button(
                                            SharedString::from(format!("tag-rebind-{path}")),
                                            path.clone(),
                                        )
                                        .w_full()
                                        .justify_start()
                                        .text_ellipsis()
                                        .on_click(
                                            cx.listener(move |app, _, window, cx| {
                                                if app.change_tags(
                                                    Change::Rebind {
                                                        missing: &id,
                                                        target: &target,
                                                    },
                                                    cx,
                                                ) {
                                                    app.show_tag_panel(
                                                        Mode::Browse,
                                                        "",
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            }),
                                        )
                                    }),
                            ),
                    );
            }
            Mode::Browse | Mode::Edit { .. } => {
                let exact = self
                    .tags
                    .snapshot
                    .tags
                    .iter()
                    .any(|tag| tags::name_key(&tag.name) == query);
                let paths = match &self.tags.mode {
                    Mode::Edit { paths, .. } => Some(paths.as_slice()),
                    _ => None,
                };
                let list = self
                    .tags
                    .snapshot
                    .tags
                    .iter()
                    .filter(|tag| tags::name_key(&tag.name).contains(&query));
                panel = panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Input::new(&self.tags.input)
                                    .xsmall()
                                    .h(px(22.))
                                    .text_size(px(12.))
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(
                                tag_control("create-tag", IconName::Plus, crate::i18n::tags::CREATE_TYPED_TAG.get())
                                    .when(query.is_empty() || exact, |control| {
                                        control.opacity(0.45)
                                    })
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        if !exact {
                                            app.submit_tag_input(window, cx);
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("tags-list")
                            .max_h(px(204.))
                            .overflow_y_scroll()
                            .children(list.map(|tag| self.render_tag_row(tag, paths, cx)))
                            .when(
                                paths.is_none() && !self.tags.snapshot.missing.is_empty(),
                                |list| {
                                    list.child(
                                        div()
                                            .px_1()
                                            .pt_2()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child(crate::i18n::tags::PENDING_RELINK.get()),
                                    )
                                    .children(
                                        self.tags
                                            .snapshot
                                            .missing
                                            .iter()
                                            .filter(|(_, path)| {
                                                path.to_lowercase().contains(&query)
                                            })
                                            .map(|(id, path)| {
                                                let id = id.clone();
                                                let title = SharedString::from(path.clone());
                                                div()
                                                    .id(SharedString::from(format!("missing-{id}")))
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .tooltip(move |window, cx| {
                                                        Tooltip::new(title.clone())
                                                            .build(window, cx)
                                                    })
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .px_1()
                                                            .text_ellipsis()
                                                            .text_xs()
                                                            .child(path.clone()),
                                                    )
                                                    .child(
                                                        tag_control(
                                                            "rebind",
                                                            IconName::Link,
                                                            crate::i18n::tags::RELINK_BTN.get(),
                                                        )
                                                        .on_click(cx.listener(
                                                            move |app, _, window, cx| {
                                                                app.show_tag_panel(
                                                                    Mode::Recover(id.clone()),
                                                                    "",
                                                                    window,
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                    )
                                            }),
                                    )
                                },
                            ),
                    );
            }
        }
        Some(panel.into_any_element())
    }

    fn render_tag_row(
        &self,
        tag: &tags::Tag,
        paths: Option<&[String]>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = tag.id.clone();
        let name = tag.name.clone();
        let (_, bg, fg) = COLORS.get(tag.color as usize).copied().unwrap_or(COLORS[0]);
        let title = SharedString::from(name.clone());
        let row = div()
            .id(SharedString::from(id.clone()))
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .when(self.tags.filter.as_ref() == Some(&id), |row| {
                row.bg(rgb(0xe8e7e4))
            });
        let mut label = div()
            .id("tag-name")
            .flex_1()
            .min_w_0()
            .py_1()
            .px_1()
            .text_ellipsis()
            .text_xs()
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .child(
                div()
                    .px_1p5()
                    .rounded(px(8.))
                    .bg(rgb(bg))
                    .text_color(rgb(fg))
                    .text_ellipsis()
                    .child(name.clone()),
            );
        if let Some(paths) = paths {
            let count = paths
                .iter()
                .filter(|path| self.tags.snapshot.has(path, &id))
                .count();
            let add = id.clone();
            let remove = id;
            return row
                .child(label)
                .when(paths.len() > 1, |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("{count}/{}", paths.len())),
                    )
                })
                .when(count < paths.len(), |row| {
                    row.child(
                        tag_control("add", IconName::Plus, crate::i18n::tags::ADD_TAG.get()).on_click(
                            cx.listener(move |app, _, _, cx| app.assign_tag(&add, true, cx)),
                        ),
                    )
                })
                .when(count > 0, |row| {
                    row.child(tag_control("remove", IconName::Minus, crate::i18n::tags::REMOVE_TAG.get()).on_click(
                        cx.listener(move |app, _, _, cx| app.assign_tag(&remove, false, cx)),
                    ))
                })
                .into_any_element();
        }
        let select = id.clone();
        let rename = id.clone();
        let rename_name = name.clone();
        let color = id.clone();
        label = label
            .cursor_pointer()
            .on_click(cx.listener(move |app, _, _, cx| {
                let filter = if app.tags.filter.as_ref() == Some(&select) {
                    None
                } else {
                    Some(select.clone())
                };
                app.select_tag(filter, cx);
            }));
        row.child(label)
            .child(
                tag_control("tag-rename", IconName::PencilLine, crate::i18n::tags::MODE_RENAME_TAG.get()).on_click(
                    cx.listener(move |app, _, window, cx| {
                        app.show_tag_panel(Mode::Rename(rename.clone()), &rename_name, window, cx);
                    }),
                ),
            )
            .child(
                tag_control("tag-palette", IconName::Palette, crate::i18n::tags::CHANGE_TAG_COLOR.get()).on_click(
                    cx.listener(move |app, _, window, cx| {
                        app.show_tag_panel(Mode::Colors(color.clone()), "", window, cx);
                    }),
                ),
            )
            .child(
                tag_control("tag-delete", IconName::Trash, crate::i18n::tags::DELETE_TAG.get()).on_click(cx.listener(
                    move |app, _, _, cx| app.delete_tag(id.clone(), name.clone(), cx),
                )),
            )
            .into_any_element()
    }

    fn assign_tag(&mut self, tag: &str, add: bool, cx: &mut Context<Self>) {
        if let Mode::Edit { paths, .. } = &self.tags.mode {
            let paths = paths.clone();
            self.change_tags(
                Change::Assign {
                    tag,
                    paths: &paths,
                    add,
                },
                cx,
            );
        }
    }
}
