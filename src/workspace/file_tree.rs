//! File tree actions, selection and rendering.

use super::*;
use std::collections::HashMap;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};

struct TreeSelectionDrag {
    start: Point<Pixels>,
    current: Point<Pixels>,
    original: BTreeSet<String>,
    moved: bool,
}

impl TreeSelectionDrag {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.start.x.min(self.current.x), self.start.y.min(self.current.y)),
            point(self.start.x.max(self.current.x), self.start.y.max(self.current.y)),
        )
    }
}

#[derive(Clone)]
enum NameAction {
    Create,
    CreateFolder,
    RenameFile(String),
    RenameFolder(String),
    RenameProject(String),
}

/// The file listing comes from disk. Search, selection, drag and trash visibility
/// are view state, while collapsed folders are captured in the workspace session.
pub(super) struct FileTree {
    files: Vec<ProjectFile>,
    folders: Vec<String>,
    searching: bool,
    search_input: Entity<InputState>,
    collapsed_folders: BTreeSet<String>,
    focused_folder: Option<String>,
    focused_file: Option<String>,
    selected_files: BTreeSet<String>,
    tree_row_bounds: Arc<Mutex<BTreeMap<String, (bool, Bounds<Pixels>)>>>,
    tree_drag: Option<TreeSelectionDrag>,
    trash_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging_tree_item: Arc<AtomicBool>,
    trash_entries: Vec<crate::trash::TrashEntry>,
    show_trash: bool,
    next_trash_cleanup: Instant,
    file_tree_focus: FocusHandle,
    sidebar_scroll: ScrollHandle,
    name_action: Option<NameAction>,
    name_input: Entity<InputState>,
}

impl FileTree {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(&name_input, window, |app, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) && app.tree.name_action.is_some() {
                if let Err(error) = app.apply_name(window, cx) {
                    app.report(error, cx);
                }
            }
        }).detach();
        let search_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&search_input, |app, _, _: &InputEvent, cx| {
            if app.tags.filter.is_some() {
                app.prune_tag_selection(cx);
            }
            cx.notify();
        }).detach();
        Self {
            files: Vec::new(),
            folders: Vec::new(),
            searching: false,
            search_input,
            collapsed_folders: BTreeSet::new(),
            focused_folder: None,
            focused_file: None,
            selected_files: BTreeSet::new(),
            tree_row_bounds: Arc::new(Mutex::new(BTreeMap::new())),
            tree_drag: None,
            trash_bounds: Arc::new(Mutex::new(None)),
            dragging_tree_item: Arc::new(AtomicBool::new(false)),
            trash_entries: Vec::new(),
            show_trash: false,
            next_trash_cleanup: Instant::now(),
            file_tree_focus: cx.focus_handle(),
            sidebar_scroll: ScrollHandle::new(),
            name_action: None,
            name_input,
        }
    }

    pub(super) fn files(&self) -> &[ProjectFile] {
        &self.files
    }

    pub(super) fn folders(&self) -> &[String] {
        &self.folders
    }

    pub(super) fn selected_files(&self) -> &BTreeSet<String> {
        &self.selected_files
    }

    pub(super) fn focused_folder(&self) -> Option<&str> {
        self.focused_folder.as_deref()
    }

    pub(super) fn collapsed_folders(&self) -> &BTreeSet<String> {
        &self.collapsed_folders
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.file_tree_focus.focus(window, cx);
    }

    pub(super) fn scroll_to_tags(&self, project_count: usize) {
        self.sidebar_scroll.scroll_to_top_of_item(project_count + 2);
    }

    pub(super) fn is_trash_open(&self) -> bool {
        self.show_trash
    }

    pub(super) fn dismiss_trash(&mut self) {
        self.show_trash = false;
    }

    pub(super) fn name_dialog_open(&self) -> bool {
        self.name_action.is_some()
    }

    pub(super) fn dismiss_overlays(&mut self) {
        self.name_action = None;
        self.searching = false;
    }

    pub(super) fn replace_listing(&mut self, files: Vec<ProjectFile>, folders: Vec<String>) {
        self.files = files;
        self.folders = folders;
    }

    pub(super) fn reload_files(&mut self, root: &Path) -> io::Result<()> {
        self.files = files::list_files(root)?;
        Ok(())
    }

    pub(super) fn reload_watched_listing(&mut self, root: &Path) -> io::Result<()> {
        self.reload_files(root)?;
        self.folders = files::list_folders(root).unwrap_or_default();
        if self.focused_folder.as_ref().is_some_and(|folder| !self.folders.contains(folder)) {
            self.focused_folder = None;
        }
        Ok(())
    }

    pub(super) fn reset_selection(&mut self) {
        self.focused_folder = None;
        self.selected_files.clear();
    }

    pub(super) fn clear_directory(&mut self) {
        self.files.clear();
        self.folders.clear();
        self.collapsed_folders.clear();
        self.reset_selection();
    }

    pub(super) fn restore_collapsed_folders(&mut self, saved: Option<BTreeSet<String>>) {
        self.collapsed_folders = saved.unwrap_or_else(|| self.folders.iter().cloned().collect());
    }

    pub(super) fn relocate_file(&mut self, old: &str, new: &str) {
        if self.focused_file.as_deref() == Some(old) {
            self.focused_file = Some(new.to_owned());
        }
        if self.selected_files.remove(old) {
            self.selected_files.insert(new.to_owned());
        }
    }
}

#[derive(Clone, Copy)]
struct CreatablePreset {
    ext: &'static str,
    sample_path: &'static str,
    menu_label: &'static str,
    folder_label: crate::i18n::BiText,
}

const CREATABLE_PRESETS: &[CreatablePreset] = &[
    CreatablePreset {
        ext: "md",
        sample_path: "untitled.md",
        menu_label: ".md",
        folder_label: crate::i18n::workspace::MENU_NEW_MARKDOWN,
    },
    CreatablePreset {
        ext: "tex",
        sample_path: "untitled.tex",
        menu_label: ".tex",
        folder_label: crate::i18n::workspace::MENU_NEW_TEX,
    },
    CreatablePreset {
        ext: "drawio",
        sample_path: "untitled.drawio",
        menu_label: ".drawio",
        folder_label: crate::i18n::workspace::MENU_NEW_DRAWIO,
    },
    CreatablePreset {
        ext: "bib",
        sample_path: "untitled.bib",
        menu_label: ".bib",
        folder_label: crate::i18n::workspace::MENU_NEW_BIB,
    },
    CreatablePreset {
        ext: "txt",
        sample_path: "untitled.txt",
        menu_label: ".txt",
        folder_label: crate::i18n::workspace::MENU_NEW_TXT,
    },
];

impl Workspace {
    pub(super) fn apply_tree_filter(&mut self, cx: &App) {
        self.tree.tree_drag = None;
        self.tree.focused_folder = None;
        self.prune_tag_selection(cx);
    }

    pub(super) fn prune_tag_selection(&mut self, cx: &App) {
        if self.tree.selected_files.is_empty() {
            return;
        }
        let query = if self.tree.searching {
            self.tree.search_input.read(cx).value().to_lowercase()
        } else {
            String::new()
        };
        let filtered = self.tags.filter.is_some() || !query.is_empty();
        let available: BTreeSet<_> = self.tree.files.iter().map(|file| file.path.as_str()).collect();
        self.tree.selected_files.retain(|path| {
            available.contains(path.as_str())
                && self
                    .tags
                    .filter
                    .as_deref()
                    .is_none_or(|tag| self.tags.snapshot.has(path, tag))
                && path.to_lowercase().contains(&query)
                && (filtered
                    || !self.tree.collapsed_folders.iter().any(|folder| {
                        path.strip_prefix(folder)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                    }))
        });
    }

