//! Binds agent proposals to the initiating project and conversation. Approval
//! arrives through the host's existing per-conversation confirmation card.

use super::*;
use crate::tags::{
    self, Change, Store,
    agent::{Edit, Failure, Proposal, Request, TagConfirmation, ToolResult},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) type TagProposals = BTreeMap<(String, String, Uuid), PendingTagEdit>;

pub(super) struct PendingTagEdit {
    request_id: String,
    edit: Proposal,
}

impl Workspace {
    pub(super) fn tag_request(&mut self, request: &Value, cx: &mut Context<Self>) {
        if request["type"] == "tag_cancel" {
            self.tag_proposals.retain(|(project, session, _), pending| {
                request["projectKey"].as_str() != Some(project)
                    || request["sessionId"].as_str() != Some(session)
                    || request["id"].as_str() != Some(pending.request_id.as_str())
            });
            return;
        }
        let result = self.handle_tag_request(request, cx);
        match result {
            Ok(Some(result)) => self.ai.read(cx).respond_tag(request, result),
            Ok(None) => {}
            Err(error) => self.ai.read(cx).respond_tag(request, error.result()),
        }
    }

    fn tag_tool_store(&self, request: &Value) -> ToolResult<Store> {
        let project = self.directory_workspace().ok_or_else(|| {
            Failure::new(
                "directory_workspace_required",
                crate::i18n::tags::ERR_AGENT_TAGS_PROJECT_OR_DEFAULT.get(),
            )
        })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
        if request["projectKey"].as_str() != Some(crate::ai::project_key(&root).as_str()) {
            return Err(Failure::new(
                "project_not_active",
                crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get(),
            ));
        }
        Ok(Store::project(&root))
    }

    fn handle_tag_request(
        &mut self,
        request: &Value,
        cx: &mut Context<Self>,
    ) -> ToolResult<Option<Value>> {
        let operation: Request = serde_json::from_value(request.clone())
            .map_err(|error| Failure::new("invalid_request", error.to_string()))?;
        let project = request["projectKey"].as_str().expect("host tag project");
        let session = request["sessionId"].as_str().expect("host tag session");
        let owner = |proposal| (project.to_owned(), session.to_owned(), proposal);
        // Discard must work after the user has switched spaces or stopped the run.
        if let Request::Discard(id) = operation {
            self.tag_proposals.remove(&owner(id.proposal_id));
            return Ok(Some(json!({ "status": "ok" })));
        }
        let store = self.tag_tool_store(request)?;
        let (id, edit) = match operation {
            Request::List(params) => {
                self.run_tag_query(request, cx, move || store.agent_list(params));
                return Ok(None);
            }
            Request::Query(params) => {
                let inventory = self.files.clone();
                self.run_tag_query(request, cx, move || store.agent_query(&inventory, params));
                return Ok(None);
            }
            Request::PreviewFiles(draft) => (draft.proposal_id, Edit::Files(draft.edit)),
            Request::PreviewDefinition(draft) => (draft.proposal_id, Edit::Definition(draft.edit)),
            Request::Commit(id) => {
                let pending = self
                    .tag_proposals
                    .remove(&owner(id.proposal_id))
                    .ok_or_else(|| {
                        Failure::new("stale_preview", crate::i18n::tags::ERR_AGENT_STALE_TAG_CHANGES.get())
                    })?;
                let (snapshot, result) = store.agent_commit(&self.files, pending.edit)?;
                self.use_tag_snapshot(snapshot, cx);
                return Ok(Some(result));
            }
            Request::Discard(_) => unreachable!("discard can run without an active project"),
        };
        let proposal = store.agent_preview(&self.files, edit)?;
        let Some(confirmation) = proposal.confirmation() else {
            return Ok(Some(json!({ "status": "ok", "updated": 0 })));
        };
        self.tag_proposals.insert(
            owner(id),
            PendingTagEdit {
                request_id: request["id"].as_str().expect("host tag request id").into(),
                edit: proposal,
            },
        );
        Ok(Some(
            json!({ "status": "ok", "confirmation": confirmation }),
        ))
    }

