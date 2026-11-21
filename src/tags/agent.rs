//! Agent-facing queries and proposals for one directory workspace. The store
//! renders the proposed changes before the user sees them and recomputes the
//! same proposal inside the write transaction before applying it.

use super::*;
use crate::model::ProjectFile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::SystemTime;

pub type ToolResult<T> = std::result::Result<T, Failure>;

#[derive(Debug)]
pub struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn result(self) -> Value {
        json!({ "status": "error", "error": { "code": self.code, "message": self.message } })
    }

    fn storage(error: impl std::fmt::Display) -> Self {
        Self::new("tag_store_error", error.to_string())
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", content = "params", rename_all = "snake_case")]
pub enum Request {
    List(List),
    Query(Query),
    PreviewFiles(Draft<FileEdit>),
    PreviewDefinition(Draft<DefinitionEdit>),
    Commit(ProposalId),
    Discard(ProposalId),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalId {
    pub proposal_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft<T> {
    pub proposal_id: Uuid,
    pub edit: T,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    query: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    paths: Option<Vec<String>>,
    under_directory: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(default, rename = "match")]
    match_kind: Match,
    name_contains: Option<String>,
    extension: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum Match {
    #[default]
    All,
    Any,
}

impl Match {
    fn matches(&self, tags: &[String], labels: &[String]) -> bool {
        tags.is_empty()
            || match self {
                Self::All => tags.iter().all(|id| labels.contains(id)),
                Self::Any => tags.iter().any(|id| labels.contains(id)),
            }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEdit {
    paths: Option<Vec<String>>,
    under_directory: Option<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    create_missing: bool,
    #[serde(default)]
    tag_colors: Vec<RequestedColor>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestedColor {
    tag: String,
    color: Color,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DefinitionEdit {
    Rename { tag: String, new_name: String },
    SetColor { tag: String, color: Color },
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    Gray,
    Blue,
    Green,
    Orange,
    Red,
    Purple,
}

// Indexes match the palette already persisted by the desktop tag picker.
const COLORS: [&str; 6] = ["gray", "blue", "green", "orange", "red", "purple"];

pub enum Edit {
    Files(FileEdit),
    Definition(DefinitionEdit),
}

pub struct Proposal {
    edit: Edit,
    plan: Plan,
}

#[derive(PartialEq, Eq)]
enum Plan {
    Files {
        files: Vec<FileChange>,
        colors: Vec<PlannedColor>,
    },
    Definition {
        tag: Tag,
        change: DefinitionChange,
        members: Vec<String>,
    },
}

#[derive(PartialEq, Eq)]
enum DefinitionChange {
    Name(String),
    Color(Color),
}

#[derive(Clone, PartialEq, Eq)]
enum AddedLabel {
    Existing(Tag),
    New(String),
}

impl AddedLabel {
    fn name(&self) -> &str {
        match self {
            Self::Existing(tag) => &tag.name,
            Self::New(name) => name,
        }
    }
}

#[derive(PartialEq, Eq)]
struct FileChange {
    path: String,
    identity: Identity,
    before: Vec<Tag>,
    add: Vec<AddedLabel>,
    remove: Vec<Tag>,
}

#[derive(PartialEq, Eq)]
struct PlannedColor {
    label: AddedLabel,
    color: Color,
    members: Vec<String>,
}

impl FileChange {
    fn changed(&self) -> bool {
        !self.add.is_empty() || !self.remove.is_empty()
    }
}

#[derive(PartialEq, Eq)]
enum Identity {
    Native(String),
    Stat {
        created: Option<SystemTime>,
        modified: SystemTime,
        length: u64,
    },
}

/// Display data for the exact proposal that the user is being asked to approve.
/// The approval transport keeps the proposal ID and the actual write plan separate.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TagConfirmation {
    Files {
        files: Vec<FileTagDelta>,
        created_tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        color_changes: Vec<TagColorDelta>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        label_colors: BTreeMap<String, u8>,
    },
    Rename {
        from: String,
        to: String,
        affected: usize,
    },
    Color {
        tag: String,
        from: u8,
        to: u8,
        affected: usize,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileTagDelta {
    pub path: String,
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TagColorDelta {
    pub tag: String,
    pub from: Option<u8>,
    pub to: u8,
    pub affected: usize,
}

impl Proposal {
    pub fn confirmation(&self) -> Option<TagConfirmation> {
        match &self.plan {
            Plan::Files { files, colors } => {
                let mut label_colors: BTreeMap<_, _> = files
                    .iter()
                    .flat_map(|file| &file.add)
                    .map(|label| {
                        (
                            label.name().to_owned(),
                            match label {
                                AddedLabel::Existing(tag) => tag.color,
                                AddedLabel::New(_) => Color::Gray as u8,
                            },
                        )
                    })
                    .collect();
                label_colors.extend(
                    colors
                        .iter()
                        .map(|change| (change.label.name().to_owned(), change.color as u8)),
                );
                let color_changes = colors
                    .iter()
                    .map(|change| TagColorDelta {
                        tag: change.label.name().to_owned(),
                        from: match &change.label {
                            AddedLabel::Existing(tag) => Some(tag.color),
                            AddedLabel::New(_) => None,
                        },
                        to: change.color as u8,
                        affected: change.members.len()
                            + files
                                .iter()
                                .filter(|file| file.add.contains(&change.label))
                                .count(),
                    })
                    .collect();
                let created_tags = files
                    .iter()
                    .flat_map(|file| &file.add)
                    .filter_map(|label| match label {
                        AddedLabel::New(name) => Some(name.clone()),
                        AddedLabel::Existing(_) => None,
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let files: Vec<_> = files
                    .iter()
                    .filter(|file| file.changed())
                    .map(|file| FileTagDelta {
                        path: file.path.clone(),
                        add: file
                            .add
                            .iter()
                            .map(|label| label.name().to_owned())
                            .collect(),
                        remove: file.remove.iter().map(|label| label.name.clone()).collect(),
                    })
                    .collect();
                (!files.is_empty() || !colors.is_empty()).then_some(TagConfirmation::Files {
                    files,
                    created_tags,
                    color_changes,
                    label_colors,
                })
            }
            Plan::Definition {
                tag,
                change,
                members,
            } => {
                let affected = members.len();
                match change {
                    DefinitionChange::Name(name) if name != &tag.name => {
                        Some(TagConfirmation::Rename {
                            from: tag.name.clone(),
                            to: name.clone(),
                            affected,
                        })
                    }
                    DefinitionChange::Color(color) if *color as u8 != tag.color => {
                        Some(TagConfirmation::Color {
                            tag: tag.name.clone(),
                            from: tag.color,
                            to: *color as u8,
                            affected,
                        })
                    }
                    _ => None,
                }
            }
        }
    }
}

impl Store {
    pub fn agent_list(&self, request: List) -> ToolResult<Value> {
        let limit = page_limit(request.limit)?;
        let data = self.snapshot().map_err(Failure::storage)?;
        let query = request.query.as_deref().map(name_key).unwrap_or_default();
        let mut matches = data
            .tags
            .iter()
            .filter_map(|tag| {
                let key = name_key(&tag.name);
                (key.contains(&query) && after_cursor(&key, &request.cursor))
                    .then(|| (key, tag_value(tag)))
            })
            .take(limit + 1)
            .collect::<Vec<_>>();
        let cursor = next_page(&mut matches, limit);
        Ok(
            json!({ "status": "ok", "tags": matches.into_iter().map(|(_, row)| row).collect::<Vec<_>>(), "next_cursor": cursor }),
        )
    }

    pub fn agent_query(&self, visible: &[ProjectFile], request: Query) -> ToolResult<Value> {
        let limit = page_limit(request.limit)?;
        let paths = request
            .paths
            .as_ref()
            .map(|paths| self.targets(paths, visible))
            .transpose()?;
        let directory = request
            .under_directory
            .as_deref()
            .map(|folder| self.directory(folder))
            .transpose()?;
        let names = request.tags.as_deref().unwrap_or_default();
        if names.len() > 20 {
            return Err(Failure::new("invalid_request", crate::i18n::tags::ERR_AGENT_TAG_MAX_QUERY.get()));
        }
        let name = request
            .name_contains
            .as_deref()
            .map(name_key)
            .filter(|value| !value.is_empty());
        let extension = request
            .extension
            .as_deref()
            .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        if paths.is_none()
            && directory.is_none()
            && names.is_empty()
            && name.is_none()
            && extension.is_none()
        {
            return Err(Failure::new(
                "invalid_request",
                crate::i18n::tags::ERR_AGENT_TAG_FILTER_REQUIRED.get(),
            ));
        }
        let mut db = self.open(false).map_err(Failure::storage)?;
        let read = db
            .as_mut()
            .map(Connection::transaction)
            .transpose()
            .map_err(Failure::storage)?;
        let db = read.as_deref();
        let data = read_snapshot(db)?;
        let tag_ids = names
            .iter()
            .map(|name| tag_named(&data, name).map(|tag| tag.id.clone()))
            .collect::<ToolResult<Vec<_>>>()?;
        let mut candidates: Vec<_> = visible
            .iter()
            .filter(|file| {
                after_cursor(&file.path, &request.cursor)
                    && paths
                        .as_ref()
                        .is_none_or(|paths| paths.contains(&file.path))
                    && directory
                        .as_ref()
                        .is_none_or(|folder| Path::new(&file.path).starts_with(folder))
            })
            .collect();
        candidates.sort_unstable_by(|a, b| a.path.cmp(&b.path));

        let mut found = Vec::new();
        for file in candidates {
            let path = Path::new(&file.path);
            let filename = path
                .file_name()
                .expect("project file has name")
                .to_string_lossy();
            let suffix = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default();
            if name
                .as_ref()
                .is_some_and(|name| !filename.to_lowercase().contains(name))
                || extension
                    .as_ref()
                    .is_some_and(|ext| !suffix.eq_ignore_ascii_case(ext))
            {
                continue;
            }
            let bound = data
                .assigned
                .get(&file.path)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if !request.match_kind.matches(&tag_ids, bound) {
                continue;
            }
            let full = match crate::files::project_path(self.project_root(), &file.path) {
                Ok(path) if path == self.absolute(&file.path) && path.is_file() => path,
                Ok(_) => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(Failure::storage(error)),
            };
            // A same-path replacement can reach the tree before reconciliation.
            let actual = if bound.is_empty()
                || binding_matches(
                    db.expect("assigned tags come from the database"),
                    &file.path,
                    &disk_identity(&full),
                )? {
                bound
            } else {
                &[]
            };
            if !request.match_kind.matches(&tag_ids, actual) {
                continue;
            }
            found.push((file.path.clone(), json!({
                "path": file.path, "extension": suffix.to_ascii_lowercase(),
                "tags": data.tags.iter().filter(|tag| actual.contains(&tag.id)).map(|tag| &tag.name).collect::<Vec<_>>(),
            })));
            if found.len() > limit {
                break;
            }
        }
        let cursor = next_page(&mut found, limit);
        Ok(
            json!({ "status": "ok", "files": found.into_iter().map(|(_, row)| row).collect::<Vec<_>>(), "next_cursor": cursor }),
        )
    }

    pub fn agent_preview(&self, visible: &[ProjectFile], edit: Edit) -> ToolResult<Proposal> {
        let mut db = self.open(false).map_err(Failure::storage)?;
        let read = db
            .as_mut()
            .map(Connection::transaction)
            .transpose()
            .map_err(Failure::storage)?;
        let plan = self.plan_agent_edit(read.as_deref(), visible, &edit)?;
        Ok(Proposal { edit, plan })
    }

    pub fn agent_commit(
        &self,
        visible: &[ProjectFile],
        proposal: Proposal,
    ) -> ToolResult<(Snapshot, Value)> {
        let create = matches!(&proposal.plan, Plan::Files { files, .. } if files.iter().any(|file| {
            file.add.iter().any(|tag| matches!(tag, AddedLabel::New(_)))
        }));
        let mut db = self
            .open(create)
            .map_err(Failure::storage)?
            .ok_or_else(stale)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(Failure::storage)?;
        // Only the confirmed paths, memberships and relevant tag definitions
        // participate in the comparison. Edits in another folder can proceed.
        let live = self
            .plan_agent_edit(Some(&tx), visible, &proposal.edit)
            .map_err(|error| {
                if error.code == "tag_store_error" {
                    error
                } else {
                    stale()
                }
            })?;
        if live != proposal.plan {
            return Err(stale());
        }
        let result = self
            .apply_agent_edit(&tx, &live)
            .map_err(Failure::storage)?;
        let snapshot = snapshot_after_changes(&tx).map_err(Failure::storage)?;
        tx.commit().map_err(Failure::storage)?;
        Ok((snapshot, result))
    }

    fn plan_agent_edit(
        &self,
        db: Option<&Connection>,
        visible: &[ProjectFile],
        edit: &Edit,
    ) -> ToolResult<Plan> {
        let data = read_snapshot(db)?;
        match edit {
            Edit::Files(edit) => {
                if (edit.add.is_empty() && edit.remove.is_empty())
                    || edit.add.len() > 20
                    || edit.remove.len() > 20
                {
                    return Err(Failure::new(
                        "invalid_request",
                        crate::i18n::tags::ERR_AGENT_TAG_ADD_REMOVE_REQUIRED.get(),
                    ));
                }
                let mut add = BTreeMap::new();
                for name in &edit.add {
                    let name = label(name)
                        .map_err(|error| Failure::new("invalid_request", error.to_string()))?;
                    let tag = match tag_named(&data, name) {
                        Ok(tag) => AddedLabel::Existing(tag.clone()),
                        Err(_) if edit.create_missing => AddedLabel::New(name.to_owned()),
                        Err(error) => return Err(error),
                    };
                    add.entry(name_key(name)).or_insert(tag);
                }
                let mut remove = BTreeMap::new();
                for name in &edit.remove {
                    let tag = tag_named(&data, name)?;
                    let key = name_key(&tag.name);
                    if add.contains_key(&key) {
                        return Err(Failure::new(
                            "invalid_request",
                            crate::i18n::tags::err_agent_tag_both_add_remove(&tag.name),
                        ));
                    }
                    remove.insert(key, tag);
                }
                if edit.tag_colors.len() > 20 {
                    return Err(Failure::new(
                        "invalid_request",
                        crate::i18n::tags::ERR_AGENT_TAG_MAX_COLORS.get(),
                    ));
                }
                let mut colors = BTreeMap::new();
                for request in &edit.tag_colors {
                    let key = name_key(&request.tag);
                    let label = add.get(&key).ok_or_else(|| {
                        Failure::new(
                            "invalid_request",
                            crate::i18n::tags::err_agent_tag_color_must_be_in_add(&request.tag),
                        )
                    })?;
                    if colors.insert(key, (label, request.color)).is_some() {
                        return Err(Failure::new(
                            "invalid_request",
                            crate::i18n::tags::err_agent_tag_duplicate_color(label.name()),
                        ));
                    }
                }

                // Rebuild the directory's target set at commit as well. A
                // file added during confirmation must never widen an approval.
                let targets = match (&edit.paths, &edit.under_directory) {
                    (Some(paths), None) => self.targets(paths, visible)?,
                    (None, Some(folder)) => {
                        let folder = self.directory(folder)?;
                        visible
                            .iter()
                            .filter(|file| Path::new(&file.path).starts_with(&folder))
                            .map(|file| file.path.clone())
                            .collect()
                    }
                    _ => {
                        return Err(Failure::new(
                            "invalid_request",
                            crate::i18n::tags::ERR_AGENT_TAG_PATHS_OR_DIR.get(),
                        ));
                    }
                };
                let mut result = Vec::with_capacity(targets.len());
                for path in targets {
                    let full = crate::files::project_path(self.project_root(), &path).map_err(
                        |error| Failure::new("file_unavailable", format!("{path}: {error}")),
                    )?;
                    if full != self.absolute(&path) || !full.is_file() {
                        return Err(Failure::new(
                            "file_unavailable",
                            crate::i18n::tags::err_agent_tag_path_changed(&path),
                        ));
                    }
                    let metadata = fs::metadata(&full).map_err(Failure::storage)?;
                    let disk = disk_identity(&full);
                    let identity = match &disk {
                        Some(id) => Identity::Native(id.clone()),
                        None => Identity::Stat {
                            created: metadata.created().ok(),
                            modified: metadata.modified().map_err(Failure::storage)?,
                            length: metadata.len(),
                        },
                    };
                    let before = tags_for(&data, &path);
                    let before = if before.is_empty()
                        || binding_matches(
                            db.expect("assigned tags come from the database"),
                            &path,
                            &disk,
                        )? {
                        before
                    } else {
                        Vec::new()
                    };
                    let add = add
                        .values()
                        .filter(|tag| match tag {
                            AddedLabel::Existing(tag) => !before.contains(tag),
                            AddedLabel::New(_) => true,
                        })
                        .cloned()
                        .collect();
                    let remove = remove
                        .values()
                        .filter(|tag| before.contains(tag))
                        .map(|tag| (*tag).clone())
                        .collect();
                    result.push(FileChange {
                        path,
                        identity,
                        before,
                        add,
                        remove,
                    });
                }
                let mut planned_colors = Vec::new();
                if !result.is_empty() {
                    // Changing an existing definition affects all of its
                    // references, including files outside this selection.
                    for (label, color) in colors.into_values() {
                        let members = match label {
                            AddedLabel::Existing(tag) if tag.color == color as u8 => continue,
                            AddedLabel::Existing(tag) => {
                                tag_members(db.expect("existing tag has a store"), &tag.id)?
                            }
                            AddedLabel::New(_) => Vec::new(),
                        };
                        planned_colors.push(PlannedColor {
                            label: label.clone(),
                            color,
                            members,
                        });
                    }
                }
                Ok(Plan::Files {
                    files: result,
                    colors: planned_colors,
                })
            }
            Edit::Definition(edit) => {
                let (name, change) = match edit {
                    DefinitionEdit::Rename { tag, new_name } => {
                        let new_name = label(new_name)
                            .map_err(|error| Failure::new("invalid_request", error.to_string()))?;
                        if name_key(tag) != name_key(new_name) && tag_named(&data, new_name).is_ok()
                        {
                            return Err(Failure::new(
                                "tag_exists",
                                crate::i18n::tags::err_agent_tag_exists(new_name),
                            ));
                        }
                        (tag, DefinitionChange::Name(new_name.to_owned()))
                    }
                    DefinitionEdit::SetColor { tag, color } => {
                        (tag, DefinitionChange::Color(*color))
                    }
                };
                let tag = tag_named(&data, name)?.clone();
                // Renaming a definition affects references in the trash and in
                // missing-file records as well as references in the live tree.
                let members =
                    tag_members(db.expect("tag definitions come from the database"), &tag.id)?;
                Ok(Plan::Definition {
                    tag,
                    change,
                    members,
                })
            }
        }
    }

    fn apply_agent_edit(&self, db: &Connection, plan: &Plan) -> Result<Value> {
        match plan {
            Plan::Files { files, colors } => {
                let mut create: BTreeMap<&str, Vec<String>> = BTreeMap::new();
                let mut add: BTreeMap<&str, Vec<String>> = BTreeMap::new();
                let mut remove: BTreeMap<&str, Vec<String>> = BTreeMap::new();
                let mut changed = Vec::new();
                for file in files.iter().filter(|file| file.changed()) {
                    for tag in &file.add {
                        let paths = match tag {
                            AddedLabel::New(name) => create.entry(name).or_default(),
                            AddedLabel::Existing(tag) => add.entry(&tag.id).or_default(),
                        };
                        paths.push(file.path.clone());
                    }
                    for tag in &file.remove {
                        remove.entry(&tag.id).or_default().push(file.path.clone());
                    }
                    changed.push(&file.path);
                }
                for (name, paths) in &create {
                    self.apply_change(db, Change::Create { name, paths })?;
                }
                for (tag, paths) in add {
                    self.apply_change(
                        db,
                        Change::Assign {
                            tag,
                            paths: &paths,
                            add: true,
                        },
                    )?;
                }
                for (tag, paths) in remove {
                    self.apply_change(
                        db,
                        Change::Assign {
                            tag,
                            paths: &paths,
                            add: false,
                        },
                    )?;
                }
                for change in colors {
                    let id = match &change.label {
                        AddedLabel::Existing(tag) => tag.id.clone(),
                        // The same transaction has already created all planned labels.
                        AddedLabel::New(name) => db.query_row(
                            "SELECT id FROM tags WHERE name_key = ?1",
                            [name_key(name)],
                            |row| row.get(0),
                        )?,
                    };
                    self.apply_change(
                        db,
                        Change::Color {
                            tag: &id,
                            color: change.color as u8,
                        },
                    )?;
                }
                let mut result = json!({ "status": "ok", "updated": changed.len(), "paths": changed, "created_tags": create.keys().collect::<Vec<_>>() });
                if !colors.is_empty() {
                    result["tag_colors"] = json!(
                        colors
                            .iter()
                            .map(|change| json!({
                                "tag": change.label.name(), "color": change.color,
                            }))
                            .collect::<Vec<_>>()
                    );
                }
                Ok(result)
            }
            Plan::Definition {
                tag,
                change,
                members,
            } => {
                let change = match change {
                    DefinitionChange::Name(name) => Change::Rename { tag: &tag.id, name },
                    DefinitionChange::Color(color) => Change::Color {
                        tag: &tag.id,
                        color: *color as u8,
                    },
                };
                self.apply_change(db, change)?;
                Ok(json!({ "status": "ok", "definition_changed": true, "updated": members.len() }))
            }
        }
    }

    fn project_root(&self) -> &Path {
        self.root
            .as_deref()
            .expect("agent tools require a directory workspace")
    }

    fn directory(&self, input: &str) -> ToolResult<PathBuf> {
        let root = self.project_root();
        let full = crate::files::project_path(root, input).map_err(|error| {
            let code = if error.kind() == io::ErrorKind::PermissionDenied {
                "invalid_path"
            } else {
                "directory_not_found"
            };
            Failure::new(code, crate::i18n::tags::err_agent_open_dir(input, &error))
        })?;
        let relative = full
            .strip_prefix(root)
            .expect("validated project directory");
        if !full.is_dir()
            || relative.components().any(|component| {
                crate::files::EXCLUDED_DIRECTORIES
                    .iter()
                    .any(|name| component.as_os_str() == *name)
            })
        {
            return Err(Failure::new(
                "directory_not_found",
                crate::i18n::tags::err_agent_dir_not_in_workspace(input),
            ));
        }
        Ok(relative.to_owned())
    }

    fn targets(&self, input: &[String], visible: &[ProjectFile]) -> ToolResult<BTreeSet<String>> {
        if input.is_empty() || input.len() > 100 {
            return Err(Failure::new(
                "invalid_request",
                crate::i18n::tags::ERR_AGENT_FILE_COUNT.get(),
            ));
        }
        let inventory: BTreeSet<_> = visible.iter().map(|file| file.path.as_str()).collect();
        let root = self.project_root();
        let mut targets = BTreeSet::new();
        for path in input {
            let full = crate::files::project_path(root, path).map_err(|error| {
                let code = match error.kind() {
                    io::ErrorKind::PermissionDenied => "invalid_path",
                    io::ErrorKind::NotFound => "file_not_found",
                    _ => "file_unavailable",
                };
                Failure::new(code, format!("{path}: {error}"))
            })?;
            let relative = full
                .strip_prefix(root)
                .expect("validated project path")
                .to_string_lossy()
                .replace('\\', "/");
            if full.is_dir() {
                return Err(Failure::new(
                    "file_not_found",
                    crate::i18n::tags::err_agent_paths_files_only(path),
                ));
            }
            if !inventory.contains(relative.as_str()) || !full.is_file() {
                return Err(Failure::new(
                    "file_not_found",
                    crate::i18n::tags::err_agent_unsupported_file(path),
                ));
            }
            targets.insert(relative);
        }
        Ok(targets)
    }
}

fn read_snapshot(db: Option<&Connection>) -> ToolResult<Snapshot> {
    db.map(snapshot)
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(Failure::storage)
}

fn tag_members(db: &Connection, tag: &str) -> ToolResult<Vec<String>> {
    db.prepare("SELECT file_id FROM file_tags WHERE tag_id = ?1 ORDER BY file_id")
        .and_then(|mut statement| statement.query_map([tag], |row| row.get(0))?.collect())
        .map_err(Failure::storage)
}

fn binding_matches(db: &Connection, path: &str, disk: &Option<String>) -> ToolResult<bool> {
    Ok(active_file(db, path)
        .map_err(Failure::storage)?
        .is_some_and(|file| same_or_unavailable(&file.disk, disk)))
}

fn tags_for(data: &Snapshot, path: &str) -> Vec<Tag> {
    data.tags
        .iter()
        .filter(|tag| data.has(path, &tag.id))
        .cloned()
        .collect()
}

fn tag_named<'a>(data: &'a Snapshot, name: &str) -> ToolResult<&'a Tag> {
    let key = name_key(name);
    data.tags
        .iter()
        .find(|tag| name_key(&tag.name) == key)
        .ok_or_else(|| {
            Failure::new(
                "unknown_tag",
                crate::i18n::tags::err_agent_tag_not_found(name.trim()),
            )
        })
}

fn tag_value(tag: &Tag) -> Value {
    let color = COLORS.get(tag.color as usize).unwrap_or(&COLORS[0]);
    json!({ "name": tag.name, "color": color })
}

fn page_limit(limit: Option<usize>) -> ToolResult<usize> {
    let limit = limit.unwrap_or(50);
    if (1..=100).contains(&limit) {
        Ok(limit)
    } else {
        Err(Failure::new(
            "invalid_request",
            crate::i18n::tags::ERR_AGENT_LIMIT_RANGE.get(),
        ))
    }
}

fn after_cursor(key: &str, cursor: &Option<String>) -> bool {
    cursor.as_deref().is_none_or(|cursor| key > cursor)
}

fn next_page(rows: &mut Vec<(String, Value)>, limit: usize) -> Option<String> {
    if rows.len() <= limit {
        return None;
    }
    rows.pop();
    rows.last().map(|(key, _)| key.clone())
}

fn stale() -> Failure {
    Failure::new(
        "stale_preview",
        crate::i18n::tags::ERR_AGENT_STALE_PREVIEW.get(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::list_files;

    #[test]
    fn directory_queries_recurse_by_path_component_and_keep_other_selectors_distinct() {
        let root = tempfile::tempdir().unwrap();
        let folder = "Notes/C++特性解读";
        let nested = "Notes/C++特性解读/子目录";
        let sibling = "Notes/C++特性解读存档";
        fs::create_dir_all(root.path().join(nested)).unwrap();
        fs::create_dir(root.path().join(sibling)).unwrap();
        fs::write(root.path().join(folder).join("a.md"), "a").unwrap();
        fs::write(root.path().join(nested).join("b.pdf"), "b").unwrap();
        fs::write(root.path().join(sibling).join("c.md"), "c").unwrap();
        let store = Store::project(root.path());
        let inventory = list_files(root.path()).unwrap();

        let query = serde_json::from_value(json!({ "under_directory": folder })).unwrap();
        let result = store.agent_query(&inventory, query).unwrap();
        assert_eq!(
            result["files"],
            json!([
                { "path": "Notes/C++特性解读/a.md", "extension": "md", "tags": [] },
                { "path": "Notes/C++特性解读/子目录/b.pdf", "extension": "pdf", "tags": [] },
            ])
        );
        let query = serde_json::from_value(json!({ "name_contains": "C++特性解读" })).unwrap();
        assert_eq!(
            store.agent_query(&inventory, query).unwrap()["files"],
            json!([])
        );
        let query = serde_json::from_value(json!({ "paths": [folder] })).unwrap();
        let error = store.agent_query(&inventory, query).unwrap_err();
        assert_eq!(error.code, "file_not_found");
        assert!(error.message.contains("under_directory"));
        let query = serde_json::from_value(json!({ "limit": 100 })).unwrap();
        assert_eq!(
            store.agent_query(&inventory, query).unwrap_err().code,
            "invalid_request"
        );
        let query =
            serde_json::from_value(json!({ "under_directory": ".", "limit": 100 })).unwrap();
        assert_eq!(
            store.agent_query(&inventory, query).unwrap()["files"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn directory_approval_requires_a_fresh_preview_when_its_file_set_changes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("Notes/Sub")).unwrap();
        fs::create_dir(root.path().join("NotesBackup")).unwrap();
        fs::write(root.path().join("Notes/a.md"), "a").unwrap();
        fs::write(root.path().join("Notes/Sub/b.pdf"), "b").unwrap();
        fs::write(root.path().join("NotesBackup/c.md"), "c").unwrap();
        let store = Store::project(root.path());
        let edit = || {
            Edit::Files(
                serde_json::from_value(json!({
                    "under_directory": "Notes", "add": ["重点"], "create_missing": true,
                    "tag_colors": [{ "tag": "重点", "color": "red" }],
                }))
                .unwrap(),
            )
        };
        let inventory = list_files(root.path()).unwrap();
        let proposal = store.agent_preview(&inventory, edit()).unwrap();
        fs::write(root.path().join("Notes/late.md"), "late").unwrap();
        let inventory = list_files(root.path()).unwrap();

        let error = store.agent_commit(&inventory, proposal).unwrap_err();
        assert_eq!(error.code, "stale_preview");
        assert!(store.snapshot().unwrap().tags.is_empty());

        let proposal = store.agent_preview(&inventory, edit()).unwrap();
        let (snapshot, result) = store.agent_commit(&inventory, proposal).unwrap();
        assert_eq!(result["updated"], 3);
        assert_eq!(
            result["tag_colors"],
            json!([{ "tag": "重点", "color": "red" }])
        );
        assert_eq!(snapshot.tags[0].color, Color::Red as u8);
        let tag = &snapshot.tags[0].id;
        assert!(snapshot.has("Notes/a.md", tag));
        assert!(snapshot.has("Notes/Sub/b.pdf", tag));
        assert!(snapshot.has("Notes/late.md", tag));
        assert!(!snapshot.has("NotesBackup/c.md", tag));
    }

    #[test]
    fn sharing_an_existing_color_change_with_file_assignment_previews_the_global_impact() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("target.md"), "target").unwrap();
        fs::write(root.path().join("outside.md"), "outside").unwrap();
        let store = Store::project(root.path());
        let original = store
            .change(Change::Create {
                name: "重点",
                paths: &["outside.md".into()],
            })
            .unwrap();
        let id = &original.tags[0].id;
        let edit = serde_json::from_value(json!({
            "paths": ["target.md"], "add": ["重点"],
            "tag_colors": [{ "tag": "重点", "color": "blue" }],
        }))
        .unwrap();
        let inventory = list_files(root.path()).unwrap();
        let proposal = store.agent_preview(&inventory, Edit::Files(edit)).unwrap();
        let preview = serde_json::to_value(proposal.confirmation()).unwrap();
        assert_eq!(
            preview["color_changes"],
            json!([
                { "tag": "重点", "from": 0, "to": 1, "affected": 2 },
            ])
        );
        assert_eq!(preview["label_colors"]["重点"], 1);
        let (snapshot, result) = store.agent_commit(&inventory, proposal).unwrap();
        assert_eq!(result["updated"], 1);
        assert_eq!(snapshot.tags[0].color, Color::Blue as u8);
        assert!(snapshot.has("target.md", id));
        assert!(snapshot.has("outside.md", id));
    }

    #[test]
    fn queries_intersect_or_union_labels_and_page_by_project_path() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("讲义")).unwrap();
        fs::write(root.path().join("讲义/a.pdf"), "a").unwrap();
        fs::write(root.path().join("b.pdf"), "b").unwrap();
        let store = Store::project(root.path());
        store
            .change(Change::Create {
                name: "重点",
                paths: &["讲义/a.pdf".into(), "b.pdf".into()],
            })
            .unwrap();
        store
            .change(Change::Create {
                name: "待复习",
                paths: &["讲义/a.pdf".into()],
            })
            .unwrap();
        let inventory = list_files(root.path()).unwrap();

        let all =
            serde_json::from_value(json!({ "tags": ["重点", "待复习"], "extension": ".PDF" }))
                .unwrap();
        let result = store.agent_query(&inventory, all).unwrap();
        assert_eq!(result["files"][0]["path"], "讲义/a.pdf");
        assert_eq!(result["files"].as_array().unwrap().len(), 1);

        let any = serde_json::from_value(
            json!({ "tags": ["重点", "待复习"], "match": "any", "limit": 1 }),
        )
        .unwrap();
        let result = store.agent_query(&inventory, any).unwrap();
        assert_eq!(result["files"][0]["path"], "b.pdf");
        let next = serde_json::from_value(json!({
            "tags": ["重点", "待复习"], "match": "any", "limit": 1, "cursor": result["next_cursor"],
        }))
        .unwrap();
        let result = store.agent_query(&inventory, next).unwrap();
        assert_eq!(result["files"][0]["path"], "讲义/a.pdf");
        assert!(result["next_cursor"].is_null());
    }

    #[test]
    fn approved_batch_creates_a_label_and_changes_only_the_selected_membership() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.md"), "a").unwrap();
        fs::write(root.path().join("b.pdf"), "b").unwrap();
        fs::write(root.path().join("other.md"), "other").unwrap();
        let store = Store::project(root.path());
        let initial = store
            .change(Change::Create {
                name: "待复习",
                paths: &["a.md".into(), "b.pdf".into(), "other.md".into()],
            })
            .unwrap();
        let old_id = &initial.tags[0].id;
        let state = store
            .change(Change::Create {
                name: "重点",
                paths: &["a.md".into()],
            })
            .unwrap();
        let keep_id = &tag_named(&state, "重点").unwrap().id;
        let inventory = list_files(root.path()).unwrap();
        let edit = serde_json::from_value(json!({
            "paths": ["a.md", "b.pdf"], "add": ["已掌握"], "remove": ["待复习"], "create_missing": true,
        })).unwrap();

        let proposal = store.agent_preview(&inventory, Edit::Files(edit)).unwrap();
        assert_eq!(
            serde_json::to_value(proposal.confirmation()).unwrap(),
            json!({
                "kind": "files",
                "files": [
                    { "path": "a.md", "add": ["已掌握"], "remove": ["待复习"] },
                    { "path": "b.pdf", "add": ["已掌握"], "remove": ["待复习"] },
                ],
                "created_tags": ["已掌握"],
                "label_colors": { "已掌握": 0 },
            }),
        );
        assert!(tag_named(&store.snapshot().unwrap(), "已掌握").is_err());
        let (snapshot, result) = store.agent_commit(&inventory, proposal).unwrap();
        let new_id = &tag_named(&snapshot, "已掌握").unwrap().id;
        assert_eq!(result["updated"], 2);
        assert!(snapshot.has("a.md", keep_id));
        assert!(snapshot.has("a.md", new_id));
        assert!(snapshot.has("b.pdf", new_id));
        assert!(!snapshot.has("a.md", old_id));
        assert!(!snapshot.has("b.pdf", old_id));
        assert!(snapshot.has("other.md", old_id));
        assert!(!snapshot.has("other.md", new_id));
    }

    #[test]
    fn replacement_after_preview_rejects_the_entire_batch() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.md"), "a").unwrap();
        fs::write(root.path().join("b.pdf"), "b").unwrap();
        let store = Store::project(root.path());
        let inventory = list_files(root.path()).unwrap();
        let edit = serde_json::from_value(json!({
            "paths": ["a.md", "b.pdf"], "add": ["重点"], "create_missing": true,
        }))
        .unwrap();
        let proposal = store.agent_preview(&inventory, Edit::Files(edit)).unwrap();
        fs::rename(root.path().join("b.pdf"), root.path().join("backup.pdf")).unwrap();
        fs::write(root.path().join("b.pdf"), "replacement").unwrap();

        let error = store.agent_commit(&inventory, proposal).unwrap_err();
        assert_eq!(error.code, "stale_preview");
        assert!(store.snapshot().unwrap().tags.is_empty());
    }

    #[test]
    fn rename_checks_the_confirmed_impact_and_preserves_tag_identity() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.md"), "a").unwrap();
        fs::write(root.path().join("b.pdf"), "b").unwrap();
        let store = Store::project(root.path());
        let data = store
            .change(Change::Create {
                name: "待复习",
                paths: &["a.md".into()],
            })
            .unwrap();
        let tag = &data.tags[0].id;
        let edit = || {
            Edit::Definition(DefinitionEdit::Rename {
                tag: "待复习".into(),
                new_name: "待回顾".into(),
            })
        };
        let proposal = store.agent_preview(&[], edit()).unwrap();
        assert_eq!(
            serde_json::to_value(proposal.confirmation()).unwrap(),
            json!({ "kind": "rename", "from": "待复习", "to": "待回顾", "affected": 1 }),
        );
        store
            .change(Change::Assign {
                tag,
                paths: &["b.pdf".into()],
                add: true,
            })
            .unwrap();
        assert_eq!(
            store.agent_commit(&[], proposal).unwrap_err().code,
            "stale_preview"
        );

        let proposal = store.agent_preview(&[], edit()).unwrap();
        let (current, result) = store.agent_commit(&[], proposal).unwrap();
        assert_eq!(result["updated"], 2);
        assert_eq!(tag_named(&current, "待回顾").unwrap().id, *tag);
        assert!(current.has("a.md", tag));
        assert!(current.has("b.pdf", tag));
    }
}