    pub(super) fn render_name_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let action = self.tree.name_action.as_ref()?;
        let title = match action {
            NameAction::Create => crate::i18n::workspace::MODAL_CREATE_FILE.get(),
            NameAction::CreateFolder => crate::i18n::workspace::MODAL_CREATE_FOLDER.get(),
            NameAction::RenameFile(_) => crate::i18n::workspace::MODAL_RENAME_FILE.get(),
            NameAction::RenameFolder(_) => crate::i18n::workspace::MODAL_RENAME_FOLDER.get(),
            NameAction::RenameProject(_) => crate::i18n::workspace::MODAL_RENAME_PROJECT.get(),
        };
        Some(
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
                        .p_6()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(rgb(0xe6e5e1))
                        .shadow_lg()
                        .bg(rgb(0xffffff))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().font_semibold().child(title))
                                .child(
                                    crate::ui::control("close-name-dialog", IconName::Close)
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.tree.name_action = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(Input::new(&self.tree.name_input))
                        .when(matches!(action, NameAction::Create), |modal| {
                            modal.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(crate::i18n::workspace::CREATE_FILE_HINT.get()),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_3()
                                .child(
                                    crate::ui::button_secondary("cancel-name", crate::i18n::common::CANCEL.get())
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.tree.name_action = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    crate::ui::button_primary("apply-name", crate::i18n::common::CONFIRM.get()).on_click(
                                        cx.listener(|app, _, window, cx| {
                                            if let Err(error) = app.apply_name(window, cx) {
                                                app.report(error, cx);
                                            }
                                        }),
                                    ),
                                ),
                        ),
                ).into_any_element(),
        )
    }

    pub(super) fn poll_trash(&mut self, cx: &mut Context<Self>) {
        if Instant::now() >= self.tree.next_trash_cleanup {
            self.tree.next_trash_cleanup = Instant::now() + Duration::from_secs(3600);
            match crate::trash::cleanup().and_then(|_| Ok(crate::trash::list()?)) {
                Ok(entries) => self.tree.trash_entries = entries,
                Err(error) => self.report(error, cx),
            }
            cx.notify();
        }
    }

    fn start_name(
        &mut self,
        action: NameAction,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tree.name_action = Some(action);
        self.tree.name_input.update(cx, |input, cx| {
            input.set_value(value.to_owned(), window, cx);
        });
        // Let the menu and its pointer event finish before the dialog takes focus.
        let input = self.tree.name_input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }

    fn move_tree_files(&mut self, dragged: &crate::terminal::DraggedPaths, folder: &str, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let project = self.projects.directory().ok_or_else(|| io::Error::other(crate::i18n::workspace::ERR_SELECT_SOURCE_PROJECT.get()))?;
        if dragged.1.as_deref() != Some(project.id.as_str()) {
            return Err(io::Error::other(crate::i18n::workspace::ERR_MOVE_SAME_PROJECT_FILE_ONLY.get()));
        }
        if self.build.is_busy() {
            return Err(io::Error::other(crate::i18n::workspace::ERR_WAIT_BUILD_MOVE_FILE.get()));
        }
        let root = PathBuf::from(&project.folder);
        // The empty relative directory denotes the project root, never an absolute file key.
        let destination = files::project_path(&root, if folder.is_empty() { "." } else { folder })?;
        let mut moves = Vec::new();
        let mut targets = BTreeSet::new();
        // Preflight the whole selection before touching disk; existing targets are never replaced.
        for source in &dragged.0 {
            let relative = source.strip_prefix(&root).map_err(|_| io::Error::other(crate::i18n::workspace::ERR_SOURCE_NOT_IN_PROJECT.get()))?.to_string_lossy().replace('\\', "/");
            let source = files::project_path(&root, &relative)?;
            let target = destination.join(source.file_name().expect("dragged file has name"));
            if source == target { continue; }
            if target.exists() || !targets.insert(target.to_string_lossy().to_lowercase()) {
                return Err(io::Error::other(crate::i18n::workspace::err_target_exists_unmoved(&target)));
            }
            let name = source.file_name().unwrap().to_string_lossy();
            let renamed = if folder.is_empty() { name.into_owned() } else { format!("{folder}/{name}") };
            moves.push((source, target, relative, renamed));
        }
        let mut result = Ok(());
        let mut relocated = Vec::new();
        for (source, target, relative, renamed) in moves {
            // Match rename's no-overwrite hard-link operation. On failure, retain earlier successes.
            result = std::fs::hard_link(&source, &target).and_then(|_| std::fs::remove_file(&source));
            if result.is_err() { break; }
            self.relocate_file_state(&relative, &renamed, window, cx);
            self.expand_ancestors(&renamed);
            relocated.push((relative, renamed));
            result = crate::annotations::relocate_annotations(&source, &target);
            if result.is_err() { break; }
        }
        self.tree.files = files::list_files(&root)?;
        self.tree.folders = files::list_folders(&root)?;
        self.relocate_tags(&relocated, false, cx);
        self.projects.save()?;
        self.remember_session()?;
        cx.notify();
        result
    }

    fn move_tree_folder(&mut self, dragged: &crate::terminal::DraggedFolder, folder: &str, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let project = self.projects.directory().ok_or_else(|| io::Error::other(crate::i18n::workspace::ERR_SELECT_SOURCE_PROJECT.get()))?;
        if dragged.1 != project.id {
            return Err(io::Error::other(crate::i18n::workspace::ERR_MOVE_SAME_PROJECT_FOLDER_ONLY.get()));
        }
        if self.build.is_busy() {
            return Err(io::Error::other(crate::i18n::workspace::ERR_WAIT_BUILD_MOVE_FOLDER.get()));
        }
        let root = PathBuf::from(&project.folder);
        let renamed = files::move_folder_into(&root, &dragged.0, folder)?;
        if renamed == dragged.0 {
            return Ok(());
        }
        self.relocate_folder_state(&dragged.0, &renamed, window, cx);
        self.tree.focused_folder = Some(renamed.clone());
        self.expand_ancestors(&renamed);
        // Folder-local .folio stores move with the directory.
        self.tree.files = files::list_files(&root)?;
        self.tree.folders = files::list_folders(&root)?;
        self.relocate_tags(&[(dragged.0.clone(), renamed)], true, cx);
        self.projects.save()?;
        self.remember_session()?;
        self.tree.file_tree_focus.focus(window, cx);
        cx.notify();
        Ok(())
    }

    fn relocate_folder_state(&mut self, old: &str, renamed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let old_prefix = format!("{old}/");
        // Include open buffers even if their file was removed externally; never reload a moved draft.
        let keys: BTreeSet<String> = self.tree.files.iter().map(|file| &file.path)
            .chain(self.documents.tracked_paths())
            .filter(|key| key.starts_with(&old_prefix)).cloned().collect();
        for key in keys {
            let new_key = format!("{renamed}/{}", key.strip_prefix(&old_prefix).expect("matched folder prefix"));
            self.relocate_file_state(&key, &new_key, window, cx);
        }
        let remap = |path: &str| {
            if path == old { renamed.to_owned() }
            else if let Some(suffix) = path.strip_prefix(&old_prefix) { format!("{renamed}/{suffix}") }
            else { path.to_owned() }
        };
        self.tree.collapsed_folders = self.tree.collapsed_folders.iter().map(|path| remap(path)).collect();
        self.tree.focused_folder = self.tree.focused_folder.as_deref().map(remap);
        self.tree.focused_file = self.tree.focused_file.as_deref().map(remap);
        self.invalidate_document_tree();
    }

    fn focused_folder_prefix(&self) -> String {
        self.tree.focused_folder
            .as_deref()
            .filter(|folder| !folder.is_empty())
            .map_or_else(String::new, |folder| format!("{folder}/"))
    }

    pub(super) fn expand_ancestors(&mut self, relative: &str) {
        let mut prefix = String::new();
        let parts: Vec<_> = relative.split('/').collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            self.tree.collapsed_folders.remove(&prefix);
        }
    }

    pub(super) fn new_markdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let folder = self.tree.focused_folder.clone();
        self.new_preset_in("md", folder.as_deref(), window, cx)
    }

    pub(super) fn new_default_markdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        if !self.projects.is_default() {
            if self.build.is_busy() { return Ok(()); }
            if !self.confirm_drafts(cx, |app, window, cx| {
                if let Err(error) = app.new_default_markdown(window, cx) { app.report(error, cx); }
            }) { return Ok(()); }
            self.select_project("__folio_default", window, cx)?;
        }
        self.tree.show_trash = false;
        self.new_preset_in("md", None, window, cx)
    }

    fn ensure_creatable_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> io::Result<bool> {
        if self.projects.directory().is_some() {
            return Ok(true);
        }
        if self.build.is_busy() {
            return Ok(false);
        }
        if !self.confirm_drafts(cx, retry) {
            return Ok(false);
        }
        self.select_project("__folio_default", window, cx)?;
        Ok(true)
    }

    fn new_preset_in(
        &mut self,
        ext: &'static str,
        folder: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let target_folder = folder.map(ToOwned::to_owned);
        if !self.ensure_creatable_workspace(window, cx, move |app, window, cx| {
            if let Err(error) = app.new_preset_in(ext, target_folder.as_deref(), window, cx) {
                app.report(error, cx);
            }
        })? {
            return Ok(());
        }
        let root = PathBuf::from(&self.projects.directory().unwrap().folder);
        let file = match ext {
            "md" => files::create_markdown_in(&root, folder)?,
            "drawio" => files::create_drawio_in(&root, folder)?,
            _ => files::create_untitled_in(&root, folder, |n| {
                crate::i18n::workspace::untitled_ext(ext, n)
            })?,
        };
        self.tree.files = files::list_files(&root)?;
        self.expand_ancestors(&file.path);
        self.open_file(&file, window, cx)
    }

    fn start_create_other(
        &mut self,
        folder: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let target_folder = folder.map(ToOwned::to_owned);
        if !self.ensure_creatable_workspace(window, cx, move |app, window, cx| {
            if let Err(error) = app.start_create_other(target_folder.as_deref(), window, cx) {
                app.report(error, cx);
            }
        })? {
            return Ok(());
        }
        let prefix = folder
            .map(|folder| folder.trim().trim_matches(['/', '\\']))
            .filter(|folder| !folder.is_empty())
            .map_or_else(String::new, |folder| format!("{folder}/"));
        self.start_name(NameAction::Create, &prefix, window, cx);
        Ok(())
    }

    fn render_new_file_split_button(&self, cx: &Context<Self>) -> AnyElement {
        let md_app = cx.entity();
        let menu_app = cx.entity();
        div()
            .flex()
            .items_center()
            .rounded(px(5.))
            .child(
                button("new-markdown", "")
                    .rounded_r_none()
                    .px_1p5()
                    .gap_0p5()
                    .child(crate::ui::icon(IconName::Plus).w(px(12.)).h(px(12.)))
                    .child(crate::ui::icon(IconName::NotebookPen).w(px(13.)).h(px(13.)))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            crate::i18n::workspace::MENU_NEW_MARKDOWN.get(),
                        )
                        .build(window, cx)
                    })
                    .on_click(move |_, window, cx| {
                        md_app.update(cx, |app, cx| {
                            if let Err(error) = app.new_markdown(window, cx) {
                                app.report(error, cx);
                            }
                        });
                    }),
            )
            .child(div().w(px(1.)).h(px(14.)).bg(rgb(0xe2e1dc)))
            .child(
                Button::new("new-file-menu")
                    .ghost()
                    .compact()
                    .with_size(px(12.))
                    .w(px(20.))
                    .h(px(27.))
                    .rounded_l_none()
                    .icon(crate::ui::icon(IconName::ChevronDown).size(px(12.)))
                    .tooltip(crate::i18n::workspace::TOOLTIP_NEW_FILE_MENU.get())
                    .dropdown_menu(move |menu, _, _| {
                        let menu = CREATABLE_PRESETS
                            .iter()
                            .fold(menu.min_w(px(124.)), |menu, preset| {
                                let preset_app = menu_app.clone();
                                let ext = preset.ext;
                                let sample_path = preset.sample_path;
                                let menu_label = preset.menu_label;
                                menu.item(
                                    PopupMenuItem::element(move |_, _| {
                                        div()
                                            .w_full()
                                            .h(px(20.))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_xs()
                                            .child(crate::ui::file_icon(
                                                sample_path,
                                                ext == "drawio",
                                                false,
                                            ))
                                            .child(menu_label)
                                    })
                                    .on_click(move |_, window, cx| {
                                        preset_app.update(cx, |app, cx| {
                                            let folder = app.tree.focused_folder.clone();
                                            if let Err(error) = app.new_preset_in(
                                                ext,
                                                folder.as_deref(),
                                                window,
                                                cx,
                                            ) {
                                                app.report(error, cx);
                                            }
                                        });
                                    }),
                                )
                            });
                        let other_app = menu_app.clone();
                        menu.separator().item(
                            PopupMenuItem::element(move |_, _| {
                                div()
                                    .w_full()
                                    .h(px(20.))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .child(crate::ui::icon(IconName::Plus).size(px(13.)))
                                    .child(crate::i18n::workspace::MENU_NEW_OTHER.get())
                            })
                            .on_click(move |_, window, cx| {
                                other_app.update(cx, |app, cx| {
                                    let folder = app.tree.focused_folder.clone();
                                    if let Err(error) = app.start_create_other(
                                        folder.as_deref(),
                                        window,
                                        cx,
                                    ) {
                                        app.report(error, cx);
                                    }
                                });
                            }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn apply_name(&mut self, window: &mut Window, cx: &mut Context<Self>) -> io::Result<()> {
        let name = self.tree.name_input.read(cx).value().trim().to_owned();
        match self.tree.name_action.clone().expect("modal has action") {
            NameAction::Create => {
                let root = PathBuf::from(&self.projects.directory().expect("file creation has a directory").folder);
                files::create_file(&root, &name)?;
                self.tree.files = files::list_files(&root)?;
                self.tree.folders = files::list_folders(&root).unwrap_or_default();
                let normalized = name.trim().replace('\\', "/");
                let file = self.tree.files
                    .iter()
                    .find(|file| file.path == normalized)
                    .expect("new supported file appears in scan")
                    .clone();
                self.expand_ancestors(&file.path);
                self.open_file(&file, window, cx)?;
            }
            NameAction::CreateFolder => {
                let root = PathBuf::from(&self.projects.directory().expect("folder creation has a directory").folder);
                files::create_folder(&root, &name)?;
                self.tree.files = files::list_files(&root)?;
                self.tree.folders = files::list_folders(&root).unwrap_or_default();
                let created = name
                    .trim()
                    .trim_matches(['/', '\\'])
                    .replace('\\', "/");
                self.expand_ancestors(&created);
            }
            NameAction::RenameFile(path) => {
                self.rename_document(&path, &name, window, cx)?;
            }
            NameAction::RenameFolder(folder_path) => {
                let root = PathBuf::from(&self.projects.directory().expect("folder tree has a directory").folder);
                let renamed = files::rename_folder(&root, &folder_path, &name)?;
                if renamed != folder_path {
                    self.relocate_folder_state(&folder_path, &renamed, window, cx);
                    self.tree.files = files::list_files(&root)?;
                    self.tree.folders = files::list_folders(&root)?;
                    self.relocate_tags(&[(folder_path, renamed)], true, cx);
                    self.projects.save()?;
                }
            }
            NameAction::RenameProject(id) => {
                self.projects.rename(&id, name)?;
            }
        }
        self.remember_session()?;
        self.tree.name_action = None;
        cx.notify();
        Ok(())
    }
    fn delete_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_trash(vec![path.to_owned()], window, cx);
    }

    fn delete_folder(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.build.is_running() {
            return;
        }
        let prefix = format!("{folder}/");
        let paths = self.tree.files
            .iter()
            .filter(|file| file.path.starts_with(&prefix))
            .map(|file| file.path.clone())
            .collect();
        self.confirm_trash(paths, window, cx);
    }

    pub(super) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree.selected_files.is_empty() || self.build.is_running() {
            return;
        }
        self.confirm_trash(self.tree.selected_files.iter().cloned().collect(), window, cx);
    }

    fn confirm_trash(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() || self.build.is_running() { return; }
        let retry = paths.clone();
        if self.ask(
            Dialog::DeleteSelected,
            crate::i18n::workspace::dialog_trash_title(paths.len()),
            crate::i18n::workspace::DIALOG_TRASH_BODY.get().into(),
            DialogButtons::Confirm(crate::i18n::workspace::DIALOG_TRASH_CONFIRM.get().into()),
            cx,
            move |app, window, cx| app.confirm_trash(retry, window, cx),
        ) != Some(rfd::MessageDialogResult::Ok)
        {
            return;
        }
        self.delete_paths(paths, window, cx);
    }

    fn delete_paths(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        for path in &paths {
            if self.documents.needs_save_before_removal(path) {
                match self.save_file(path, false, cx) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        self.report(error, cx);
                        return;
                    }
                }
            }
        }
        let project = self.projects.directory().expect("files have a project").clone();
        let root = Path::new(&project.folder);
        let mut deleted = Vec::new();
        let mut failed = false;
        let batch = uuid::Uuid::new_v4().to_string();
        for path in paths {
            let result = crate::trash::put(&project.id, root, &path, &batch, project.main_file == path);
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    failed = true;
                    self.report(error, cx);
                    break;
                }
            };
            deleted.push(path.clone());
            self.tree.trash_entries.push(entry);
            self.detach_removed_document(&path, window, cx);
            self.tree.selected_files.remove(&path);
            if self.tree.focused_file.as_deref() == Some(&path) {
                self.tree.focused_file = None;
            }
        }
        if deleted.is_empty() {
            return;
        }
        self.documents.select_last_tab_if_closed();
        let main = project.main_file.clone();
        if deleted.iter().any(|path| path == &main) {
            if let Err(error) = self.projects.remove_main_file(&project.id) {
                self.report(error, cx);
            }
        }
        if let Err(error) = self.refresh_files(window, cx) {
            self.report(error, cx);
        }
        if let Err(error) = self.remember_session() {
            self.report(error, cx);
        }
        self.tree.file_tree_focus.focus(window, cx);
        if !failed {
            self.report(crate::i18n::workspace::REPORT_MOVED_TO_TRASH.get(), cx);
        }
        cx.notify();
    }

    pub(super) fn copy_file_path(
        &mut self,
        _: &CopyFilePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tree.selected_files.is_empty() {
            let paths: Vec<String> = self.tree.selected_files
                .iter()
                .map(|path| self.document_path(path).to_string_lossy().into_owned())
                .collect();
            let count = paths.len();
            cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
            if count == 1 {
                self.report(crate::i18n::workspace::REPORT_COPIED_PATH.get(), cx);
            } else {
                self.report(crate::i18n::workspace::report_copied_paths(count), cx);
            }
            return;
        }
        if let Some(folder) = &self.tree.focused_folder
            && let Some(project) = self.projects.directory()
        {
            let full = PathBuf::from(&project.folder).join(folder);
            cx.write_to_clipboard(ClipboardItem::new_string(
                full.to_string_lossy().into_owned(),
            ));
            self.report(crate::i18n::workspace::REPORT_COPIED_FOLDER_PATH.get(), cx);
            return;
        }
        if !self.documents.active().is_empty() {
            let full = self.document_path(self.documents.active());
            cx.write_to_clipboard(ClipboardItem::new_string(
                full.to_string_lossy().into_owned(),
            ));
            self.report(crate::i18n::workspace::REPORT_COPIED_PATH.get(), cx);
        }
    }

    fn file_parent_relative(path: &str) -> String {
        Path::new(path)
            .parent()
            .map(|parent| parent.to_string_lossy().replace('\\', "/"))
            .filter(|parent| !parent.is_empty() && parent != ".")
            .unwrap_or_default()
    }

    fn paste_target_folder(&self) -> String {
        if let Some(folder) = self.tree.focused_folder.as_deref().filter(|f| !f.is_empty()) {
            return folder.to_owned();
        }
        if self.tree.selected_files.len() == 1
            && let Some(file) = self.tree.selected_files.iter().next()
        {
            return Self::file_parent_relative(file);
        }
        if let Some(file) = self.tree.focused_file.as_deref().filter(|f| !f.is_empty()) {
            return Self::file_parent_relative(file);
        }
        String::new()
    }

    pub(super) fn paste_to_file_tree(
        &mut self,
        _: &PasteToFileTree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = self.paste_target_folder();
        self.paste_images_into_folder(&folder, window, cx);
    }

    pub(super) fn paste_images_into_folder(
        &mut self,
        folder_rel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.directory() else {
            return;
        };
        let Some(item) = cx.read_from_clipboard() else {
            self.report(crate::i18n::workspace::REPORT_NO_CLIPBOARD_IMAGE.get(), cx);
            return;
        };
        let root = PathBuf::from(&project.folder);
        let target_dir = if folder_rel.is_empty() {
            root.clone()
        } else {
            match files::project_path(&root, folder_rel) {
                Ok(path) => path,
                Err(error) => {
                    self.report(error, cx);
                    return;
                }
            }
        };

        let saved = match crate::image::save_clipboard_images_to_dir(&item, &target_dir) {
            Ok(saved) if saved.is_empty() => {
                self.report(crate::i18n::workspace::REPORT_NO_CLIPBOARD_IMAGE.get(), cx);
                return;
            }
            Ok(saved) => saved,
            Err(error) => {
                self.report(error, cx);
                return;
            }
        };

        self.tree.show_trash = false;
        let mut created_rel = Vec::with_capacity(saved.len());
        for path in &saved {
            if let Ok(rel) = path.strip_prefix(&root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                self.expand_ancestors(&rel_str);
                created_rel.push(rel_str);
            }
        }
        if let Err(error) = self.refresh_files(window, cx) {
            self.report(error, cx);
            return;
        }
        if !created_rel.is_empty() {
            self.tree.selected_files.clear();
            for rel in &created_rel {
                self.tree.selected_files.insert(rel.clone());
            }
            self.tree.focused_folder = None;
            self.tree.focused_file = created_rel.first().cloned();
        }
        if saved.len() == 1 {
            let name = saved[0]
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.report(crate::i18n::workspace::report_pasted_image(&name), cx);
        } else {
            self.report(crate::i18n::workspace::report_pasted_images(saved.len()), cx);
        }
        cx.notify();
    }

    pub(super) fn delete_file_tree_item(
        &mut self,
        _: &DeleteFileTreeItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.projects.directory().is_none() || self.build.is_running() {
            return;
        }
        if !self.tree.selected_files.is_empty() {
            if self.tree.selected_files.len() == 1 {
                if let Some(path) = self.tree.selected_files.iter().next().cloned() {
                    self.delete_file(&path, window, cx);
                }
            } else {
                self.delete_selected(window, cx);
            }
            return;
        }
        if let Some(folder) = self.tree.focused_folder.clone() {
            self.delete_folder(&folder, window, cx);
            return;
        }
        if !self.documents.active().is_empty() && self.tree.files.iter().any(|file| file.path == self.documents.active()) {
            let path = self.documents.active().to_owned();
            self.delete_file(&path, window, cx);
        }
    }

    pub(super) fn undo_file_delete(
        &mut self,
        _: &UndoFileDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.directory() else { return; };
        let Some(batch) = self.tree.trash_entries.iter().rev().find(|entry| entry.project_id == project.id).map(|entry| entry.batch.clone()) else { return; };
        let ids = self.tree.trash_entries.iter().filter(|entry| entry.batch == batch).map(|entry| entry.id.clone()).collect();
        self.restore_trash(ids, window, cx);
    }

    fn restore_trash(&mut self, mut ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(id) = ids.first().cloned() {
            let Some(entry) = self.tree.trash_entries.iter().find(|entry| entry.id == id).cloned() else {
                ids.remove(0);
                continue;
            };
            if !entry.project_root.is_dir() {
                self.report(crate::i18n::workspace::REPORT_TRASH_PARENT_MISSING.get(), cx);
                return;
            }
            let mut target = entry.project_root.join(&entry.relative);
            if target.exists() {
                let retry = ids.clone();
                let Some(choice) = self.ask(
                    Dialog::TrashConflict,
                    crate::i18n::workspace::DIALOG_TRASH_CONFLICT_TITLE.get().into(),
                    target.display().to_string(),
                    DialogButtons::RestoreConflict,
                    cx,
                    move |app, window, cx| app.restore_trash(retry, window, cx),
                ) else { return; };
                match choice {
                    rfd::MessageDialogResult::Yes => {
                        if self.projects.directory().is_some_and(|project| project.id == entry.project_id && Path::new(&project.folder) == entry.project_root) {
                            self.delete_paths(vec![entry.relative.clone()], window, cx);
                            if target.exists() { return; }
                        } else {
                            let batch = uuid::Uuid::new_v4().to_string();
                            match crate::trash::put(&entry.project_id, &entry.project_root, &entry.relative, &batch, false) {
                                Ok(replaced) => self.tree.trash_entries.push(replaced),
                                Err(error) => { self.report(error, cx); return; }
                            }
                        }
                    }
                    rfd::MessageDialogResult::No => {
                        let original = target.clone();
                        let stem = original.file_stem().expect("file name").to_string_lossy();
                        let extension = original.extension().map(|value| format!(".{}", value.to_string_lossy())).unwrap_or_default();
                        let mut number = 1;
                        while target.exists() {
                            target = original.with_file_name(format!("{stem} ({number}){extension}"));
                            number += 1;
                        }
                    }
                    _ => { ids.remove(0); continue; }
                }
            }
            let result = std::fs::create_dir_all(target.parent().expect("file parent"))
                .map_err(Into::into).and_then(|_| crate::trash::restore(&entry, &target));
            if let Err(error) = result { self.report(error, cx); return; }
            self.tree.trash_entries.retain(|item| item.id != id);
            if entry.was_main && target == entry.project_root.join(&entry.relative) {
                if let Err(error) = self.projects.restore_main_file(&entry.project_id, &entry.relative) {
                    self.report(error, cx);
                }
            }
            ids.remove(0);
        }
        if let Err(error) = self.refresh_files(window, cx) { self.report(error, cx); }
        cx.notify();
    }

    fn purge_trash(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids.is_empty() { return; }
        let all = ids.len() > 1 || ids.len() == self.tree.trash_entries.len();
        let retry = ids.clone();
        if self.ask(
            if all { Dialog::TrashClear } else { Dialog::TrashPurge },
            crate::i18n::workspace::dialog_purge_title(ids.len()),
            crate::i18n::workspace::DIALOG_PURGE_BODY.get().into(),
            DialogButtons::Confirm(crate::i18n::workspace::DIALOG_PURGE_CONFIRM.get().into()),
            cx,
            move |app, _, cx| app.purge_trash(retry, cx),
        ) != Some(rfd::MessageDialogResult::Ok) { return; }
        for id in ids {
            if let Some(entry) = self.tree.trash_entries.iter().find(|entry| entry.id == id).cloned() {
                if let Err(error) = crate::trash::purge(&entry) { self.report(error, cx); return; }
                self.tree.trash_entries.retain(|entry| entry.id != id);
            }
        }
        cx.notify();
    }

    pub(super) fn find_file(&mut self, _: &FindFile, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.sidebar_open = true;
        self.tree.searching = !self.tree.searching;
        if self.tree.searching {
            self.tree.search_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn render_trash(&self, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.tree.trash_entries.iter().rev().map(|entry| {
            let restore = entry.id.clone();
            let purge = entry.id.clone();
            let date = |timestamp: u64| chrono::DateTime::from_timestamp(timestamp as i64, 0)
                .map(|date| date.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            div().flex().items_center().gap_3().px_4().py_3().border_b_1().border_color(rgb(BORDER))
                .child(crate::ui::file_icon(&entry.relative, false, false))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(div().text_ellipsis().child(Path::new(&entry.relative).file_name().expect("file name").to_string_lossy().into_owned()))
                    .child(div().text_xs().text_color(rgb(MUTED)).text_ellipsis().child(entry.project_root.join(&entry.relative).display().to_string()))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(crate::i18n::workspace::trash_dates(&date(entry.deleted_at), &date(entry.deleted_at + 7 * 24 * 3600)))))
                .child(crate::ui::button_secondary(SharedString::from(format!("trash-restore-{}", entry.id)), crate::i18n::workspace::TRASH_RESTORE.get())
                    .on_click(cx.listener(move |app, _, window, cx| app.restore_trash(vec![restore.clone()], window, cx))))
                .child(crate::ui::button_danger_soft(SharedString::from(format!("trash-purge-{}", entry.id)), crate::i18n::workspace::TRASH_PURGE.get())
                    .on_click(cx.listener(move |app, _, _, cx| app.purge_trash(vec![purge.clone()], cx))))
        });
        div().size_full().flex().flex_col().bg(rgb(0xffffff))
            .child(div().flex().items_center().justify_between().px_5().py_4().border_b_1().border_color(rgb(BORDER))
                .child(div().font_semibold().child(crate::i18n::workspace::trash_title(self.tree.trash_entries.len())))
                .child(crate::ui::button_danger_soft("trash-clear", crate::i18n::workspace::TRASH_CLEAR.get())
                    .on_click(cx.listener(|app, _, _, cx| {
                        let ids = app.tree.trash_entries.iter().map(|entry| entry.id.clone()).collect();
                        app.purge_trash(ids, cx);
                    }))))
            .child(div().id("trash-list").flex_1().min_h_0().overflow_y_scroll()
                .when(self.tree.trash_entries.is_empty(), |area| area.p_5().text_color(rgb(MUTED)).child(crate::i18n::workspace::TRASH_EMPTY.get()))
                .children(entries))
            .into_any_element()
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        if !cx.has_active_drag() { self.tree.dragging_tree_item.store(false, Ordering::Relaxed); }
        let dragging_tree_item = cx.has_active_drag() && self.tree.dragging_tree_item.load(Ordering::Relaxed);
        let user_project = self.projects.active_id().is_some();
        let projects = self.projects.registered().iter().map(|project| {
            let id = project.id.clone();
            let rename_id = id.clone();
            let name = project.name.clone();
            let root = PathBuf::from(&project.folder);
            let app = cx.entity();
            div()
                .id(SharedString::from(format!("project-{}", project.id)))
                .flex()
                .items_center()
                .gap_2()
                .mx_2()
                .px_2()
                .h(px(32.))
                .flex_shrink_0()
                .rounded(px(4.))
                .cursor_pointer()
                .when(!self.tree.show_trash && Some(project.id.as_str()) == self.projects.active_id(), |row| {
                    row.bg(rgb(0xe8e7e4))
                })
                .hover(|style| style.bg(rgb(0xefefed)))
                .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                .child(crate::ui::icon(IconName::Folder))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(project.name.clone()),
                )
                .on_click(
                    cx.listener(move |app, _, window, cx| { app.tree.show_trash = false; app.switch_project(&id, window, cx); cx.notify(); }),
                )
                .context_menu(move |menu, _, _| {
                    let root = root.clone();
                    let name = name.clone();
                    let id = rename_id.clone();
                    let app = app.clone();
                    let remove_app = app.clone();
                    let remove_id = id.clone();
                    menu.min_w(px(90.))
                        .item(
                            compact_menu_item(crate::i18n::workspace::MENU_REVEAL_IN_EXPLORER.get())
                                .on_click(move |_, _, cx| reveal_path(&root, cx)),
                        )
                        .item(
                            compact_menu_item(crate::i18n::workspace::MENU_RENAME_PROJECT.get()).on_click(move |_, window, cx| {
                                app.update(cx, |app, cx| {
                                    app.start_name(
                                        NameAction::RenameProject(id.clone()),
                                        &name,
                                        window,
                                        cx,
                                    )
                                })
                            }),
                        )
                        .separator()
                        .item(
                            compact_menu_item(crate::i18n::workspace::MENU_REMOVE_PROJECT.get()).on_click(move |_, window, cx| {
                                remove_app
                                    .update(cx, |app, cx| app.remove_project(&remove_id, window, cx))
                            }),
                        )
                })
        });
        let query = if self.tree.searching {
            self.tree.search_input.read(cx).value().to_lowercase()
        } else {
            String::new()
        };
        let mut entries = BTreeMap::new();
        if self.projects.directory().is_none() {
            let mut paths: BTreeSet<_> = self.documents.tabs().iter().collect();
            if self.tags.filter.is_some() {
                paths.extend(self.tags.snapshot.assigned.keys());
            }
            for path in paths {
                if self.tag_matches(path) && path.to_lowercase().contains(&query) {
                    entries.insert(path.clone(), false);
                }
            }
        } else {
            for folder in self.tree.folders
                .iter()
                .filter(|folder| self.tags.filter.is_none() && folder.to_lowercase().contains(&query))
            {
                let parts: Vec<_> = folder.split('/').collect();
                for depth in 1..=parts.len() {
                    entries.insert(parts[..depth].join("/"), true);
                }
            }
            for file in self.tree.files
                .iter()
                .filter(|file| self.tag_matches(&file.path) && file.path.to_lowercase().contains(&query))
            {
                let parts: Vec<_> = file.path.split('/').collect();
                for depth in 1..parts.len() {
                    entries.insert(parts[..depth].join("/"), true);
                }
                entries.insert(file.path.clone(), false);
            }
        }
        let no_tag_results = self.tags.filter.is_some() && entries.is_empty();
        let files_by_path: HashMap<_, _> = self
            .tree
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.path.as_str(), index))
            .collect();
        let mut entries: Vec<_> = entries.iter().collect();
        // Group sibling files as PDF, Markdown, TeX sources/support, then other
        // extensions. Component-wise keys keep folders first and children together.
        entries.sort_by_cached_key(|(path, folder)| {
            let mut parts = path.split('/').peekable();
            let mut key = Vec::new();
            while let Some(part) = parts.next() {
                let name = part.to_lowercase();
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                let group = if parts.peek().is_some() || **folder {
                    0
                } else if name == ".latexmkrc" {
                    3
                } else {
                    match extension {
                        "pdf" => 1,
                        "md" => 2,
                        "tex" | "ltx" | "bib" | "sty" | "cls" | "bst" | "bbx" | "cbx"
                        | "cfg" | "def" | "clo" | "ist" | "glsdefs" | "tikz" | "pgf"
                        | "asy" => 3,
                        _ => 4,
                    }
                };
                key.push((
                    group,
                    if group == 4 { extension.to_owned() } else { String::new() },
                    name,
                    part.to_owned(),
                ));
            }
            key
        });
        self.tree.tree_row_bounds.lock().expect("tree bounds").clear();
        let rows = entries
            .into_iter()
            .filter(|(path, _)| {
                !query.is_empty() || self.tags.filter.is_some()
                    || !self.tree.collapsed_folders
                        .iter()
                        .any(|folder| path.starts_with(&format!("{folder}/")))
            })
            .map(|(path, folder)| {
                let row_path = path.clone();
                let is_file = !*folder && self.projects.directory().is_some();
                let row = (|| {
                let depth = if self.projects.directory().is_some() {
                    path.matches('/').count()
                } else {
                    0
                };
                if self.projects.directory().is_none() {
                    let key = path.clone();
                    let active = self.documents.active() == key;
                    let close = key.clone();
                    let rename_key = key.clone();
                    let rename_name = Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
                    let rename_app = cx.entity();
                    let tags_app = rename_app.clone();
                    let tag_path = key.clone();
                    let is_open = self.documents.tabs().contains(&key);
                    let full = PathBuf::from(path);
                    let reveal = full.clone();
                    let copy = full.clone();
                    let preview_font = crate::ui::sans_font(&self.settings.interface_font);
                    let preview_font_size = self.settings.interface_font_size;
                    let trash_bounds = self.tree.trash_bounds.clone();
                    let dragging = self.tree.dragging_tree_item.clone();
                    let folder_bounds = self.tree.tree_row_bounds.clone();
                    let close_id = SharedString::from(format!("close-default-file-{key}"));
                    let name = Path::new(path)
                        .file_name()
                        .expect("standalone file has a name")
                        .to_string_lossy()
                        .into_owned();
                    return div()
                        .id(SharedString::from(format!("default-file-{key}")))
                        .on_drag(crate::terminal::DraggedPaths(vec![full], None), move |paths, offset, _, cx| {
                            dragging.store(true, Ordering::Relaxed);
                            cx.new(|_| crate::terminal::PathsPreview { paths: paths.0.clone(), offset, font: preview_font.clone(), font_size: preview_font_size, trash_bounds: trash_bounds.clone(), is_folder: false, source_folder: None, can_trash: false, folder_bounds: folder_bounds.clone() })
                        })
                        .flex()
                        .items_center()
                        .h(px(32.))
                        .px_3()
                        .gap_2()
                        .cursor_pointer()
                        .when(active, |row| row.bg(rgb(0xe8e7e4)))
                        .hover(|style| style.bg(rgb(0xefefed)))
                        .child(crate::ui::file_icon(&key, false, false))
                        .child(div().flex_1().min_w_0().text_ellipsis().child(name))
                        .children(self.render_file_tags(&key, depth, cx))
                        .when(is_open, |row| row.child(
                            crate::ui::control(close_id, IconName::Close).on_click(cx.listener(
                                move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_tab(&close, window, cx);
                                },
                            )),
                        ))
                        .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            app.tree.show_trash = false;
                            if !app.documents.tabs().contains(&key) {
                                if let Err(error) = app.open_standalone_file(Path::new(&key), window, cx) {
                                    app.report(error, cx);
                                }
                                return;
                            }
                            app.select_tab_from_tree(&key, event.click_count() >= 2, window, cx);
                        }))
                        .context_menu(move |menu, _, _| {
                            let app = rename_app.clone();
                            let path = rename_key.clone();
                            let name = rename_name.clone();
                            let reveal = reveal.clone();
                            let copy = copy.clone();
                            let tags_app = tags_app.clone();
                            let tag_path = tag_path.clone();
                            menu.min_w(px(90.))
                                .item(compact_menu_item(crate::i18n::workspace::MENU_REVEAL_IN_EXPLORER.get()).on_click(move |_, _, cx| reveal_path(&reveal, cx)))
                                .item(compact_menu_item(crate::i18n::workspace::MENU_COPY_FULL_PATH.get()).on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string_lossy().into_owned()));
                                }))
                                .item(compact_menu_item(crate::i18n::workspace::MENU_EDIT_TAGS.get()).on_click(move |_, window, cx| {
                                    tags_app.update(cx, |app, cx| app.edit_file_tags(vec![tag_path.clone()], window, cx));
                                }))
                                .when(path.to_lowercase().ends_with(".md"), |menu| {
                                    menu.item(compact_menu_item(crate::i18n::workspace::MENU_RENAME.get()).on_click(move |_, window, cx| {
                                        app.update(cx, |app, cx| app.start_name(NameAction::RenameFile(path.clone()), &name, window, cx));
                                    }))
                                })
                        })
                        .into_any_element();
                }
                if *folder {
                    let toggled = path.clone();
                    let is_focused = self.tree.focused_folder.as_deref() == Some(path);
                    let full = PathBuf::from(&self.projects.directory().expect("folder has project").folder)
                        .join(path);
                    let deleted = path.clone();
                    let app = cx.entity();
                    let folder_drag = crate::terminal::DraggedFolder(
                        path.clone(),
                        self.projects.directory().expect("folder has project").id.clone(),
                        full.clone(),
                    );
                    let folder_font = crate::ui::sans_font(&self.settings.interface_font);
                    let folder_font_size = self.settings.interface_font_size;
                    let trash_bounds = self.tree.trash_bounds.clone();
                    let dragging = self.tree.dragging_tree_item.clone();
                    let drop_folder = path.clone();
                    let folder_bounds = self.tree.tree_row_bounds.clone();
                    let drop_project = self.projects.directory().expect("folder has project").id.clone();
                    let folder_project = drop_project.clone();
                    let folder_target = path.clone();
                    let folder_highlight = path.clone();
                    return div()
                        .id(SharedString::from(format!("folder-{path}")))
                        .on_drag(folder_drag, move |dragged, offset, _, cx| {
                            dragging.store(true, Ordering::Relaxed);
                            cx.new(|_| crate::terminal::PathsPreview { paths: vec![dragged.2.clone()], offset, font: folder_font.clone(), font_size: folder_font_size, trash_bounds: trash_bounds.clone(), is_folder: true, source_folder: Some(dragged.0.clone()), can_trash: true, folder_bounds: folder_bounds.clone() })
                        })
                        .drag_over::<crate::terminal::DraggedPaths>(move |style, dragged, _, _| {
                            if dragged.1.as_deref() == Some(drop_project.as_str()) { style.bg(rgb(0xdde9f6)) } else { style }
                        })
                        .on_drop(cx.listener(move |app, dragged: &crate::terminal::DraggedPaths, window, cx| {
                            cx.stop_propagation();
                            if let Err(error) = app.move_tree_files(dragged, &drop_folder, window, cx) { app.report(error, cx); }
                        }))
                        .drag_over::<crate::terminal::DraggedFolder>(move |style, dragged, _, _| {
                            if dragged.1 == folder_project && !Path::new(&folder_highlight).starts_with(&dragged.0) {
                                style.bg(rgb(0xdde9f6))
                            } else { style }
                        })
                        .on_drop(cx.listener(move |app, dragged: &crate::terminal::DraggedFolder, window, cx| {
                            cx.stop_propagation();
                            if let Err(error) = app.move_tree_folder(dragged, &folder_target, window, cx) { app.report(error, cx); }
                        }))
                        .flex()
                        .h(px(29.))
                        .items_center()
                        .gap_2()
                        .pl(px(12. + depth as f32 * 15.))
                        .cursor_pointer()
                        .when(is_focused, |row| row.bg(rgb(0xedf2fa)))
                        .hover(|style| style.bg(rgb(0xefefed)))
                        .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                        .child(crate::ui::icon(if self.tags.filter.is_none() && self.tree.collapsed_folders.contains(path) {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        }))
                        .child(crate::ui::icon(if self.tags.filter.is_none() && self.tree.collapsed_folders.contains(path) {
                            IconName::Folder
                        } else {
                            IconName::FolderOpen
                        }))
                        .child(
                            div().flex_1().min_w_0().text_ellipsis().child(
                                Path::new(path)
                                    .file_name()
                                    .expect("folder has name")
                                    .to_string_lossy()
                                    .into_owned(),
                            ),
                        )
                        .on_click(cx.listener(move |app, _, window, cx| {
                            cx.stop_propagation();
                            app.tree.show_trash = false;
                            app.tree.focused_folder = Some(toggled.clone());
                            app.tree.focused_file = None;
                            if app.tags.filter.is_none() {
                                if !app.tree.collapsed_folders.remove(&toggled) {
                                    app.tree.collapsed_folders.insert(toggled.clone());
                                }
                                let _ = app.remember_session();
                            }
                            app.tree.file_tree_focus.focus(window, cx);
                            cx.notify();
                        }))
                        .context_menu(move |menu, _, _| {
                            let full = full.clone();
                            let deleted = deleted.clone();
                            let renamed_path = deleted.clone();
                            let renamed_name = Path::new(&deleted)
                                .file_name()
                                .expect("folder has name")
                                .to_string_lossy()
                                .into_owned();
                            let folder_prefix = format!("{deleted}/");
                            let folder_prefix_dir = folder_prefix.clone();
                            let app = app.clone();
                            let rename_app = app.clone();
                            let create_file_app = app.clone();
                            let create_folder_app = app.clone();
                            let paste_app = app.clone();
                            let paste_folder = deleted.clone();
                            let tag_folder = deleted.clone();
                            let tags_app = app.clone();
                            let menu = CREATABLE_PRESETS.iter().fold(menu.min_w(px(130.)), |menu, preset| {
                                let preset_app = app.clone();
                                let preset_folder = deleted.clone();
                                let ext = preset.ext;
                                menu.item(compact_menu_item(preset.folder_label.get()).on_click(
                                    move |_, window, cx| {
                                        preset_app.update(cx, |app, cx| {
                                            app.tree.focused_folder = Some(preset_folder.clone());
                                            if let Err(error) = app.new_preset_in(
                                                ext,
                                                Some(&preset_folder),
                                                window,
                                                cx,
                                            ) {
                                                app.report(error, cx);
                                            }
                                        });
                                    },
                                ))
                            });
                            menu.item(compact_menu_item(crate::i18n::workspace::MENU_NEW_FILE.get()).on_click(
                                    move |_, window, cx| {
                                        create_file_app.update(cx, |app, cx| {
                                            app.start_name(
                                                NameAction::Create,
                                                &folder_prefix,
                                                window,
                                                cx,
                                            )
                                        })
                                    },
                                ))
                                .item(compact_menu_item(crate::i18n::workspace::MENU_NEW_FOLDER.get()).on_click(
                                    move |_, window, cx| {
                                        create_folder_app.update(cx, |app, cx| {
                                            app.start_name(
                                                NameAction::CreateFolder,
                                                &folder_prefix_dir,
                                                window,
                                                cx,
                                            )
                                        })
                                    },
                                ))
                                .item(compact_menu_item(crate::i18n::workspace::MENU_PASTE_IMAGE.get()).on_click(
                                    move |_, window, cx| {
                                        paste_app.update(cx, |app, cx| {
                                            app.paste_images_into_folder(&paste_folder, window, cx)
                                        })
                                    },
                                ))
                                .separator()
                                .item(
                                    compact_menu_item(crate::i18n::workspace::MENU_REVEAL_IN_EXPLORER.get())
                                        .on_click(move |_, _, cx| reveal_path(&full, cx)),
                                )
                                .item(compact_menu_icon_item(IconName::Tag, crate::i18n::workspace::MENU_BATCH_FOLDER_TAGS.get())
                                    .on_click(move |_, window, cx| {
                                        tags_app.update(cx, |app, cx| app.edit_folder_tags(&tag_folder, window, cx));
                                    }))
                                .separator()
                                .item(
                                    compact_menu_item(crate::i18n::workspace::MENU_RENAME.get()).on_click(move |_, window, cx| {
                                        rename_app.update(cx, |app, cx| {
                                            app.start_name(
                                                NameAction::RenameFolder(renamed_path.clone()),
                                                &renamed_name,
                                                window,
                                                cx,
                                            )
                                        })
                                    }),
                                )
                                .item(
                                    compact_menu_item(crate::i18n::workspace::MENU_DELETE_FOLDER.get()).on_click(move |_, window, cx| {
                                        app.update(cx, |app, cx| {
                                            app.delete_folder(&deleted, window, cx)
                                        })
                                    }),
                                )
                        })
                        .into_any_element();
                }
                let file = self.tree.files[*files_by_path
                    .get(path.as_str())
                    .expect("tree entry is a listed file")]
                    .clone();
                let open = file.clone();
                let rename = file.path.clone();
                let delete = file.path.clone();
                let full = PathBuf::from(&self.projects.directory().expect("file has project").folder)
                    .join(&file.path);
                let app = cx.entity();
                let split = file.clone();
                let row_split = file.clone();
                let selected_path = file.path.clone();
                let clicked_path = file.path.clone();
                let name = Path::new(&file.path)
                    .file_name()
                    .expect("file has name")
                    .to_string_lossy()
                    .to_string();
                let is_selected = self.tree.selected_files.contains(path);
                let batch_tags = is_selected && self.tree.selected_files.len() > 1;
                let dragged_paths = if is_selected {
                    self.tree.selected_files.iter().map(|path| self.document_path(path)).collect()
                } else {
                    vec![full.clone()]
                };
                let preview_font = crate::ui::sans_font(&self.settings.interface_font);
                let preview_font_size = self.settings.interface_font_size;
                let drag_project = self.projects.directory().expect("file has project").id.clone();
                let trash_bounds = self.tree.trash_bounds.clone();
                let dragging = self.tree.dragging_tree_item.clone();
                let folder_bounds = self.tree.tree_row_bounds.clone();
                div()
                    .id(SharedString::from(format!("file-{}", file.path)))
                    .on_drag(crate::terminal::DraggedPaths(dragged_paths, Some(drag_project)), move |paths, offset, _, cx| {
                        dragging.store(true, Ordering::Relaxed);
                        cx.new(|_| crate::terminal::PathsPreview { paths: paths.0.clone(), offset, font: preview_font.clone(), font_size: preview_font_size, trash_bounds: trash_bounds.clone(), is_folder: false, source_folder: None, can_trash: true, folder_bounds: folder_bounds.clone() })
                    })
                    .flex()
                    .items_center()
                    .h(px(29.))
                    .pl(px(14. + depth as f32 * 15.))
                    .pr_2()
                    .gap_2()
                    .cursor_pointer()
                    .when(is_selected, |row| row.bg(rgb(0xdde9f6)))
                    .when(!is_selected && self.documents.active() == file.path, |row| row.bg(rgb(0xe8e7e4)))
                    .hover(|style| style.bg(rgb(0xefefed)))
                    .active(|style| style.bg(rgb(crate::ui::PRESSED_BG)))
                    .child(
                        div()
                            .id(SharedString::from(format!("select-{path}")))
                            .w(px(15.))
                            .h(px(15.))
                            .rounded(px(3.5))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(is_selected, |s| {
                                s.bg(rgb(0x165dff))
                                    .border_1()
                                    .border_color(rgb(0x165dff))
                                    .text_color(crate::theme::on_accent())
                                    .active(|a| a.bg(rgb(0x0e42d2)).border_color(rgb(0x0e42d2)))
                                    .child(crate::ui::icon(IconName::Check).w(px(10.)).h(px(10.)))
                            })
                            .when(!is_selected, |s| {
                                s.bg(rgb(0xffffff))
                                    .border_1()
                                    .border_color(rgb(0xc4c7cc))
                                    .hover(|h| h.border_color(rgb(0x165dff)).bg(rgb(0xf2f7ff)))
                                    .active(|a| a.border_color(rgb(0x0e42d2)).bg(rgb(0xdde9f6)))
                            })
                            .on_click(cx.listener(move |app, _, window, cx| {
                                cx.stop_propagation();
                                if !app.tree.selected_files.remove(&selected_path) {
                                    app.tree.selected_files.insert(selected_path.clone());
                                }
                                app.tree.focused_folder = None;
                                app.tree.focused_file = Some(selected_path.clone());
                                app.tree.file_tree_focus.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(crate::ui::file_icon(
                        &file.path,
                        file.kind == FileKind::Asset,
                        self.projects.directory()
                            .is_some_and(|project| project.main_file == file.path),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .when(
                                self.projects.directory()
                                    .is_some_and(|project| project.main_file == file.path),
                                |name| {
                                    name.text_color(rgb(crate::ui::MAIN_TEX_COLOR))
                                        .font_semibold()
                                },
                            )
                            .child(name.clone()),
                    )
                    .children(self.render_file_tags(path, depth, cx))
                    .when(
                        Self::supports_split_pane(&file.path),
                        |row| {
                            row.child(
                                crate::ui::control(
                                    SharedString::from(format!("split-{path}")),
                                    IconName::Columns2,
                                )
                                .on_click(cx.listener(
                                    move |app, _, window, cx| {
                                        cx.stop_propagation();
                                        app.open_reading_pane(&row_split, window, cx);
                                    },
                                )),
                            )
                        },
                    )
                    .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        app.tree.show_trash = false;
                        if event.modifiers().control {
                            if !app.tree.selected_files.remove(&clicked_path) {
                                app.tree.selected_files.insert(clicked_path.clone());
                            }
                            app.tree.focused_folder = None;
                            app.tree.focused_file = Some(clicked_path.clone());
                            app.tree.file_tree_focus.focus(window, cx);
                            cx.notify();
                            return;
                        }
                        app.tree.focused_folder = None;
                        app.tree.focused_file = Some(open.path.clone());
                        if let Err(error) =
                            app.open_file_with_focus(&open, event.click_count() >= 2, window, cx)
                        {
                            app.report(error, cx);
                        }
                        if event.click_count() < 2 {
                            app.tree.file_tree_focus.focus(window, cx);
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let reveal = full.clone();
                        let copied = full.to_string_lossy().to_string();
                        let rename = rename.clone();
                        let name = name.clone();
                        let delete = delete.clone();
                        let rename_app = app.clone();
                        let delete_app = app.clone();
                        let main_app = app.clone();
                        let main = rename.clone();
                        let split = split.clone();
                        let split_app = app.clone();
                        let paste_app = app.clone();
                        let paste_folder = Self::file_parent_relative(&rename);
                        let tags_app = app.clone();
                        let batch_tags_app = app.clone();
                        let tag_path = rename.clone();
                        menu.min_w(px(90.))
                            .item(
                                compact_menu_item(crate::i18n::workspace::MENU_REVEAL_IN_EXPLORER.get())
                                    .on_click(move |_, _, cx| reveal_path(&reveal, cx)),
                            )
                            .item(
                                compact_menu_item(crate::i18n::workspace::MENU_COPY_FULL_PATH.get()).on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                                }),
                            )
                            .item(
                                compact_menu_item(crate::i18n::workspace::MENU_PASTE_IMAGE.get()).on_click(move |_, window, cx| {
                                    paste_app.update(cx, |app, cx| {
                                        app.paste_images_into_folder(&paste_folder, window, cx)
                                    })
                                }),
                            )
                            .when(batch_tags, |menu| {
                                menu.item(compact_menu_icon_item(IconName::Tag, crate::i18n::workspace::MENU_BATCH_TAGS.get())
                                    .on_click(move |_, window, cx| {
                                        batch_tags_app.update(cx, |app, cx| {
                                            let paths = app.tree.selected_files.iter().cloned().collect();
                                            app.edit_file_tags(paths, window, cx);
                                        });
                                    }))
                            })
                            .item(compact_menu_item(crate::i18n::workspace::MENU_EDIT_FILE_TAGS.get()).on_click(move |_, window, cx| {
                                tags_app.update(cx, |app, cx| app.edit_file_tags(vec![tag_path.clone()], window, cx));
                            }))
                            .when(
                                Self::supports_split_pane(&split.path),
                                |menu| {
                                    menu.item(compact_menu_item(crate::i18n::workspace::MENU_OPEN_IN_SPLIT.get()).on_click(
                                        move |_, window, cx| {
                                            split_app.update(cx, |app, cx| {
                                                app.open_reading_pane(&split, window, cx)
                                            })
                                        },
                                    ))
                                },
                            )
                            .when(user_project && main.to_lowercase().ends_with(".tex"), |menu| {
                                menu.item(compact_menu_item(crate::i18n::workspace::MENU_SET_MAIN_FILE.get()).on_click(
                                    move |_, _, cx| {
                                        main_app.update(cx, |app, cx| {
                                            app.set_main_file(&main, cx);
                                        })
                                    },
                                ))
                            })
                            .separator()
                            .item(
                                compact_menu_item(crate::i18n::workspace::MENU_RENAME.get()).on_click(move |_, window, cx| {
                                    rename_app.update(cx, |app, cx| {
                                        app.start_name(
                                            NameAction::RenameFile(rename.clone()),
                                            &name,
                                            window,
                                            cx,
                                        )
                                    })
                                }),
                            )
                            .item(compact_menu_item(crate::i18n::workspace::MENU_DELETE.get()).on_click(move |_, window, cx| {
                                delete_app.update(cx, |app, cx| app.delete_file(&delete, window, cx))
                            }))
                    })
                    .into_any_element()
                })();
                let bounds = self.tree.tree_row_bounds.clone();
                div().id(SharedString::from(format!("tree-row-{row_path}"))).relative()
                    // The child handles file dragging; blank-space presses belong to marquee selection.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(row).child(
                    canvas(move |area, _, _| {
                        bounds.lock().expect("tree bounds").insert(row_path.clone(), (is_file, area));
                    }, |_, _, _, _| {}).absolute().top_0().left_0().size_full(),
                )
            });
        let default_space = div()
            .id("default-space")
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .px_2()
            .h(px(32.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(!self.tree.show_trash && self.projects.is_default(), |row| row.bg(rgb(0xe8e7e4)))
            .hover(|style| style.bg(rgb(0xefefed)))
            .child(crate::ui::icon(IconName::NotebookPen))
            .child(div().flex_1().child(crate::i18n::workspace::DEFAULT_SPACE.get()))
            .child(crate::ui::control("default-new-markdown", IconName::Plus)
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(crate::i18n::workspace::TOOLTIP_NEW_MD_DEFAULT.get()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|app, _, window, cx| {
                    cx.stop_propagation();
                    if let Err(error) = app.new_default_markdown(window, cx) { app.report(error, cx); }
                })))
            .on_click(cx.listener(|app, _, window, cx| { app.tree.show_trash = false; app.switch_to_default(window, cx); cx.notify(); }));
        let loose_count = if self.projects.directory().is_none() { self.documents.tabs().len() } else { self.projects.loose_tab_count() };
        div()
            .w_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0xfbfbfa))
            .font_family(crate::ui::sans_font(&self.settings.interface_font))
            .text_size(px(self.settings.interface_font_size as f32))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(div().pt_2().child(default_space))
            .child(div().id("loose-files").flex().items_center().gap_2()
                .mx_2().px_2().h(px(32.)).rounded(px(4.)).cursor_pointer()
                .when(!self.tree.show_trash && self.projects.directory().is_none(), |row| row.bg(rgb(0xe8e7e4)))
                .hover(|style| style.bg(rgb(0xefefed)))
                .child(crate::ui::icon(IconName::Files))
                .child(div().flex_1().child(crate::i18n::workspace::LOOSE_FILES.get()))
                .when(loose_count > 0, |row| row.pr_1().child(div().text_xs().text_color(rgb(MUTED)).child(loose_count.to_string())))
                .on_click(cx.listener(|app, _, window, cx| {
                app.tree.show_trash = false;
                if app.projects.directory().is_none() { return; }
                if app.build.is_busy() || !app.confirm_drafts(cx, |app, window, cx| {
                    if let Err(error) = app.select_loose_files(window, cx) { app.report(error, cx); }
                }) { return; }
                if let Err(error) = app.select_loose_files(window, cx) { app.report(error, cx); }
            })))
            .child(div().relative().flex_1().min_h_0().overflow_hidden().child(
                div().id("sidebar-content-scroll").flex().flex_col().size_full().overflow_y_scroll()
                .track_scroll(&self.tree.sidebar_scroll)
                .child(div().pt_5().pb_2().px_4().flex_shrink_0().text_xs().text_color(rgb(MUTED)).child(crate::i18n::workspace::PROJECTS_HEADER.get()))
                .children(projects)
                .child(div().id("add-project").flex().flex_shrink_0().items_center().gap_2().mx_2().px_2().h(px(32.))
                    .rounded(px(4.)).cursor_pointer().text_color(rgb(MUTED))
                    .hover(|style| style.bg(rgb(0xefefed)))
                    .child(crate::ui::icon(IconName::Plus)).child(crate::i18n::workspace::ADD_LOCAL_PROJECT.get())
                    .on_click(cx.listener(|app, _, window, cx| app.add_project(&OpenProject, window, cx))))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .pt_2()
                    .flex_shrink_0()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(crate::i18n::workspace::FILES_HEADER.get())
                    .child(self.render_tag_toggle(cx))
                    .child(self.render_new_file_split_button(cx))
                    .when(self.projects.directory().is_none(), |header| header.child(button("open-file", crate::i18n::workspace::OPEN_FILE.get()).on_click(
                        cx.listener(|app, _, window, cx| app.open_file_dialog(window, cx)),
                    )))
                    .when(self.projects.directory().is_some(), |header| {
                        header.child(
                            div()
                                .flex()
                                .gap_1()
                                .child(crate::ui::control("refresh-files", IconName::RotateCcw)
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(crate::i18n::workspace::REFRESH_FILES_TOOLTIP.get())
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        match app.refresh_all_files(window, cx) {
                                            Ok(()) => {
                                                app.tree.selected_files.retain(|path| {
                                                    app.tree.files.iter().any(|file| &file.path == path)
                                                });
                                                app.report(crate::i18n::workspace::FILES_REFRESHED.get(), cx);
                                            }
                                            Err(error) => app.report(error, cx),
                                        }
                                    })))
                                .child(button("new-folder", "📁+").on_click(cx.listener(
                                    |app, _, window, cx| {
                                        let prefix = app.focused_folder_prefix();
                                        app.start_name(NameAction::CreateFolder, &prefix, window, cx)
                                    },
                                ))),
                        )
                    })
                    .child(button("find-file", "⌕").on_click(cx.listener(
                        |app, _, window, cx| app.find_file(&FindFile, window, cx),
                    ))),
            )
            .when(self.tree.searching, |sidebar| {
                sidebar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .pb_2()
                        .child(
                            Input::new(&self.tree.search_input)
                                .xsmall()
                                .h(px(22.))
                                .text_size(px(12.))
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            crate::ui::control("close-file-search", IconName::Close)
                                .size(px(20.))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.tree.searching = false;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .children(self.render_tag_filter(cx))
            .when(!self.tags.is_editing(), |sidebar| sidebar.children(self.render_tag_panel(cx)))
            .when(!self.tree.selected_files.is_empty(), |sidebar| {
                let sel_count = self.tree.selected_files.len();
                sidebar.child(
                    div()
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .bg(rgb(0xf7f6f3))
                        .border_1()
                        .border_color(rgb(0xe8e7e3))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1p5()
                        .min_w_0()
                        .child(
                            div().flex().items_center().gap_1().min_w_0().child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x787774))
                                    .text_ellipsis()
                                    .child(crate::i18n::workspace::selected_count(sel_count)),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_1()
                                .child(crate::ui::control("batch-tags", IconName::Tag)
                                    .size(px(24.))
                                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(crate::i18n::workspace::BATCH_EDIT_TAGS_TOOLTIP.get()).build(window, cx))
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.edit_file_tags(app.tree.selected_files.iter().cloned().collect(), window, cx);
                                    })))
                                .child(
                                    crate::ui::button_secondary("batch-compile", "")
                                        .w(px(24.))
                                        .min_h(px(22.))
                                        .px_0()
                                        .py_0()
                                        .child(
                                            crate::ui::icon(IconName::Play).w(px(11.)).h(px(11.)),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                crate::i18n::workspace::TOOLTIP_BATCH_COMPILE.get(),
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.compile_selected(window, cx)
                                        })),
                                )
                                .child(
                                    crate::ui::button_danger_soft("batch-delete", "")
                                        .w(px(24.))
                                        .min_h(px(22.))
                                        .px_0()
                                        .py_0()
                                        .child(
                                            crate::ui::icon(IconName::Trash).w(px(12.)).h(px(12.)),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                crate::i18n::common::DELETE.get(),
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.delete_selected(window, cx)
                                        })),
                                )
                                .child(
                                    crate::ui::control("clear-selection", IconName::Close)
                                        .size(px(24.))
                                        .tooltip(|window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                crate::i18n::workspace::TOOLTIP_CLEAR_SELECTION.get(),
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.tree.selected_files.clear();
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .child(
                // Preserve the tree's content height for the outer scroll handle. A zero
                // flex basis hides overflowing rows from its measured scroll extent.
                div().relative().flex().flex_col().flex_grow(1.).flex_shrink_0().child(div()
                    .id("file-tree-scroll")
                    .flex().flex_col().w_full().flex_grow(1.).flex_shrink_0()
                    .key_context("FolioFileTree")
                    .track_focus(&self.tree.file_tree_focus)
                    .on_action(cx.listener(Self::copy_file_path))
                    .on_action(cx.listener(Self::paste_to_file_tree))
                    .on_action(cx.listener(Self::edit_tags_from_tree))
                    .on_action(cx.listener(Self::delete_file_tree_item))
                    .on_action(cx.listener(Self::undo_file_delete))
                    .on_mouse_down(MouseButton::Left, cx.listener(|app, event: &MouseDownEvent, _, cx| {
                        if app.projects.directory().is_none() || app.tree.tree_row_bounds.lock().expect("tree bounds").values().any(|(_, bounds)| bounds.contains(&event.position)) {
                            return;
                        }
                        app.tree.tree_drag = Some(TreeSelectionDrag {
                            start: event.position,
                            current: event.position,
                            original: if event.modifiers.control { app.tree.selected_files.clone() } else { BTreeSet::new() },
                            moved: false,
                        });
                        cx.notify();
                    }))
                    .on_mouse_move(cx.listener(|app, event: &MouseMoveEvent, _, cx| {
                        if cx.has_active_drag() {
                            if app.tree.tree_drag.take().is_some() { cx.notify(); }
                            return;
                        }
                        let Some(drag) = app.tree.tree_drag.as_mut() else { return; };
                        if !event.dragging() { return; }
                        drag.current = event.position;
                        if (drag.current - drag.start).magnitude() > 3.0 { drag.moved = true; }
                        if drag.moved {
                            let selection = drag.bounds();
                            app.tree.selected_files = drag.original.clone();
                            // Bounds are measured in window coordinates, including the scroll offset.
                            for (path, (is_file, bounds)) in app.tree.tree_row_bounds.lock().expect("tree bounds").iter() {
                                if *is_file && selection.intersects(bounds) { app.tree.selected_files.insert(path.clone()); }
                            }
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|app, _, _, cx| {
                        if let Some(drag) = app.tree.tree_drag.take() {
                            if !drag.moved { app.tree.selected_files = drag.original; }
                            cx.notify();
                        }
                    }))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|app, _, _, cx| {
                        if app.tree.tree_drag.take().is_some() { cx.notify(); }
                    }))
                    .on_click(cx.listener(|app, _, window, cx| {
                        let changed = app.tree.focused_folder.take().is_some()
                            || app.tree.focused_file.take().is_some();
                        if changed {
                            cx.notify();
                        }
                        app.tree.file_tree_focus.focus(window, cx);
                    }))
                    .when(no_tag_results, |tree| tree.child(div().px_3().py_2().text_xs().text_color(rgb(MUTED)).child(crate::i18n::workspace::NO_MATCHING_FILES.get())))
                    .children(rows)
                    .when_some(self.projects.directory(), |tree, project| {
                        let project_id = project.id.clone();
                        let folder_project = project.id.clone();
                        let bounds = self.tree.tree_row_bounds.clone();
                        let paste_root_app = cx.entity();
                        // Empty space belongs to the root; folder rows retain their own drop targets.
                        tree.child(div().id("project-root-drop").relative()
                            .flex_grow(1.).h(px(48.)).flex_shrink_0()
                            .drag_over::<crate::terminal::DraggedPaths>(move |style, dragged, _, _| {
                                if dragged.1.as_deref() == Some(project_id.as_str()) { style.bg(rgb(0xdde9f6)) } else { style }
                            })
                            .drag_over::<crate::terminal::DraggedFolder>(move |style, dragged, _, _| {
                                if dragged.1 == folder_project { style.bg(rgb(0xdde9f6)) } else { style }
                            })
                            .child(canvas(move |area, _, _| {
                                bounds.lock().expect("tree bounds").insert(String::new(), (false, area));
                            }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
                            .on_click(cx.listener(|app, _, window, cx| {
                                cx.stop_propagation();
                                app.tree.focused_folder = None;
                                app.tree.focused_file = None;
                                app.tree.file_tree_focus.focus(window, cx);
                                cx.notify();
                            }))
                            .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedPaths, window, cx| {
                                cx.stop_propagation();
                                if let Err(error) = app.move_tree_files(dragged, "", window, cx) { app.report(error, cx); }
                            }))
                            .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedFolder, window, cx| {
                                cx.stop_propagation();
                                if let Err(error) = app.move_tree_folder(dragged, "", window, cx) { app.report(error, cx); }
                            }))
                            .context_menu(move |menu, _, _| {
                                let app = paste_root_app.clone();
                                menu.min_w(px(110.)).item(
                                    compact_menu_item(crate::i18n::workspace::MENU_PASTE_IMAGE.get())
                                        .on_click(move |_, window, cx| {
                                            app.update(cx, |app, cx| {
                                                app.paste_images_into_folder("", window, cx)
                                            })
                                        }),
                                )
                            }))
                    })
                    .when(self.projects.directory().is_none(), |tree| tree.child(div().h(px(48.)).flex_shrink_0())))
                    .when_some(self.tree.tree_drag.as_ref().filter(|drag| drag.moved).map(TreeSelectionDrag::bounds), |area, selection| {
                        area.child(canvas(|_, _, _| (), move |_, _, window, _| {
                            window.paint_quad(fill(selection, rgb(0x165dff).opacity(0.18)));
                        }).absolute().top_0().left_0().size_full())
                    }),
            ))
                .child(div().absolute().top(px(20.)).bottom(px(20.)).left_0().right_0().child(
                    Scrollbar::vertical(&self.tree.sidebar_scroll)
                        .mode(ScrollbarMode::Always)
                        .styles(|styles| styles
                            .thumb(|style| style.bg(rgb(0xd9d9d7)).width(px(6.)).inset(px(1.)).radius(px(4.)).min_length(px(24.)))
                            .thumb_hover(|style| style.bg(rgb(0xbcbcb9)).width(px(6.)).inset(px(1.)).radius(px(4.)))
                            .thumb_active(|style| style.bg(rgb(0xa8a8a5)).width(px(6.)).inset(px(1.)).radius(px(4.)))),
                )))
            .child(div().mx_4().h(px(1.)).flex_shrink_0().bg(rgb(0xeae9e6)))
            .child({
                let bounds = self.tree.trash_bounds.clone();
                div().id("trash-entry").relative().flex().flex_shrink_0().items_center().gap_2()
                    .mx_2().my_2().px_2().h(px(32.)).rounded(px(4.)).cursor_pointer()
                    .when(self.tree.show_trash, |row| row.bg(rgb(0xe8e7e4)))
                    .when(dragging_tree_item, |row| row.bg(rgb(0xf0f4fa)))
                    .drag_over::<crate::terminal::DraggedPaths>(|style, _, _, _| style.bg(rgb(0xdde9f6)))
                    .drag_over::<crate::terminal::DraggedFolder>(|style, _, _, _| style.bg(rgb(0xdde9f6)))
                    .child(crate::ui::icon(IconName::Trash))
                    .child(crate::i18n::workspace::TRASH_LABEL.get())
                    .child(canvas(move |area, _, _| { *bounds.lock().expect("trash bounds") = Some(area); }, |_, _, _, _| {}).absolute().size_full())
                    .on_click(cx.listener(|app, _, _, cx| { app.tree.show_trash = true; cx.notify(); }))
                    .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedPaths, window, cx| {
                        if dragged.1.is_none() { app.report(crate::i18n::workspace::ERR_LOOSE_FILE_NO_DELETE.get(), cx); return; }
                        let Some(project) = app.projects.directory() else { return; };
                        if Some(project.id.as_str()) != dragged.1.as_deref() { app.report(crate::i18n::workspace::ERR_SOURCE_PROJECT_CHANGED.get(), cx); return; }
                        let root = PathBuf::from(&project.folder);
                        let paths: Vec<String> = dragged.0.iter().filter_map(|path| path.strip_prefix(&root).ok().map(|path| path.to_string_lossy().replace('\\', "/"))).collect();
                        if paths.len() == dragged.0.len() { app.confirm_trash(paths, window, cx); }
                    }))
                    .on_drop(cx.listener(|app, dragged: &crate::terminal::DraggedFolder, window, cx| {
                        if app.projects.directory().is_none_or(|project| project.id != dragged.1) {
                            app.report(crate::i18n::workspace::ERR_SOURCE_PROJECT_CHANGED.get(), cx);
                            return;
                        }
                        // A folder is a projection of supported files; its other contents remain in place.
                        app.delete_folder(&dragged.0, window, cx);
                    }))
            })
            .child(div().id("preferences").flex().flex_shrink_0().items_center().gap_2()
                .mx_2().mb_2().px_2().h(px(32.)).rounded(px(4.)).cursor_pointer()
                .text_color(rgb(MUTED)).hover(|style| style.bg(rgb(0xefefed)))
                .child(crate::ui::icon(IconName::Settings)).child(crate::i18n::workspace::SETTINGS_LABEL.get())
                .on_click(cx.listener(|app, _, window, cx| {
                    app.preferences.update(cx, |panel, cx| panel.sync(&app.settings, window, cx));
                    app.show_preferences = true;
                    cx.notify();
                })))
            .into_any_element()
    }
}
