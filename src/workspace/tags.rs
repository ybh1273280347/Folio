//! Tag controls and the active workspace's read-only sidebar projection.

use super::*;
use crate::tags::{self, Change, Snapshot, Store};
use crate::ui::{FolioContextMenuExt, FolioDropdownMenuExt, FolioMenu, FolioMenuItem, TAG_COLORS as COLORS};
use gpui_kit::component::tooltip::Tooltip;
use std::collections::HashMap;

pub(super) struct TagPanel {
    pub snapshot: Snapshot,
    pub filter: Option<String>,
    input: Entity<InputState>,
    rename_input: Entity<InputState>,
    open: bool,
    mode: Mode,
    creating: bool,
    renaming: Option<String>,
    coloring: Option<String>,
    edit_index: usize,
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
    Recover(String),
}

enum EditCandidate {
    CreateAndAdd(String),
    Tag {
        id: String,
        name: String,
        color: u8,
        count: usize,
        total: usize,
    },
}

impl TagPanel {
    pub(super) fn is_editing(&self) -> bool {
        self.open && matches!(self.mode, Mode::Edit { .. })
    }

    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::common::SEARCH_ELLIPSIS.get()));
        let rename_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(&input, window, |app, _, event: &InputEvent, window, cx| {
            match event {
                InputEvent::PressEnter { .. } => app.submit_tag_input(window, cx),
                InputEvent::Change => app.tags.edit_index = 0,
                _ => {}
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(&rename_input, window, |app, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                app.commit_tag_rename(window, cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            snapshot: Snapshot::default(),
            filter: None,
            input,
            rename_input,
            open: false,
            mode: Mode::Browse,
            creating: false,
            renaming: None,
            coloring: None,
            edit_index: 0,
            scan: None,
        }
    }

    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        let placeholder = match &self.mode {
            Mode::Edit { .. } => crate::i18n::tags::EDIT_TAG_PLACEHOLDER.get(),
            Mode::Browse => crate::i18n::tags::NEW_TAG_PLACEHOLDER.get(),
            Mode::Recover(_) => crate::i18n::common::SEARCH_ELLIPSIS.get(),
        };
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
        });
    }

    pub(super) fn reset(&mut self) {
        self.scan = None;
        self.snapshot = Snapshot::default();
        self.filter = None;
        self.open = false;
        self.mode = Mode::Browse;
        self.creating = false;
        self.renaming = None;
        self.coloring = None;
        self.edit_index = 0;
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
        if let Some(renaming) = &self.tags.renaming
            && !self.tags.snapshot.tags.iter().any(|tag| &tag.id == renaming)
        {
            self.tags.renaming = None;
        }
        if let Some(coloring) = &self.tags.coloring
            && !self.tags.snapshot.tags.iter().any(|tag| &tag.id == coloring)
        {
            self.tags.coloring = None;
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
        let focus = !matches!(mode, Mode::Browse);
        let placeholder = match &mode {
            Mode::Edit { .. } => crate::i18n::tags::EDIT_TAG_PLACEHOLDER.get(),
            Mode::Browse => crate::i18n::tags::NEW_TAG_PLACEHOLDER.get(),
            Mode::Recover(_) => crate::i18n::common::SEARCH_ELLIPSIS.get(),
        };
        self.tags.open = true;
        self.tags.mode = mode;
        self.tags.creating = false;
        self.tags.renaming = None;
        self.tags.coloring = None;
        self.tags.edit_index = 0;
        self.tags.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value(value.to_owned(), window, cx);
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
        self.tags.creating = false;
        self.tags.renaming = None;
        self.tags.coloring = None;
        self.tags.edit_index = 0;
        cx.notify();
    }

    pub(super) fn handle_tag_keys(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tags.is_editing() {
            match key {
                "escape" => {
                    self.close_tag_panel(window, cx);
                    return true;
                }
                "up" => {
                    self.move_tag_edit_selection(-1, cx);
                    return true;
                }
                "down" => {
                    self.move_tag_edit_selection(1, cx);
                    return true;
                }
                _ => return false,
            }
        }
        if self.tags.open && key == "escape" {
            if self.tags.creating {
                self.cancel_tag_create(window, cx);
                return true;
            }
            if self.tags.renaming.is_some() {
                self.cancel_tag_rename(cx);
                return true;
            }
            if self.tags.coloring.is_some() {
                self.tags.coloring = None;
                cx.notify();
                return true;
            }
        }
        false
    }

    fn move_tag_edit_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Mode::Edit { paths, .. } = &self.tags.mode else {
            return;
        };
        let input = self.tags.input.read(cx).value().trim().to_owned();
        let len = self.edit_candidates(paths, &input).len();
        if len == 0 {
            self.tags.edit_index = 0;
            return;
        }
        let current = self.tags.edit_index.min(len - 1) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.tags.edit_index = next;
        cx.notify();
    }

    fn edit_candidates(&self, paths: &[String], input: &str) -> Vec<EditCandidate> {
        let trimmed = input.trim();
        let query = tags::name_key(trimmed);
        let exact = self
            .tags
            .snapshot
            .tags
            .iter()
            .any(|tag| tags::name_key(&tag.name) == query);
        let total = paths.len();
        let mut out = Vec::new();
        if !query.is_empty() && !exact {
            out.push(EditCandidate::CreateAndAdd(trimmed.to_owned()));
        }
        let mut unassigned = Vec::new();
        let mut assigned = Vec::new();
        for tag in &self.tags.snapshot.tags {
            if !tags::name_key(&tag.name).contains(&query) {
                continue;
            }
            let count = paths
                .iter()
                .filter(|path| self.tags.snapshot.has(path, &tag.id))
                .count();
            let candidate = EditCandidate::Tag {
                id: tag.id.clone(),
                name: tag.name.clone(),
                color: tag.color,
                count,
                total,
            };
            if count < total {
                unassigned.push(candidate);
            } else {
                assigned.push(candidate);
            }
        }
        out.extend(unassigned);
        out.extend(assigned);
        out
    }

    fn activate_edit_candidate(
        &mut self,
        candidate: &EditCandidate,
        paths: &[String],
        clear_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ok = match candidate {
            EditCandidate::CreateAndAdd(name) => {
                self.change_tags(Change::Create { name, paths }, cx)
            }
            EditCandidate::Tag {
                id,
                count,
                total,
                ..
            } => self.change_tags(
                Change::Assign {
                    tag: id,
                    paths,
                    add: *count < *total,
                },
                cx,
            ),
        };
        if ok && (clear_input || matches!(candidate, EditCandidate::CreateAndAdd(_))) {
            self.tags.edit_index = 0;
            self.tags
                .input
                .update(cx, |input, cx| input.set_value(String::new(), window, cx));
        }
    }

    fn start_tag_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tags.renaming = None;
        self.tags.coloring = None;
        self.tags.creating = true;
        self.tags.input.update(cx, |input, cx| {
            input.set_placeholder(crate::i18n::tags::NEW_TAG_PLACEHOLDER.get(), window, cx);
            input.set_value(String::new(), window, cx);
        });
        let input = self.tags.input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx))
        });
        cx.notify();
    }

    fn cancel_tag_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tags.creating = false;
        self.tags
            .input
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
        cx.notify();
    }

    fn create_browse_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.tags.input.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.cancel_tag_create(window, cx);
            return;
        }
        if self.change_tags(
            Change::Create {
                name: &name,
                paths: &[],
            },
            cx,
        ) {
            self.cancel_tag_create(window, cx);
        }
    }

    fn start_tag_rename(
        &mut self,
        id: String,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tags.creating = false;
        self.tags.coloring = None;
        self.tags.renaming = Some(id);
        self.tags
            .rename_input
            .update(cx, |input, cx| input.set_value(name.to_owned(), window, cx));
        let rename_input = self.tags.rename_input.clone();
        window.defer(cx, move |window, cx| {
            rename_input.update(cx, |input, cx| input.focus(window, cx))
        });
        cx.notify();
    }

    fn commit_tag_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(tag_id) = self.tags.renaming.clone() else {
            return;
        };
        let value = self.tags.rename_input.read(cx).value().trim().to_owned();
        if value.is_empty() {
            self.cancel_tag_rename(cx);
            return;
        }
        let unchanged = self
            .tags
            .snapshot
            .tags
            .iter()
            .find(|tag| tag.id == tag_id)
            .is_some_and(|tag| tag.name == value);
        if unchanged {
            self.cancel_tag_rename(cx);
            return;
        }
        if self.change_tags(
            Change::Rename {
                tag: &tag_id,
                name: &value,
            },
            cx,
        ) {
            self.cancel_tag_rename(cx);
        }
    }

    fn cancel_tag_rename(&mut self, cx: &mut Context<Self>) {
        self.tags.renaming = None;
        cx.notify();
    }

    fn toggle_tag_colors(&mut self, id: String, cx: &mut Context<Self>) {
        self.tags.creating = false;
        self.tags.renaming = None;
        if self.tags.coloring.as_ref() == Some(&id) {
            self.tags.coloring = None;
        } else {
            self.tags.coloring = Some(id);
        }
        cx.notify();
    }

    fn submit_tag_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tags.open {
            return;
        }
        let value = self.tags.input.read(cx).value().trim().to_owned();
        match &self.tags.mode {
            Mode::Browse => {
                if self.tags.creating {
                    self.create_browse_tag(window, cx);
                }
            }
            Mode::Edit { paths, .. } => {
                let paths = paths.clone();
                let candidates = self.edit_candidates(&paths, &value);
                let index = if candidates.is_empty() {
                    return;
                } else {
                    self.tags.edit_index.min(candidates.len() - 1)
                };
                self.activate_edit_candidate(
                    &candidates[index],
                    &paths,
                    !value.is_empty(),
                    window,
                    cx,
                );
            }
            Mode::Recover(_) => {}
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
        let palette = crate::theme::palette();
        tag_control("tags-toggle", IconName::Tag, crate::i18n::tags::TAGS_TITLE.get())
            .when(self.tags.open && !self.tags.is_editing(), |control| {
                control
                    .bg(gpui_kit::rgb(palette.selection))
                    .text_color(gpui_kit::rgb(palette.text))
            })
            .on_click(cx.listener(|app, _, window, cx| {
                if app.tags.open && !app.tags.is_editing() {
                    app.close_tag_panel(window, cx);
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
        let palette = crate::theme::palette();
        let (_, bg, fg) = COLORS.get(tag.color as usize).copied().unwrap_or(COLORS[0]);
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_1p5()
                .mx_2()
                .mb_1()
                .pl_2()
                .pr_1()
                .py(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(palette.card_border())
                .bg(gpui_kit::rgb(palette.surface))
                .text_xs()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .child(crate::i18n::tags::TAGS_TITLE.get()),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .py(px(1.))
                                .rounded_full()
                                .bg(rgb(bg))
                                .text_color(rgb(fg))
                                .text_ellipsis()
                                .child(tag.name.clone()),
                        ),
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
            let palette = crate::theme::palette();
            chips = chips.child(
                div()
                    .id("more-tags")
                    .px_1()
                    .cursor_pointer()
                    .rounded(px(6.))
                    .bg(gpui_kit::rgb(palette.hover))
                    .text_color(gpui_kit::rgb(palette.muted))
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
        crate::ui::modal_shell(
            "tag-editor-backdrop",
            div()
                .id("tag-editor-card")
                .w(px(440.))
                .max_w_full()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .children(self.render_tag_panel(cx)),
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|app, _, window, cx| app.close_tag_panel(window, cx)))
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
            Mode::Recover(_) => crate::i18n::tags::MODE_RECOVER_FILE.get().into(),
        };
        let in_detail = matches!(self.tags.mode, Mode::Recover(_));
        let folder = match &self.tags.mode {
            Mode::Edit { folder, .. } => folder.as_deref(),
            _ => None,
        };
        let palette = crate::theme::palette();
        let mut panel = div()
            .flex()
            .flex_col()
            .mx_2()
            .mb_2()
            .p_2()
            .gap_1p5()
            .min_w_0()
            .border_1()
            .border_color(palette.card_border())
            .rounded(px(8.))
            .bg(palette.elevated_surface())
            .when(editing, |panel| {
                panel
                    .mx_0()
                    .mb_0()
                    .p_5()
                    .gap_2p5()
                    .rounded(px(12.))
                    .shadow(palette.modal_shadow())
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
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .font_medium()
                                    .when(editing, |title| title.text_sm().font_semibold())
                                    .child(header),
                            )
                            .when(matches!(self.tags.mode, Mode::Browse) && !self.tags.snapshot.tags.is_empty(), |title| {
                                title.child(
                                    div()
                                        .text_xs()
                                        .text_color(gpui_kit::rgb(palette.muted))
                                        .child(format!("{}", self.tags.snapshot.tags.len())),
                                )
                            }),
                    )
                    .when(matches!(self.tags.mode, Mode::Browse), |row| {
                        row.child(
                            tag_control("tag-create-toggle", IconName::Plus, crate::i18n::tags::NEW_TAG_BTN.get())
                                .when(self.tags.creating, |btn| {
                                    btn.bg(gpui_kit::rgb(palette.selection))
                                        .text_color(gpui_kit::rgb(palette.text))
                                })
                                .on_click(cx.listener(|app, _, window, cx| {
                                    if app.tags.creating {
                                        app.cancel_tag_create(window, cx);
                                    } else {
                                        app.start_tag_create(window, cx);
                                    }
                                })),
                        )
                    })
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
                        .text_color(gpui_kit::rgb(palette.muted))
                        .child(scope.clone())
                        .tooltip(move |window, cx| Tooltip::new(scope.clone()).build(window, cx)),
                )
            });

        match &self.tags.mode {
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
                        crate::ui::SearchField::new("tag-recover-search", &self.tags.input)
                            .surface_bg(),
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
            Mode::Edit { paths, .. } => {
                panel = self.render_edit_body(panel, paths, &input, palette, cx);
            }
            Mode::Browse => {
                panel = self.render_browse_body(panel, palette, cx);
            }
        }
        Some(panel.into_any_element())
    }

    fn render_edit_body(
        &self,
        panel: Div,
        paths: &[String],
        input: &str,
        palette: crate::theme::Palette,
        cx: &Context<Self>,
    ) -> Div {
        let total = paths.len();
        let assigned_tags: Vec<_> = self
            .tags
            .snapshot
            .tags
            .iter()
            .filter_map(|tag| {
                let count = paths
                    .iter()
                    .filter(|path| self.tags.snapshot.has(path, &tag.id))
                    .count();
                (count > 0).then_some((tag, count))
            })
            .collect();

        let chips_box = div()
            .w_full()
            .min_h(px(36.))
            .p_2()
            .rounded(px(8.))
            .bg(gpui_kit::rgb(palette.background))
            .border_1()
            .border_color(palette.card_border())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .when(assigned_tags.is_empty(), |box_el| {
                box_el.child(
                    div()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .child(crate::i18n::tags::NO_ASSIGNED_TAGS.get()),
                )
            })
            .children(assigned_tags.into_iter().map(|(tag, count)| {
                let id_remove = tag.id.clone();
                let id_add = tag.id.clone();
                let (_, bg, fg) = COLORS.get(tag.color as usize).copied().unwrap_or(COLORS[0]);
                let partial = total > 1 && count < total;
                let label = if partial {
                    format!("{} {count}/{total}", tag.name)
                } else {
                    tag.name.clone()
                };
                div()
                    .id(SharedString::from(format!("assigned-chip-{}", tag.id)))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py(px(2.))
                    .rounded_full()
                    .bg(rgb(bg))
                    .text_color(rgb(fg))
                    .text_xs()
                    .child(div().max_w(px(180.)).text_ellipsis().child(label))
                    .when(partial, |chip| {
                        chip.child(
                            div()
                                .id(SharedString::from(format!("chip-add-all-{}", tag.id)))
                                .size(px(15.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.7))
                                .tooltip(|w, cx| {
                                    Tooltip::new(crate::i18n::tags::APPLY_TAG_TO_ALL.get())
                                        .build(w, cx)
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.assign_tag(&id_add, true, cx);
                                }))
                                .child(crate::ui::icon(IconName::Plus).size(px(11.))),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("chip-remove-{}", tag.id)))
                            .size(px(15.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.7))
                            .tooltip(|w, cx| {
                                Tooltip::new(crate::i18n::tags::REMOVE_TAG.get()).build(w, cx)
                            })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.assign_tag(&id_remove, false, cx);
                            }))
                            .child(crate::ui::icon(IconName::Close).size(px(11.))),
                    )
            }));

        let candidates = self.edit_candidates(paths, input);
        let active_idx = (!candidates.is_empty()).then(|| self.tags.edit_index.min(candidates.len() - 1));
        let paths_owned = paths.to_vec();
        let clear_on_pick = !input.is_empty();

        let list = div()
            .id("tags-edit-candidates")
            .max_h(px(220.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .when(candidates.is_empty(), |list| {
                list.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .child(crate::i18n::tags::NO_MATCHING_TAGS.get()),
                )
            })
            .children(candidates.into_iter().enumerate().map(|(ix, candidate)| {
                let highlighted = active_idx == Some(ix);
                let row_paths = paths_owned.clone();
                let row = div()
                    .id(SharedString::from(format!("tag-edit-cand-{ix}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_xs()
                    .when(highlighted, |row| row.bg(gpui_kit::rgb(palette.selection)))
                    .when(!highlighted, |row| {
                        row.hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                    });

                match candidate {
                    EditCandidate::CreateAndAdd(name) => {
                        let create_name = name.clone();
                        row.on_click(cx.listener(move |app, _, window, cx| {
                            app.activate_edit_candidate(
                                &EditCandidate::CreateAndAdd(create_name.clone()),
                                &row_paths,
                                true,
                                window,
                                cx,
                            );
                        }))
                        .child(
                            crate::ui::icon(IconName::Plus)
                                .size(px(13.))
                                .text_color(gpui_kit::rgb(palette.accent))
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_color(gpui_kit::rgb(palette.accent))
                                .font_medium()
                                .child(crate::i18n::tags::create_and_add_tag(&name)),
                        )
                    }
                    EditCandidate::Tag {
                        id,
                        name,
                        color,
                        count,
                        total,
                    } => {
                        let (_, bg, fg) = COLORS.get(color as usize).copied().unwrap_or(COLORS[0]);
                        let click_id = id.clone();
                        let click_name = name.clone();
                        let fully_assigned = count == total;
                        row.on_click(cx.listener(move |app, _, window, cx| {
                            app.activate_edit_candidate(
                                &EditCandidate::Tag {
                                    id: click_id.clone(),
                                    name: click_name.clone(),
                                    color,
                                    count,
                                    total,
                                },
                                &row_paths,
                                clear_on_pick,
                                window,
                                cx,
                            );
                        }))
                        .child(
                            div()
                                .px_2()
                                .py(px(1.5))
                                .rounded_full()
                                .bg(rgb(bg))
                                .text_color(rgb(fg))
                                .max_w(px(260.))
                                .text_ellipsis()
                                .child(name),
                        )
                        .child(div().flex_1().min_w_0())
                        .when(fully_assigned, |row| {
                            row.child(
                                crate::ui::icon(IconName::Check)
                                    .size(px(13.))
                                    .text_color(gpui_kit::rgb(palette.accent)),
                            )
                        })
                        .when(total > 1 && count > 0 && !fully_assigned, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child(format!("{count}/{total}")),
                            )
                        })
                    }
                }
            }));

        panel
            .child(chips_box)
            .child(crate::ui::TextField::new(&self.tags.input).compact())
            .child(list)
    }

    fn inline_tag_input_row(
        id: SharedString,
        input: &Entity<InputState>,
        save_id: &'static str,
        save_tip: &'static str,
        on_save: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
        cancel_id: &'static str,
        on_cancel: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .py_0p5()
            .child(
                div().flex_1().min_w_0().child(
                    crate::ui::TextField::new(input)
                        .compact()
                        .surface_bg(),
                ),
            )
            .child(
                tag_control(save_id, IconName::Check, save_tip)
                    .on_click(cx.listener(move |app, _, window, cx| on_save(app, window, cx))),
            )
            .child(
                tag_control(cancel_id, IconName::Close, crate::i18n::common::CANCEL.get())
                    .on_click(cx.listener(move |app, _, window, cx| on_cancel(app, window, cx))),
            )
    }

    fn render_browse_body(
        &self,
        panel: Div,
        palette: crate::theme::Palette,
        cx: &Context<Self>,
    ) -> Div {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for ids in self.tags.snapshot.assigned.values() {
            for id in ids {
                *counts.entry(id.as_str()).or_insert(0) += 1;
            }
        }
        let create_row = self.tags.creating.then(|| {
            Self::inline_tag_input_row(
                "tag-creating-row".into(),
                &self.tags.input,
                "save-new-tag",
                crate::i18n::tags::NEW_TAG_BTN.get(),
                |app, window, cx| app.create_browse_tag(window, cx),
                "cancel-new-tag",
                |app, window, cx| app.cancel_tag_create(window, cx),
                cx,
            )
        });

        let empty_hint = (self.tags.snapshot.tags.is_empty()
            && !self.tags.creating
            && self.tags.snapshot.missing.is_empty())
        .then(|| {
            div()
                .id("empty-new-tag-btn")
                .flex()
                .items_center()
                .gap_1p5()
                .px_1p5()
                .py_1p5()
                .rounded(px(6.))
                .cursor_pointer()
                .text_xs()
                .text_color(gpui_kit::rgb(palette.muted))
                .hover(move |s| {
                    s.bg(gpui_kit::rgb(palette.hover))
                        .text_color(gpui_kit::rgb(palette.text))
                })
                .on_click(cx.listener(|app, _, window, cx| app.start_tag_create(window, cx)))
                .child(crate::ui::icon(IconName::Plus).size(px(13.)).flex_shrink_0())
                .child(crate::i18n::tags::NEW_TAG_BTN.get())
        });

        panel
            .children(create_row)
            .child(
                div()
                    .id("tags-list")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(
                        self.tags
                            .snapshot
                            .tags
                            .iter()
                            .map(|tag| self.render_browse_tag_row(tag, counts.get(tag.id.as_str()).copied().unwrap_or(0), palette, cx)),
                    )
                    .children(empty_hint)
                    .when(!self.tags.snapshot.missing.is_empty(), |list| {
                        list.child(
                            div()
                                .px_1()
                                .pt_2()
                                .text_xs()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .child(crate::i18n::tags::PENDING_RELINK.get()),
                        )
                        .children(
                            self.tags
                                .snapshot
                                .missing
                                .iter()
                                .map(|(id, path)| {
                                    let id = id.clone();
                                    let title = SharedString::from(path.clone());
                                    div()
                                        .id(SharedString::from(format!("missing-{id}")))
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(title.clone()).build(window, cx)
                                        })
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .px_1()
                                                .text_ellipsis()
                                                .text_xs()
                                                .text_color(gpui_kit::rgb(palette.text))
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
                    }),
            )
    }

    fn build_tag_action_menu(
        menu: FolioMenu,
        app: Entity<Workspace>,
        id: String,
        name: String,
    ) -> FolioMenu {
        let rename_app = app.clone();
        let rename_id = id.clone();
        let rename_name = name.clone();
        let color_app = app.clone();
        let color_id = id.clone();
        let delete_app = app;
        let delete_id = id;
        let delete_name = name;
        menu.min_w(px(148.))
            .item(
                FolioMenuItem::new(crate::i18n::tags::MODE_RENAME_TAG.get())
                    .icon(IconName::PencilLine)
                    .on_click(move |_, window, cx| {
                        rename_app.update(cx, |app, cx| {
                            app.start_tag_rename(rename_id.clone(), &rename_name, window, cx);
                        });
                    }),
            )
            .item(
                FolioMenuItem::new(crate::i18n::tags::CHANGE_TAG_COLOR.get())
                    .icon(IconName::Palette)
                    .on_click(move |_, _, cx| {
                        color_app.update(cx, |app, cx| {
                            app.toggle_tag_colors(color_id.clone(), cx);
                        });
                    }),
            )
            .separator()
            .item(
                FolioMenuItem::new(crate::i18n::tags::DELETE_TAG.get())
                    .icon(IconName::Trash)
                    .danger()
                    .on_click(move |_, _, cx| {
                        delete_app.update(cx, |app, cx| {
                            app.delete_tag(delete_id.clone(), delete_name.clone(), cx);
                        });
                    }),
            )
    }

    fn render_browse_tag_row(
        &self,
        tag: &tags::Tag,
        file_count: usize,
        palette: crate::theme::Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = tag.id.clone();
        let name = tag.name.clone();
        if self.tags.renaming.as_deref() == Some(&id) {
            return Self::inline_tag_input_row(
                SharedString::from(format!("tag-renaming-{id}")),
                &self.tags.rename_input,
                "save-tag-name",
                crate::i18n::tags::SAVE_TAG_NAME.get(),
                |app, window, cx| app.commit_tag_rename(window, cx),
                "cancel-tag-name",
                |app, _, cx| app.cancel_tag_rename(cx),
                cx,
            )
            .into_any_element();
        }

        let (_, bg, fg) = COLORS.get(tag.color as usize).copied().unwrap_or(COLORS[0]);
        let active_filter = self.tags.filter.as_ref() == Some(&id);
        let showing_colors = self.tags.coloring.as_deref() == Some(&id);
        let row_group: SharedString = format!("tag-row-grp-{id}").into();
        let title = SharedString::from(name.clone());
        let select = id.clone();

        let ctx_app = cx.entity();
        let ctx_id = id.clone();
        let ctx_name = name.clone();
        let menu_app = cx.entity();
        let menu_id = id.clone();
        let menu_name = name.clone();

        let more_btn = div()
            .flex_shrink_0()
            .opacity(if showing_colors { 1.0 } else { 0.0 })
            .group_hover(row_group.clone(), |s| s.opacity(1.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                crate::ui::panel_toolbar_button(
                    SharedString::from(format!("tag-more-{id}")),
                    palette,
                    cx,
                )
                .w(px(22.))
                .h(px(22.))
                .rounded(px(5.))
                .icon(crate::ui::icon(IconName::Ellipsis).size(px(13.)))
                .tooltip(crate::i18n::tags::MORE_TAG_ACTIONS.get())
                .folio_dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                    Self::build_tag_action_menu(
                        menu,
                        menu_app.clone(),
                        menu_id.clone(),
                        menu_name.clone(),
                    )
                }),
            );

        let row = div()
            .id(SharedString::from(format!("tag-row-{id}")))
            .group(row_group)
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .px_1()
            .py(px(2.))
            .rounded(px(6.))
            .when(active_filter, |row| row.bg(gpui_kit::rgb(palette.selection)))
            .when(!active_filter, |row| {
                row.hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
            })
            .folio_context_menu(move |menu, _, _| {
                Self::build_tag_action_menu(menu, ctx_app.clone(), ctx_id.clone(), ctx_name.clone())
            })
            .child(
                div()
                    .id(SharedString::from(format!("tag-select-{id}")))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        let filter = if app.tags.filter.as_ref() == Some(&select) {
                            None
                        } else {
                            Some(select.clone())
                        };
                        app.select_tag(filter, cx);
                    }))
                    .child(
                        div()
                            .max_w(px(150.))
                            .px_2()
                            .py(px(1.5))
                            .rounded_full()
                            .bg(rgb(bg))
                            .text_color(rgb(fg))
                            .text_xs()
                            .text_ellipsis()
                            .child(name),
                    )
                    .child(div().flex_1().min_w_0())
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child(format!("{file_count}")),
                    ),
            )
            .child(more_btn);

        if !showing_colors {
            return row.into_any_element();
        }

        let swatches = div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .children(COLORS.iter().enumerate().map(|(color, &(title, swatch_bg, swatch_fg))| {
                let swatch_id = id.clone();
                let color = color as u8;
                div()
                    .id(SharedString::from(format!("tag-color-{id}-{color}")))
                    .size(px(18.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(swatch_bg))
                    .text_color(rgb(swatch_fg))
                    .cursor_pointer()
                    .hover(|style| style.border_1().border_color(rgb(swatch_fg)))
                    .when(tag.color == color, |swatch| {
                        swatch.child(crate::ui::icon(IconName::Check).size(px(11.)))
                    })
                    .tooltip(move |window, cx| Tooltip::new(title.get()).build(window, cx))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if app.change_tags(
                            Change::Color {
                                tag: &swatch_id,
                                color,
                            },
                            cx,
                        ) {
                            app.tags.coloring = None;
                            cx.notify();
                        }
                    }))
            }));

        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(row)
            .child(swatches)
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
