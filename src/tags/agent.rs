//! Agent-facing queries and proposals for one directory workspace. The store
//! renders the proposed changes before the user sees them and recomputes the
//! same proposal inside the write transaction before applying it.

use super::*;
use crate::model::ProjectFile;
use serde::Deserialize;
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
    paths: Vec<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    create_missing: bool,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DefinitionEdit {
    Rename { tag: String, new_name: String },
    SetColor { tag: String, color: Color },
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
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
const COLORS: [(&str, &str); 6] = [
    ("gray", "灰色"),
    ("blue", "蓝色"),
    ("green", "绿色"),
    ("orange", "橙色"),
    ("red", "红色"),
    ("purple", "紫色"),
];

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
    Files(Vec<FileChange>),
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

impl Proposal {
    pub fn confirmation(&self) -> Option<String> {
        match &self.plan {
            Plan::Files(files) => {
                let mut lines = Vec::new();
                let mut created = BTreeSet::new();
                for file in files.iter().filter(|file| file.changed()) {
                    let mut effects = Vec::new();
                    for label in &file.add {
                        if let AddedLabel::New(name) = label {
                            created.insert(name.as_str());
                        }
                        effects.push(format!("＋{}", label.name()));
                    }
                    for label in &file.remove {
                        effects.push(format!("－{}", label.name));
                    }
                    lines.push(format!("{}\n  {}", file.path, effects.join("  ")));
                }
                if lines.is_empty() {
                    return None;
                }
                let mut header = format!("将更新 {} 个文件的标签", lines.len());
                if !created.is_empty() {
                    header.push_str(&format!(
                        "\n新建标签　{}",
                        created.into_iter().collect::<Vec<_>>().join("、")
                    ));
                }
                Some(format!("{header}\n\n{}", lines.join("\n\n")))
            }
            Plan::Definition {
                tag,
                change,
                members,
            } => {
                let summary = match change {
                    DefinitionChange::Name(name) if name != &tag.name => {
                        format!("将「{}」重命名为「{name}」", tag.name)
                    }
                    DefinitionChange::Color(color) if *color as u8 != tag.color => {
                        format!("将「{}」改为{}", tag.name, COLORS[*color as usize].1)
                    }
                    _ => return None,
                };
                Some(format!("{summary}\n影响 {} 个关联文件", members.len()))
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
        let names = request.tags.as_deref().unwrap_or_default();
        if names.len() > 20 {
            return Err(Failure::new("invalid_request", "每次最多查询 20 个标签。"));
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
        if paths.is_none() && names.is_empty() && name.is_none() && extension.is_none() {
            return Err(Failure::new(
                "invalid_request",
                "请指定文件、标签、文件名或扩展名。",
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
        let create = matches!(&proposal.plan, Plan::Files(files) if files.iter().any(|file| {
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
                        "请指定要添加或移除的标签，每组最多 20 个。",
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
                            format!("不能同时添加和移除「{}」。", tag.name),
                        ));
                    }
                    remove.insert(key, tag);
                }

                let targets = self.targets(&edit.paths, visible)?;
                let mut result = Vec::with_capacity(targets.len());
                for path in targets {
                    let full = self.absolute(&path);
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
                Ok(Plan::Files(result))
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
                                format!("已存在标签「{new_name}」。"),
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
                let members = db
                    .expect("tag definitions come from the database")
                    .prepare("SELECT file_id FROM file_tags WHERE tag_id = ?1 ORDER BY file_id")
                    .and_then(|mut statement| {
                        statement.query_map([&tag.id], |row| row.get(0))?.collect()
                    })
                    .map_err(Failure::storage)?;
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
            Plan::Files(files) => {
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
                Ok(
                    json!({ "status": "ok", "updated": changed.len(), "paths": changed, "created_tags": create.keys().collect::<Vec<_>>() }),
                )
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

    fn targets(&self, input: &[String], visible: &[ProjectFile]) -> ToolResult<BTreeSet<String>> {
        if input.is_empty() || input.len() > 100 {
            return Err(Failure::new(
                "invalid_request",
                "每次请选择 1 到 100 个具体文件。",
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
                Failure::new(code, format!("{path}：{error}"))
            })?;
            let relative = full
                .strip_prefix(root)
                .expect("validated project path")
                .to_string_lossy()
                .replace('\\', "/");
            if !inventory.contains(relative.as_str()) || !full.is_file() {
                return Err(Failure::new(
                    "file_not_found",
                    format!("项目文件树中没有此受支持文件：{path}"),
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
                format!("找不到标签「{}」，请查看当前空间的标签。", name.trim()),
            )
        })
}

fn tag_value(tag: &Tag) -> Value {
    let color = COLORS.get(tag.color as usize).unwrap_or(&COLORS[0]).0;
    json!({ "name": tag.name, "color": color })
}

fn page_limit(limit: Option<usize>) -> ToolResult<usize> {
    let limit = limit.unwrap_or(50);
    if (1..=100).contains(&limit) {
        Ok(limit)
    } else {
        Err(Failure::new(
            "invalid_request",
            "每页数量必须在 1 到 100 之间。",
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
        "确认期间目标文件或标签发生了变化，请重新检索并预览。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::list_files;

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
        assert!(
            proposal
                .confirmation()
                .unwrap()
                .contains("b.pdf\n  ＋已掌握  －待复习")
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
        assert!(
            proposal
                .confirmation()
                .unwrap()
                .contains("影响 1 个关联文件")
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