    pub(super) fn undo_tag_confirmations(
        &mut self,
        items: &[TagConfirmation],
        cx: &mut Context<Self>,
    ) {
        for item in items {
            match item {
                TagConfirmation::Files {
                    files,
                    created_tags,
                    color_changes,
                    ..
                } => {
                    let mut remove_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
                    let mut add_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
                    for delta in files {
                        for tag in &delta.add {
                            remove_map
                                .entry(tag.clone())
                                .or_default()
                                .push(delta.path.clone());
                        }
                        for tag in &delta.remove {
                            add_map
                                .entry(tag.clone())
                                .or_default()
                                .push(delta.path.clone());
                        }
                    }
                    for (tag_name, paths) in remove_map {
                        let key = tags::name_key(&tag_name);
                        if let Some(id) = self
                            .tags
                            .snapshot
                            .tags
                            .iter()
                            .find(|t| tags::name_key(&t.name) == key)
                            .map(|t| t.id.clone())
                        {
                            self.change_tags(
                                Change::Assign {
                                    tag: &id,
                                    paths: &paths,
                                    add: false,
                                },
                                cx,
                            );
                        }
                    }
                    for (tag_name, paths) in add_map {
                        let key = tags::name_key(&tag_name);
                        if let Some(id) = self
                            .tags
                            .snapshot
                            .tags
                            .iter()
                            .find(|t| tags::name_key(&t.name) == key)
                            .map(|t| t.id.clone())
                        {
                            self.change_tags(
                                Change::Assign {
                                    tag: &id,
                                    paths: &paths,
                                    add: true,
                                },
                                cx,
                            );
                        } else {
                            self.change_tags(
                                Change::Create {
                                    name: &tag_name,
                                    paths: &paths,
                                },
                                cx,
                            );
                        }
                    }
                    for change in color_changes {
                        if let Some(old_color) = change.from {
                            let key = tags::name_key(&change.tag);
                            if let Some(id) = self
                                .tags
                                .snapshot
                                .tags
                                .iter()
                                .find(|t| tags::name_key(&t.name) == key)
                                .map(|t| t.id.clone())
                            {
                                self.change_tags(
                                    Change::Color {
                                        tag: &id,
                                        color: old_color,
                                    },
                                    cx,
                                );
                            }
                        }
                    }
                    for created_name in created_tags {
                        let key = tags::name_key(created_name);
                        if let Some(id) = self
                            .tags
                            .snapshot
                            .tags
                            .iter()
                            .find(|t| tags::name_key(&t.name) == key)
                            .map(|t| t.id.clone())
                        {
                            let still_used = self
                                .tags
                                .snapshot
                                .assigned
                                .values()
                                .any(|ids| ids.iter().any(|assigned| assigned == &id));
                            if !still_used {
                                self.change_tags(Change::Delete(&id), cx);
                            }
                        }
                    }
                }
                TagConfirmation::Rename { from, to, .. } => {
                    let key = tags::name_key(to);
                    if let Some(id) = self
                        .tags
                        .snapshot
                        .tags
                        .iter()
                        .find(|t| tags::name_key(&t.name) == key)
                        .map(|t| t.id.clone())
                    {
                        self.change_tags(Change::Rename { tag: &id, name: from }, cx);
                    }
                }
                TagConfirmation::Color { tag, from, .. } => {
                    let key = tags::name_key(tag);
                    if let Some(id) = self
                        .tags
                        .snapshot
                        .tags
                        .iter()
                        .find(|t| tags::name_key(&t.name) == key)
                        .map(|t| t.id.clone())
                    {
                        self.change_tags(
                            Change::Color {
                                tag: &id,
                                color: *from,
                            },
                            cx,
                        );
                    }
                }
            }
        }
    }

    fn run_tag_query(
        &mut self,
        request: &Value,
        cx: &mut Context<Self>,
        query: impl FnOnce() -> ToolResult<Value> + Send + 'static,
    ) {
        let task = cx.background_spawn(async move { query() });
        let request = request.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |app, cx| {
                let result = app
                    .tag_tool_store(&request)
                    .and(result)
                    .unwrap_or_else(Failure::result);
                app.ai.read(cx).respond_tag(&request, result);
            });
        })
        .detach();
    }
}
