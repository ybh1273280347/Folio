//! Project-grouped Trash view with scope switching, search filtering,
//! countdown-only retention display, and batch restore/purge actions.

use super::*;
use crate::i18n::workspace as text;
use crate::theme::Palette;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_PROJECT_ID: &str = "__folio_default";
const TRASH_ROW_HEIGHT: f32 = 48.;

struct GroupFold {
    from: f32,
    opening: bool,
    started: Instant,
}

impl GroupFold {
    fn sample(&self, now: Instant) -> (f32, bool) {
        let target = if self.opening { 1. } else { 0. };
        let distance = (target - self.from).abs();
        let duration = if self.opening { 0.195 } else { 0.155 };
        let t = (now.duration_since(self.started).as_secs_f32() / (duration * distance.sqrt()).max(0.06)).min(1.);
        (self.from + (target - self.from) * (1. - (1. - t).powi(3)), t < 1.)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TrashScope {
    #[default]
    CurrentProject,
    AllProjects,
}

pub(super) struct TrashView {
    pub(super) entries: Vec<crate::trash::TrashEntry>,
    open: bool,
    scope: TrashScope,
    search_input: Entity<InputState>,
    selected: BTreeSet<String>,
    collapsed_projects: BTreeSet<String>,
    group_folds: BTreeMap<String, GroupFold>,
    scroll: ScrollHandle,
    next_cleanup: Instant,
    last_rendered_minute: u64,
}

impl TrashView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(text::TRASH_SEARCH_PLACEHOLDER.get())
        });
        cx.subscribe(&search_input, |app, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && app.trash.open {
                cx.notify();
            }
        })
        .detach();
        Self {
            entries: Vec::new(),
            open: false,
            scope: TrashScope::CurrentProject,
            search_input,
            selected: BTreeSet::new(),
            collapsed_projects: BTreeSet::new(),
            group_folds: BTreeMap::new(),
            scroll: ScrollHandle::new(),
            next_cleanup: Instant::now(),
            last_rendered_minute: current_epoch_secs() / 60,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn dismiss(&mut self) {
        self.open = false;
        self.selected.clear();
        self.group_folds.clear();
    }

    pub(super) fn prune_selection(&mut self) {
        self.selected
            .retain(|id| self.entries.iter().any(|entry| &entry.id == id));
    }

    fn prune_selection_to_scope(&mut self, active_project_id: Option<&str>) {
        self.selected.retain(|id| {
            self.entries.iter().any(|entry| {
                &entry.id == id
                    && (self.scope == TrashScope::AllProjects
                        || active_project_id == Some(entry.project_id.as_str()))
            })
        });
    }
}

#[derive(Clone, Debug)]
struct ProjectMeta {
    id: String,
    name: String,
    is_current: bool,
    is_registered: bool,
}

#[derive(Clone)]
struct TrashGroup {
    key: String,
    name: String,
    is_current: bool,
    is_registered: bool,
    root_exists: bool,
    latest_deleted_at: u64,
    entries: Vec<crate::trash::TrashEntry>,
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn fg_alpha(hex: u32, alpha: f32) -> Rgba {
    let mut color = gpui_kit::rgb(hex);
    color.a = alpha;
    color
}

fn format_remaining_countdown(deleted_at: u64, now: u64) -> (String, bool) {
    let expires = deleted_at.saturating_add(crate::trash::RETENTION_SECS);
    let remaining = expires.saturating_sub(now);
    let urgent = remaining <= 24 * 3600;
    let days = remaining / (24 * 3600);
    let hours = (remaining % (24 * 3600)) / 3600;
    let label = if days > 0 && hours > 0 {
        format!("{days}d {hours}h")
    } else if days > 0 {
        format!("{days}d")
    } else if hours > 0 {
        format!("{hours}h")
    } else {
        "<1h".to_owned()
    };
    (label, urgent)
}

fn format_local_timestamp(timestamp: u64) -> String {
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map(|date| date.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn split_relative_path(relative: &str) -> (String, Option<String>) {
    let normalized = relative.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    match parts.split_last() {
        Some((file, dirs)) if !dirs.is_empty() => {
            ((*file).to_owned(), Some(dirs.join(" / ")))
        }
        Some((file, _)) => ((*file).to_owned(), None),
        None => (relative.to_owned(), None),
    }
}

fn fallback_project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| root.display().to_string())
}

fn group_trash_entries(
    entries: &[crate::trash::TrashEntry],
    active_project_id: Option<&str>,
    metas: &[ProjectMeta],
    scope: TrashScope,
    query: &str,
) -> Vec<TrashGroup> {
    let meta_map: HashMap<&str, &ProjectMeta> = metas
        .iter()
        .map(|meta| (meta.id.as_str(), meta))
        .collect();
    let needle = query.trim().to_lowercase();

    let mut groups_by_key: HashMap<String, TrashGroup> = HashMap::new();
    for entry in entries {
        if scope == TrashScope::CurrentProject
            && active_project_id != Some(entry.project_id.as_str())
        {
            continue;
        }

        let meta = meta_map.get(entry.project_id.as_str()).copied();
        let (project_name, is_current, is_registered) = if let Some(meta) = meta {
            (meta.name.clone(), meta.is_current, meta.is_registered)
        } else if entry.project_id == DEFAULT_PROJECT_ID {
            (
                text::DEFAULT_SPACE.get().to_owned(),
                active_project_id == Some(DEFAULT_PROJECT_ID),
                true,
            )
        } else {
            (
                fallback_project_name(&entry.project_root),
                active_project_id == Some(entry.project_id.as_str()),
                false,
            )
        };

        if !needle.is_empty() {
            let normalized_rel = entry.relative.replace('\\', "/").to_lowercase();
            let file_name = split_relative_path(&entry.relative).0.to_lowercase();
            let proj_lower = project_name.to_lowercase();
            if !file_name.contains(&needle)
                && !normalized_rel.contains(&needle)
                && !proj_lower.contains(&needle)
            {
                continue;
            }
        }

        let key = if is_registered {
            format!("id:{}", entry.project_id)
        } else {
            format!("orphan:{}:{}", entry.project_id, entry.project_root.display())
        };

        let group = groups_by_key.entry(key.clone()).or_insert_with(|| TrashGroup {
            key,
            name: project_name,
            is_current,
            is_registered,
            root_exists: entry.project_root.is_dir(),
            latest_deleted_at: entry.deleted_at,
            entries: Vec::new(),
        });
        if entry.deleted_at > group.latest_deleted_at {
            group.latest_deleted_at = entry.deleted_at;
        }
        group.entries.push(entry.clone());
    }

    let mut groups: Vec<TrashGroup> = groups_by_key.into_values().collect();
    for group in &mut groups {
        group.entries.sort_by(|left, right| {
            right
                .deleted_at
                .cmp(&left.deleted_at)
                .then_with(|| right.deleted_at_nanos.cmp(&left.deleted_at_nanos))
                .then_with(|| right.id.cmp(&left.id))
        });
    }
    groups.sort_by(|left, right| {
        right
            .is_current
            .cmp(&left.is_current)
            .then_with(|| right.is_registered.cmp(&left.is_registered))
            .then_with(|| right.latest_deleted_at.cmp(&left.latest_deleted_at))
            .then_with(|| left.name.cmp(&right.name))
    });
    groups
}

#[derive(Clone, Copy)]
struct TrashType {
    heading: f32,
    title: f32,
    body: f32,
    meta: f32,
    micro: f32,
}

impl TrashType {
    fn from_ui_font(base: u8) -> Self {
        let s = f32::from(base);
        Self {
            heading: (s + 4.0).clamp(15.0, 20.0),
            title: s.max(12.0),
            body: (s - 1.0).max(11.0),
            meta: (s - 2.0).max(10.5),
            micro: (s - 3.0).max(9.5),
        }
    }
}

impl Workspace {
    pub(super) fn open_trash(&mut self, cx: &mut Context<Self>) {
        self.trash.open = true;
        self.trash.scope = if self.projects.directory().is_some() {
            TrashScope::CurrentProject
        } else {
            TrashScope::AllProjects
        };
        self.trash.last_rendered_minute = current_epoch_secs() / 60;
        self.trash.prune_selection();
        cx.notify();
    }

    pub(super) fn close_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.trash.is_open() {
            return;
        }
        self.trash.dismiss();
        self.focus_active_or_workspace(window, cx);
        cx.notify();
    }

    pub(super) fn poll_trash(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if Instant::now() >= self.trash.next_cleanup {
            self.trash.next_cleanup = Instant::now() + Duration::from_secs(3600);
            match crate::trash::cleanup().and_then(|_| Ok(crate::trash::list()?)) {
                Ok(entries) => {
                    self.trash.entries = entries;
                    self.trash.prune_selection();
                }
                Err(error) => self.report(error, cx),
            }
            changed = true;
        }
        if self.trash.open {
            let minute = current_epoch_secs() / 60;
            if minute != self.trash.last_rendered_minute {
                self.trash.last_rendered_minute = minute;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn restore_trash(
        &mut self,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_in_flight_sync();
        let mut index = 0;
        while let Some(id) = ids.get(index).cloned() {
            let Some(entry) = self
                .trash
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
            else {
                index += 1;
                continue;
            };
            if !entry.project_root.is_dir() {
                self.report(text::REPORT_TRASH_PARENT_MISSING.get(), cx);
                return;
            }
            let mut target = entry.project_root.join(&entry.relative);
            if target.exists() {
                let retry = ids[index..].to_vec();
                let Some(choice) = self.ask(
                    Dialog::TrashConflict,
                    text::DIALOG_TRASH_CONFLICT_TITLE.get().into(),
                    target.display().to_string(),
                    DialogButtons::RestoreConflict,
                    cx,
                    move |app, window, cx| app.restore_trash(retry, window, cx),
                ) else {
                    return;
                };
                match choice {
                    rfd::MessageDialogResult::Yes => {
                        if self.projects.directory().is_some_and(|project| {
                            project.id == entry.project_id
                                && Path::new(&project.folder) == entry.project_root
                        }) {
                            self.delete_paths(vec![entry.relative.clone()], None, window, cx);
                            if target.exists() {
                                return;
                            }
                        } else {
                            let batch = uuid::Uuid::new_v4().to_string();
                            match crate::trash::put(
                                &entry.project_id,
                                &entry.project_root,
                                &entry.relative,
                                &batch,
                                false,
                            ) {
                                Ok(replaced) => self.trash.entries.push(replaced),
                                Err(error) => {
                                    self.report(error, cx);
                                    return;
                                }
                            }
                        }
                    }
                    rfd::MessageDialogResult::No => {
                        let original = target.clone();
                        let stem = original.file_stem().expect("file name").to_string_lossy();
                        let extension = original
                            .extension()
                            .map(|value| format!(".{}", value.to_string_lossy()))
                            .unwrap_or_default();
                        let mut number = 1;
                        while target.exists() {
                            target = original
                                .with_file_name(format!("{stem} ({number}){extension}"));
                            number += 1;
                        }
                    }
                    _ => {
                        index += 1;
                        continue;
                    }
                }
            }
            let result = std::fs::create_dir_all(target.parent().expect("file parent"))
                .map_err(Into::into)
                .and_then(|_| crate::trash::restore(&entry, &target));
            if let Err(error) = result {
                self.report(error, cx);
                return;
            }
            self.trash.entries.retain(|item| item.id != id);
            self.trash.selected.remove(&id);
            if entry.was_main && target == entry.project_root.join(&entry.relative) {
                if let Err(error) = self
                    .projects
                    .restore_main_file(&entry.project_id, &entry.relative)
                {
                    self.report(error, cx);
                }
            }
            index += 1;
        }
        if let Err(error) = self.refresh_files(window, cx) {
            self.report(error, cx);
        }
        if self.kb_enabled() {
            self.refresh_kb_snapshot(cx);
            if !self.right_panel.kb.last_query.is_empty() {
                self.submit_kb_search(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn purge_trash(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let all = ids.len() > 1 || ids.len() == self.trash.entries.len();
        let retry = ids.clone();
        if self.ask(
            if all {
                Dialog::TrashClear
            } else {
                Dialog::TrashPurge
            },
            text::dialog_purge_title(ids.len()),
            text::DIALOG_PURGE_BODY.get().into(),
            DialogButtons::Confirm(text::DIALOG_PURGE_CONFIRM.get().into()),
            cx,
            move |app, _, cx| app.purge_trash(retry, cx),
        ) != Some(rfd::MessageDialogResult::Ok)
        {
            return;
        }
        self.cancel_in_flight_sync();
        for id in ids {
            if let Some(entry) = self
                .trash
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
            {
                if let Err(error) = crate::trash::purge(&entry) {
                    self.report(error, cx);
                    return;
                }
                self.trash.entries.retain(|entry| entry.id != id);
                self.trash.selected.remove(&id);
            }
        }
        cx.notify();
    }

    fn project_metas(&self) -> Vec<ProjectMeta> {
        let active_id = self.projects.directory().map(|project| project.id.as_str());
        let mut metas = Vec::with_capacity(self.projects.registered().len() + 1);
        metas.push(ProjectMeta {
            id: DEFAULT_PROJECT_ID.to_owned(),
            name: text::DEFAULT_SPACE.get().to_owned(),
            is_current: active_id == Some(DEFAULT_PROJECT_ID),
            is_registered: true,
        });
        for project in self.projects.registered() {
            metas.push(ProjectMeta {
                id: project.id.clone(),
                name: project.name.clone(),
                is_current: active_id == Some(project.id.as_str()),
                is_registered: true,
            });
        }
        metas
    }

    pub(super) fn render_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.settings.theme.palette;
        let dark = palette.is_dark();
        let t = TrashType::from_ui_font(self.settings.interface_font_size);

        let active_project = self.projects.directory();
        let active_id = active_project.map(|project| project.id.as_str());
        let effective_scope = if active_id.is_some() {
            self.trash.scope
        } else {
            TrashScope::AllProjects
        };

        let current_project_count = active_id.map_or(0, |id| {
            self.trash
                .entries
                .iter()
                .filter(|entry| entry.project_id == id)
                .count()
        });
        let total_count = self.trash.entries.len();

        let scope_ids: Vec<String> = self
            .trash
            .entries
            .iter()
            .filter(|entry| {
                effective_scope == TrashScope::AllProjects
                    || active_id == Some(entry.project_id.as_str())
            })
            .map(|entry| entry.id.clone())
            .collect();

        let query = self.trash.search_input.read(cx).value().to_string();
        let query_active = !query.trim().is_empty();
        let anim_now = Instant::now();
        let mut folds = BTreeMap::new();
        self.trash.group_folds.retain(|key, fold| {
            let (progress, moving) = fold.sample(anim_now);
            let moving = moving
                && effective_scope == TrashScope::AllProjects
                && !query_active
                && !cx.reduce_motion();
            if moving {
                folds.insert(key.clone(), (progress, fold.opening));
            }
            moving
        });
        if !folds.is_empty() {
            window.request_animation_frame();
        }
        let metas = self.project_metas();
        let groups = group_trash_entries(
            &self.trash.entries,
            active_id,
            &metas,
            effective_scope,
            &query,
        );

        let visible_ids: BTreeSet<String> = groups
            .iter()
            .flat_map(|group| group.entries.iter().map(|entry| entry.id.clone()))
            .collect();
        let selected_visible_ids: Vec<String> = self
            .trash
            .selected
            .iter()
            .filter(|id| visible_ids.contains(*id))
            .cloned()
            .collect();

        let now = current_epoch_secs();
        let search_focused = self
            .trash
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);

        let header = self.render_trash_header(
            palette,
            dark,
            t,
            active_id.is_some(),
            effective_scope,
            current_project_count,
            total_count,
            scope_ids,
            search_focused,
            query_active,
            cx,
        );

        let body_content: AnyElement = if groups.is_empty() {
            if query_active {
                self.render_trash_empty_state(
                    palette,
                    t,
                    IconName::Search,
                    text::TRASH_NO_MATCHES.get().to_owned(),
                    None,
                    Some(
                        div()
                            .id("trash-clear-filter")
                            .h(px(28.))
                            .px_3()
                            .rounded(px(7.))
                            .bg(palette.paper_surface())
                            .border_1()
                            .border_color(palette.paper_border())
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(t.meta))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.text))
                            .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.trash
                                    .search_input
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                cx.notify();
                            }))
                            .child(text::TRASH_CLEAR_FILTER.get())
                            .into_any_element(),
                    ),
                )
            } else if effective_scope == TrashScope::CurrentProject && total_count > 0 {
                self.render_trash_empty_state(
                    palette,
                    t,
                    IconName::Trash,
                    text::TRASH_EMPTY_PROJECT.get().to_owned(),
                    Some(text::trash_other_projects_hint(total_count)),
                    Some(
                        div()
                            .id("trash-view-all-projects")
                            .h(px(28.))
                            .px_3()
                            .rounded(px(7.))
                            .bg(gpui_kit::rgb(palette.accent))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(t.meta))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.on_accent))
                            .hover(|s| s.opacity(0.90))
                            .active(|s| s.opacity(0.82))
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.trash.scope = TrashScope::AllProjects;
                                app.trash.group_folds.clear();
                                cx.notify();
                            }))
                            .child(text::TRASH_VIEW_ALL_PROJECTS.get())
                            .into_any_element(),
                    ),
                )
            } else {
                self.render_trash_empty_state(
                    palette,
                    t,
                    IconName::Trash,
                    text::TRASH_EMPTY.get().to_owned(),
                    Some(text::TRASH_EMPTY_SUBTITLE.get().to_owned()),
                    None,
                )
            }
        } else {
            let cards = groups.into_iter().map(|group| {
                let fold = folds.get(&group.key).copied();
                self.render_trash_group_card(
                    palette,
                    dark,
                    t,
                    effective_scope,
                    query_active,
                    now,
                    fold,
                    group,
                    cx,
                )
            });
            div()
                .max_w(px(860.))
                .w_full()
                .mx_auto()
                .px_6()
                .pt_1()
                .pb(px(76.))
                .flex()
                .flex_col()
                .gap_4()
                .children(cards)
                .into_any_element()
        };

        let scroll_area = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("trash-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.trash.scroll)
                    .child(body_content),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .bottom(px(8.))
                    .left_0()
                    .right_0()
                    .child(
                        Scrollbar::vertical(&self.trash.scroll)
                            .mode(ScrollbarMode::Always)
                            .styles(move |styles| {
                                styles
                                    .thumb(move |style| {
                                        style
                                            .bg(fg_alpha(palette.muted, 0.30))
                                            .width(px(6.))
                                            .inset(px(1.))
                                            .radius(px(4.))
                                            .min_length(px(24.))
                                    })
                                    .thumb_hover(move |style| {
                                        style
                                            .bg(fg_alpha(palette.muted, 0.50))
                                            .width(px(6.))
                                            .inset(px(1.))
                                            .radius(px(4.))
                                    })
                                    .thumb_active(move |style| {
                                        style
                                            .bg(fg_alpha(palette.muted, 0.72))
                                            .width(px(6.))
                                            .inset(px(1.))
                                            .radius(px(4.))
                                    })
                            }),
                    ),
            );

        div()
            .id("trash-view")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui_kit::rgb(palette.background))
            .child(header)
            .child(scroll_area)
            .when(!selected_visible_ids.is_empty(), |root| {
                root.child(self.render_trash_batch_bar(
                    palette,
                    dark,
                    t,
                    selected_visible_ids,
                    cx,
                ))
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_trash_header(
        &self,
        palette: Palette,
        dark: bool,
        t: TrashType,
        has_active_project: bool,
        effective_scope: TrashScope,
        current_project_count: usize,
        total_count: usize,
        scope_ids: Vec<String>,
        search_focused: bool,
        query_active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let seg_bg = fg_alpha(palette.text, 0.045);
        let hover_bg = gpui_kit::rgb(palette.hover);
        let ring_color = if search_focused {
            gpui_kit::rgb(palette.accent)
        } else {
            gpui_kit::rgb(palette.border)
        };
        let outer_halo = fg_alpha(palette.accent, 0.14);

        let clear_label = if effective_scope == TrashScope::CurrentProject {
            text::TRASH_CLEAR_PROJECT.get()
        } else {
            text::TRASH_CLEAR_ALL.get()
        };
        let clear_disabled = scope_ids.is_empty();
        let danger_hover_bg = if dark { 0x3b1d22 } else { 0xfff1f0 };
        let danger_fg = gpui_kit::rgb(palette.danger);

        let scope_chip = |id: &'static str,
                          scope_val: TrashScope,
                          label: &'static str,
                          count: usize| {
            let active = effective_scope == scope_val;
            div()
                .id(id)
                .h(px(26.))
                .px_2p5()
                .rounded(px(6.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(t.meta))
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(palette.paper_surface())
                        .border_1()
                        .border_color(palette.paper_border())
                        .font_medium()
                        .text_color(gpui_kit::rgb(palette.text))
                })
                .when(!active, |el| {
                    el.text_color(gpui_kit::rgb(palette.muted))
                        .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                })
                .on_click(cx.listener(move |app, _, _, cx| {
                    if app.trash.scope == scope_val {
                        return;
                    }
                    app.trash.scope = scope_val;
                    app.trash.group_folds.clear();
                    let active_id = app.projects.directory().map(|p| p.id.clone());
                    app.trash.prune_selection_to_scope(active_id.as_deref());
                    cx.notify();
                }))
                .child(label)
                .child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(if active {
                            fg_alpha(palette.accent, 0.12)
                        } else {
                            fg_alpha(palette.text, 0.06)
                        })
                        .text_size(px(t.micro))
                        .text_color(if active {
                            gpui_kit::rgb(palette.accent)
                        } else {
                            gpui_kit::rgb(palette.muted)
                        })
                        .child(count.to_string()),
                )
        };

        let search_box = div()
            .w(px(240.))
            .rounded(px(9.))
            .when(search_focused, |el| el.p(px(2.)).bg(outer_halo))
            .child(
                div()
                    .w_full()
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_1()
                    .border_color(ring_color)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        crate::ui::icon(IconName::Search)
                            .w(px(13.))
                            .h(px(13.))
                            .flex_shrink_0()
                            .text_color(gpui_kit::rgb(palette.muted)),
                    )
                    .child(
                        Input::new(&self.trash.search_input)
                            .appearance(false)
                            .bordered(false)
                            .flex_1()
                            .min_w_0()
                            .px_0()
                            .py_0()
                            .text_size(px(t.body))
                            .text_color(gpui_kit::rgb(palette.text)),
                    )
                    .when(query_active, |bar| {
                        bar.child(
                            div()
                                .id("trash-clear-search")
                                .size(px(18.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .hover(move |s| {
                                    s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text))
                                })
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.trash.search_input.update(cx, |input, cx| {
                                        input.set_value("", window, cx);
                                    });
                                    cx.notify();
                                }))
                                .child(
                                    crate::ui::icon(IconName::Close)
                                        .w(px(11.))
                                        .h(px(11.)),
                                ),
                        )
                    }),
            );

        div()
            .max_w(px(860.))
            .w_full()
            .mx_auto()
            .px_6()
            .pt_5()
            .pb_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(t.heading))
                                    .font_semibold()
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .child(text::TRASH_LABEL.get()),
                            )
                            .child(
                                div()
                                    .text_size(px(t.meta))
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child(text::TRASH_SUBTITLE.get()),
                            ),
                    )
                    .child(
                        div()
                            .id("trash-clear-scope")
                            .h(px(28.))
                            .px_2p5()
                            .rounded(px(7.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(t.meta))
                            .font_medium()
                            .border_1()
                            .border_color(if clear_disabled {
                                palette.paper_border()
                            } else {
                                fg_alpha(palette.danger, 0.25)
                            })
                            .text_color(if clear_disabled {
                                fg_alpha(palette.muted, 0.55)
                            } else {
                                danger_fg
                            })
                            .when(!clear_disabled, |el| {
                                el.cursor_pointer()
                                    .bg(fg_alpha(palette.danger, 0.06))
                                    .hover(move |s| s.bg(gpui_kit::rgb(danger_hover_bg)))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.purge_trash(scope_ids.clone(), cx);
                                    }))
                            })
                            .child(crate::ui::icon(IconName::Trash).w(px(13.)).h(px(13.)))
                            .child(clear_label),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .p(px(2.))
                            .rounded(px(8.))
                            .bg(seg_bg)
                            .gap(px(2.))
                            .when(has_active_project, |seg| {
                                seg.child(scope_chip(
                                    "trash-scope-current",
                                    TrashScope::CurrentProject,
                                    text::TRASH_SCOPE_CURRENT.get(),
                                    current_project_count,
                                ))
                            })
                            .child(scope_chip(
                                "trash-scope-all",
                                TrashScope::AllProjects,
                                text::TRASH_SCOPE_ALL.get(),
                                total_count,
                            )),
                    )
                    .child(search_box),
            )
            .into_any_element()
    }

    fn render_trash_empty_state(
        &self,
        palette: Palette,
        t: TrashType,
        icon: IconName,
        title: String,
        subtitle: Option<String>,
        action: Option<AnyElement>,
    ) -> AnyElement {
        div()
            .w_full()
            .min_h(px(340.))
            .px_6()
            .py_12()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size(px(44.))
                    .rounded_full()
                    .bg(palette.paper_surface())
                    .border_1()
                    .border_color(palette.paper_border())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::ui::icon(icon)
                            .w(px(18.))
                            .h(px(18.))
                            .text_color(gpui_kit::rgb(palette.muted)),
                    ),
            )
            .child(
                div()
                    .text_size(px(t.title))
                    .font_medium()
                    .text_color(gpui_kit::rgb(palette.text))
                    .text_center()
                    .child(title),
            )
            .when_some(subtitle, |col, sub| {
                col.child(
                    div()
                        .text_size(px(t.meta))
                        .text_color(gpui_kit::rgb(palette.muted))
                        .text_center()
                        .child(sub),
                )
            })
            .when_some(action, |col, btn| col.child(div().pt_1().child(btn)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_trash_group_card(
        &self,
        palette: Palette,
        dark: bool,
        t: TrashType,
        effective_scope: TrashScope,
        query_active: bool,
        now: u64,
        fold: Option<(f32, bool)>,
        group: TrashGroup,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsible = effective_scope == TrashScope::AllProjects;
        let collapsed = collapsible
            && !query_active
            && self.trash.collapsed_projects.contains(&group.key);
        let show_drawer = !collapsed || fold.is_some();
        let visual_progress = fold
            .map(|(progress, _)| progress)
            .unwrap_or(if collapsed { 0. } else { 1. });

        let group_ids: Vec<String> = group.entries.iter().map(|e| e.id.clone()).collect();
        let selected_in_group = group_ids
            .iter()
            .filter(|id| self.trash.selected.contains(*id))
            .count();
        let all_selected = !group_ids.is_empty() && selected_in_group == group_ids.len();
        let some_selected = selected_in_group > 0 && !all_selected;

        let group_key_toggle = group.key.clone();
        let group_ids_select = group_ids.clone();
        let group_ids_restore = group_ids.clone();
        let group_ids_purge = group_ids.clone();
        let root_exists = group.root_exists;
        let hover_bg = gpui_kit::rgb(palette.hover);
        let danger_hover_bg = if dark { 0x3b1d22 } else { 0xfff1f0 };
        let accent = gpui_kit::rgb(palette.accent);
        let on_accent = gpui_kit::rgb(palette.on_accent);
        let warn_bg = if dark { 0x3c3323 } else { 0xfef3c7 };
        let warn_fg = if dark { 0xe7bd7c } else { 0xb45309 };

        let group_checkbox = div()
            .id(SharedString::from(format!("trash-group-check-{}", group.key)))
            .size(px(15.))
            .rounded(px(4.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(all_selected || some_selected, |el| {
                el.bg(accent)
                    .border_1()
                    .border_color(accent)
                    .text_color(on_accent)
                    .child(
                        crate::ui::icon(if all_selected {
                            IconName::Check
                        } else {
                            IconName::Minus
                        })
                        .w(px(10.))
                        .h(px(10.)),
                    )
            })
            .when(!all_selected && !some_selected, |el| {
                el.bg(palette.paper_surface())
                    .border_1()
                    .border_color(fg_alpha(palette.muted, 0.45))
                    .hover(move |s| s.border_color(accent))
            })
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                if all_selected {
                    for id in &group_ids_select {
                        app.trash.selected.remove(id);
                    }
                } else {
                    for id in &group_ids_select {
                        app.trash.selected.insert(id.clone());
                    }
                }
                cx.notify();
            }));

        let title_cluster = div()
            .id(SharedString::from(format!("trash-group-toggle-{}", group.key)))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .when(collapsible && !query_active, |el| {
                el.cursor_pointer().on_click(cx.listener(move |app, _, _, cx| {
                    let opening = app.trash.collapsed_projects.remove(&group_key_toggle);
                    if !opening {
                        app.trash
                            .collapsed_projects
                            .insert(group_key_toggle.clone());
                    }
                    if !cx.reduce_motion() {
                        let now = Instant::now();
                        let from = app
                            .trash
                            .group_folds
                            .get(&group_key_toggle)
                            .map_or(if opening { 0. } else { 1. }, |fold| fold.sample(now).0);
                        app.trash.group_folds.insert(
                            group_key_toggle.clone(),
                            GroupFold {
                                from,
                                opening,
                                started: now,
                            },
                        );
                    }
                    cx.notify();
                }))
            })
            .when(collapsible, |el| {
                el.child(
                    crate::ui::icon(IconName::ChevronRight)
                        .w(px(13.))
                        .h(px(13.))
                        .flex_shrink_0()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .rotate(gpui::radians(std::f32::consts::FRAC_PI_2 * visual_progress)),
                )
            })
            .child(
                crate::ui::icon(if collapsed {
                    IconName::Folder
                } else {
                    IconName::FolderOpen
                })
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0()
                .text_color(if group.is_current {
                    accent
                } else {
                    gpui_kit::rgb(palette.muted)
                }),
            )
            .child(
                div()
                    .text_size(px(t.body))
                    .font_medium()
                    .text_color(gpui_kit::rgb(palette.text))
                    .text_ellipsis()
                    .child(group.name.clone()),
            )
            .child(
                div()
                    .text_size(px(t.meta))
                    .text_color(gpui_kit::rgb(palette.muted))
                    .flex_shrink_0()
                    .child(text::trash_item_count(group.entries.len())),
            )
            .when(!group.is_registered, |el| {
                el.child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(fg_alpha(palette.text, 0.06))
                        .text_size(px(t.micro))
                        .text_color(gpui_kit::rgb(palette.muted))
                        .flex_shrink_0()
                        .child(text::TRASH_PROJECT_REMOVED.get()),
                )
            })
            .when(!root_exists, |el| {
                el.child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(gpui_kit::rgb(warn_bg))
                        .text_size(px(t.micro))
                        .text_color(gpui_kit::rgb(warn_fg))
                        .flex_shrink_0()
                        .child(text::TRASH_DIR_MISSING.get()),
                )
            });

        let restore_all_btn = div()
            .id(SharedString::from(format!("trash-group-restore-{}", group.key)))
            .h(px(24.))
            .px_2()
            .rounded(px(6.))
            .flex()
            .items_center()
            .gap(px(5.))
            .text_size(px(t.meta))
            .text_color(if root_exists {
                gpui_kit::rgb(palette.muted)
            } else {
                fg_alpha(palette.muted, 0.45)
            })
            .when(root_exists, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.restore_trash(group_ids_restore.clone(), window, cx);
                    }))
            })
            .when(!root_exists, |el| {
                el.tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        text::REPORT_TRASH_PARENT_MISSING.get(),
                    )
                    .build(window, cx)
                })
            })
            .child(crate::ui::icon(IconName::RotateCcw).w(px(12.)).h(px(12.)))
            .child(text::TRASH_RESTORE_ALL.get());

        let header = div()
            .h(px(38.))
            .px(px(14.))
            .bg(palette.paper_header())
            .when(show_drawer, |h| {
                h.border_b_1().border_color(palette.paper_border())
            })
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(group_checkbox)
                    .child(title_cluster),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(restore_all_btn)
                    .when(collapsible, |actions| {
                        actions.child(
                            div()
                                .id(SharedString::from(format!(
                                    "trash-group-purge-{}",
                                    group.key
                                )))
                                .h(px(24.))
                                .px_2()
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(t.meta))
                                .cursor_pointer()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .hover(move |s| {
                                    s.bg(gpui_kit::rgb(danger_hover_bg))
                                        .text_color(gpui_kit::rgb(palette.danger))
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.purge_trash(group_ids_purge.clone(), cx);
                                }))
                                .child(text::TRASH_CLEAR_GROUP.get()),
                        )
                    }),
            );

        let rows = if show_drawer {
            group
                .entries
                .into_iter()
                .enumerate()
                .map(|(index, entry)| {
                    self.render_trash_row(
                        palette,
                        dark,
                        t,
                        index > 0,
                        root_exists,
                        now,
                        entry,
                        cx,
                    )
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let drawer = if let Some((progress, opening)) = fold {
            if rows.is_empty() {
                None
            } else {
                // Keep rows at full height behind an animated clip so siblings slide smoothly.
                let height = rows.len() as f32 * TRASH_ROW_HEIGHT * progress;
                Some(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .h(px(height))
                        .overflow_hidden()
                        .children(rows.into_iter().enumerate().map(|(i, row)| {
                            let reveal = (height / TRASH_ROW_HEIGHT - i as f32).clamp(0., 1.);
                            row.opacity(0.55 + 0.45 * reveal)
                        }))
                        .when(!opening, |clip| {
                            clip.child(div().absolute().inset_0().block_mouse_except_scroll())
                        })
                        .into_any_element(),
                )
            }
        } else if !collapsed && !rows.is_empty() {
            Some(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .children(rows)
                    .into_any_element(),
            )
        } else {
            None
        };

        div()
            .rounded(px(10.))
            .bg(palette.paper_surface())
            .border_1()
            .border_color(palette.paper_border())
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            .when_some(drawer, |card, drawer| card.child(drawer))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_trash_row(
        &self,
        palette: Palette,
        dark: bool,
        t: TrashType,
        show_divider: bool,
        root_exists: bool,
        now: u64,
        entry: crate::trash::TrashEntry,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let is_selected = self.trash.selected.contains(&entry.id);
        let (file_name, parent_dir) = split_relative_path(&entry.relative);
        let (countdown, urgent) = format_remaining_countdown(entry.deleted_at, now);
        let deleted_str = format_local_timestamp(entry.deleted_at);
        let expires_str =
            format_local_timestamp(entry.deleted_at.saturating_add(crate::trash::RETENTION_SECS));
        let time_tooltip = text::trash_time_tooltip(&deleted_str, &expires_str);

        let row_id = SharedString::from(format!("trash-row-{}", entry.id));
        let row_group = SharedString::from(format!("trash-row-grp-{}", entry.id));
        let toggle_id = entry.id.clone();
        let restore_id = entry.id.clone();
        let purge_id = entry.id.clone();

        let accent = gpui_kit::rgb(palette.accent);
        let on_accent = gpui_kit::rgb(palette.on_accent);
        let hover_bg = gpui_kit::rgb(palette.hover);
        let danger_hover_bg = if dark { 0x3b1d22 } else { 0xfff1f0 };
        let warn_bg = if dark { 0x3c3323 } else { 0xfef3c7 };
        let warn_fg = if dark { 0xe7bd7c } else { 0xb45309 };

        let row_checkbox = div()
            .size(px(15.))
            .rounded(px(4.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(is_selected, |el| {
                el.bg(accent)
                    .border_1()
                    .border_color(accent)
                    .text_color(on_accent)
                    .child(crate::ui::icon(IconName::Check).w(px(10.)).h(px(10.)))
            })
            .when(!is_selected, |el| {
                let grp = row_group.clone();
                el.bg(palette.paper_surface())
                    .border_1()
                    .border_color(fg_alpha(palette.muted, 0.45))
                    .opacity(0.5)
                    .group_hover(grp, |s| s.opacity(1.0).border_color(accent))
            });

        let is_asset = crate::files::is_asset(Path::new(&entry.relative));
        let file_icon = crate::ui::file_icon(&entry.relative, is_asset, entry.was_main);

        let countdown_pill = div()
            .id(SharedString::from(format!("trash-time-{}", entry.id)))
            .px(px(6.))
            .py(px(2.))
            .rounded(px(5.))
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(t.micro))
            .when(urgent, |el| {
                el.bg(gpui_kit::rgb(warn_bg))
                    .text_color(gpui_kit::rgb(warn_fg))
                    .font_medium()
            })
            .when(!urgent, |el| el.text_color(palette.paper_folio()))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(time_tooltip.clone()).build(window, cx)
            })
            .child(crate::ui::icon(IconName::Clock).w(px(11.)).h(px(11.)))
            .child(countdown);

        let restore_btn = div()
            .id(SharedString::from(format!("trash-restore-{}", entry.id)))
            .size(px(26.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(if root_exists {
                gpui_kit::rgb(palette.muted)
            } else {
                fg_alpha(palette.muted, 0.4)
            })
            .when(root_exists, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text)))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(text::TRASH_RESTORE.get())
                            .build(window, cx)
                    })
                    .on_click(cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.restore_trash(vec![restore_id.clone()], window, cx);
                    }))
            })
            .when(!root_exists, |el| {
                el.tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        text::REPORT_TRASH_PARENT_MISSING.get(),
                    )
                    .build(window, cx)
                })
            })
            .child(crate::ui::icon(IconName::RotateCcw).w(px(13.)).h(px(13.)));

        let purge_btn = div()
            .id(SharedString::from(format!("trash-purge-{}", entry.id)))
            .size(px(26.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(gpui_kit::rgb(palette.muted))
            .hover(move |s| {
                s.bg(gpui_kit::rgb(danger_hover_bg))
                    .text_color(gpui_kit::rgb(palette.danger))
            })
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(text::TRASH_PURGE.get())
                    .build(window, cx)
            })
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.purge_trash(vec![purge_id.clone()], cx);
            }))
            .child(crate::ui::icon(IconName::Trash).w(px(13.)).h(px(13.)));

        let actions = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .opacity(if is_selected { 0.85 } else { 0.0 })
            .group_hover(row_group.clone(), |s| s.opacity(1.0))
            .child(restore_btn)
            .child(purge_btn);

        div()
            .id(row_id)
            .group(row_group)
            .h(px(TRASH_ROW_HEIGHT))
            .flex_shrink_0()
            .px(px(14.))
            .when(show_divider, |row| {
                row.border_t_1().border_color(palette.paper_divider())
            })
            .when(is_selected, |row| row.bg(fg_alpha(palette.accent, 0.08)))
            .when(!is_selected, |row| {
                row.hover(move |s| s.bg(palette.paper_hover()))
            })
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .on_click(cx.listener(move |app, _, _, cx| {
                if !app.trash.selected.remove(&toggle_id) {
                    app.trash.selected.insert(toggle_id.clone());
                }
                cx.notify();
            }))
            .child(row_checkbox)
            .child(file_icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(t.body))
                                    .font_medium()
                                    .text_color(gpui_kit::rgb(palette.text))
                                    .text_ellipsis()
                                    .child(file_name),
                            )
                            .when(entry.was_main, |line| {
                                line.child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(fg_alpha(palette.accent, 0.12))
                                        .text_size(px(t.micro))
                                        .font_medium()
                                        .text_color(accent)
                                        .flex_shrink_0()
                                        .child(text::TRASH_MAIN_FILE_BADGE.get()),
                                )
                            }),
                    )
                    .when_some(parent_dir, |col, parent| {
                        col.child(
                            div()
                                .text_size(px(t.micro))
                                .text_color(palette.paper_folio())
                                .text_ellipsis()
                                .child(parent),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(countdown_pill)
                    .child(actions),
            )
    }

    fn render_trash_batch_bar(
        &self,
        palette: Palette,
        dark: bool,
        t: TrashType,
        selected_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = selected_ids.len();
        let restorable_ids: Vec<String> = selected_ids
            .iter()
            .filter(|id| {
                self.trash
                    .entries
                    .iter()
                    .find(|e| &e.id == *id)
                    .is_some_and(|e| e.project_root.is_dir())
            })
            .cloned()
            .collect();
        let can_restore = !restorable_ids.is_empty();
        let purge_ids = selected_ids;
        let hover_bg = gpui_kit::rgb(palette.hover);
        let danger_hover_bg = if dark { 0x3b1d22 } else { 0xfff1f0 };
        let shadow = vec![
            BoxShadow::new(px(0.), px(6.), hsla(0., 0., 0., if dark { 0.35 } else { 0.10 }))
                .blur_radius(px(16.)),
        ];

        div()
            .absolute()
            .bottom_4()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("trash-batch-bar")
                    .rounded(px(10.))
                    .bg(palette.paper_surface())
                    .border_1()
                    .border_color(palette.paper_border())
                    .shadow(shadow)
                    .px_3()
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px_1()
                            .text_size(px(t.meta))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.text))
                            .child(text::selected_count(count)),
                    )
                    .child(div().w(px(1.)).h(px(16.)).bg(palette.paper_border()))
                    .child(
                        div()
                            .id("trash-batch-restore")
                            .h(px(26.))
                            .px_2p5()
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(t.meta))
                            .font_medium()
                            .when(can_restore, |el| {
                                el.cursor_pointer()
                                    .bg(gpui_kit::rgb(palette.accent))
                                    .text_color(gpui_kit::rgb(palette.on_accent))
                                    .hover(|s| s.opacity(0.90))
                                    .active(|s| s.opacity(0.82))
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.restore_trash(restorable_ids.clone(), window, cx);
                                    }))
                            })
                            .when(!can_restore, |el| {
                                el.bg(fg_alpha(palette.text, 0.06))
                                    .text_color(fg_alpha(palette.muted, 0.55))
                            })
                            .child(
                                crate::ui::icon(IconName::RotateCcw)
                                    .w(px(12.))
                                    .h(px(12.)),
                            )
                            .child(text::TRASH_RESTORE_SELECTED.get()),
                    )
                    .child(
                        div()
                            .id("trash-batch-purge")
                            .h(px(26.))
                            .px_2p5()
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .cursor_pointer()
                            .text_size(px(t.meta))
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.danger))
                            .bg(fg_alpha(palette.danger, 0.08))
                            .hover(move |s| s.bg(gpui_kit::rgb(danger_hover_bg)))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.purge_trash(purge_ids.clone(), cx);
                            }))
                            .child(crate::ui::icon(IconName::Trash).w(px(12.)).h(px(12.)))
                            .child(text::TRASH_PURGE.get()),
                    )
                    .child(
                        div()
                            .id("trash-batch-clear-sel")
                            .size(px(24.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .hover(move |s| {
                                s.bg(hover_bg).text_color(gpui_kit::rgb(palette.text))
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.trash.selected.clear();
                                cx.notify();
                            }))
                            .child(crate::ui::icon(IconName::Close).w(px(12.)).h(px(12.))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GroupFold, ProjectMeta, TrashScope, format_remaining_countdown, group_trash_entries,
        split_relative_path,
    };
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    fn sample_entry(
        id: &str,
        project_id: &str,
        root: &str,
        relative: &str,
        deleted_at: u64,
    ) -> crate::trash::TrashEntry {
        crate::trash::TrashEntry {
            id: id.to_owned(),
            batch: "b1".to_owned(),
            project_id: project_id.to_owned(),
            project_root: PathBuf::from(root),
            relative: relative.to_owned(),
            deleted_at,
            deleted_at_nanos: 0,
            was_main: false,
            file_id: None,
        }
    }

    #[test]
    fn countdown_formats_and_marks_urgency() {
        let now = 1_700_000_000;
        let seven_days = crate::trash::RETENTION_SECS;

        // 6d 23h left (deleted 1h ago) -> not urgent
        assert_eq!(
            format_remaining_countdown(now - 3600, now),
            ("6d 23h".to_owned(), false)
        );
        // Exactly 3d left -> not urgent
        assert_eq!(
            format_remaining_countdown(now - (seven_days - 3 * 24 * 3600), now),
            ("3d".to_owned(), false)
        );
        // Exactly 24h left -> urgent boundary
        assert_eq!(
            format_remaining_countdown(now - (seven_days - 24 * 3600), now),
            ("1d".to_owned(), true)
        );
        // 14h left -> urgent
        assert_eq!(
            format_remaining_countdown(now - (seven_days - 14 * 3600), now),
            ("14h".to_owned(), true)
        );
        // 30m left -> urgent <1h
        assert_eq!(
            format_remaining_countdown(now - (seven_days - 1800), now),
            ("<1h".to_owned(), true)
        );
        // Past retention -> urgent <1h
        assert_eq!(
            format_remaining_countdown(now - seven_days - 500, now),
            ("<1h".to_owned(), true)
        );
    }

    #[test]
    fn split_relative_path_handles_nested_and_root_paths() {
        assert_eq!(
            split_relative_path("Notes/1/Untitled.md"),
            ("Untitled.md".to_owned(), Some("Notes / 1".to_owned()))
        );
        assert_eq!(
            split_relative_path(r"images\Untitled\paste.png"),
            ("paste.png".to_owned(), Some("images / Untitled".to_owned()))
        );
        assert_eq!(split_relative_path("main.tex"), ("main.tex".to_owned(), None));
    }

    #[test]
    fn grouping_respects_scope_ordering_and_search() {
        let entries = vec![
            sample_entry("1", "p2", "/tmp/p2", "old.md", 100),
            sample_entry("2", "p1", "/tmp/p1", "Notes/1/a.md", 200),
            sample_entry("3", "p1", "/tmp/p1", "Notes/1/b.md", 300),
            sample_entry("4", "orphan", "/tmp/orphan-proj", "lost.tex", 500),
        ];
        let metas = vec![
            ProjectMeta {
                id: "p1".to_owned(),
                name: "Data Structures".to_owned(),
                is_current: true,
                is_registered: true,
            },
            ProjectMeta {
                id: "p2".to_owned(),
                name: "Calculus".to_owned(),
                is_current: false,
                is_registered: true,
            },
        ];

        let current_only = group_trash_entries(
            &entries,
            Some("p1"),
            &metas,
            TrashScope::CurrentProject,
            "",
        );
        assert_eq!(current_only.len(), 1);
        assert_eq!(current_only[0].name, "Data Structures");
        assert_eq!(
            current_only[0]
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["3", "2"]
        );

        let all_groups = group_trash_entries(
            &entries,
            Some("p1"),
            &metas,
            TrashScope::AllProjects,
            "",
        );
        assert_eq!(
            all_groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            vec!["Data Structures", "Calculus", "orphan-proj"]
        );
        assert!(!all_groups[2].is_registered);

        let filtered = group_trash_entries(
            &entries,
            Some("p1"),
            &metas,
            TrashScope::AllProjects,
            "calculus",
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "Calculus");
    }

    #[test]
    fn group_fold_animates_and_reverses_from_mid_flight() {
        let start = Instant::now();
        let open_fold = GroupFold {
            from: 0.0,
            opening: true,
            started: start,
        };
        let (p0, moving0) = open_fold.sample(start);
        assert!((p0 - 0.0).abs() < 1e-5 && moving0);

        let mid_time = start + Duration::from_millis(80);
        let (p_mid, moving_mid) = open_fold.sample(mid_time);
        assert!(p_mid > 0.0 && p_mid < 1.0 && moving_mid);

        let (p_end, moving_end) = open_fold.sample(start + Duration::from_millis(220));
        assert!((p_end - 1.0).abs() < 1e-5 && !moving_end);

        let reverse_fold = GroupFold {
            from: p_mid,
            opening: false,
            started: mid_time,
        };
        let (rev_start, rev_moving) = reverse_fold.sample(mid_time);
        assert!((rev_start - p_mid).abs() < 1e-5 && rev_moving);
        let (rev_end, rev_done) = reverse_fold.sample(mid_time + Duration::from_millis(200));
        assert!((rev_end - 0.0).abs() < 1e-5 && !rev_done);
    }
}
