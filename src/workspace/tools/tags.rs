//! Binds tag proposals to their project and conversation and updates workspace views.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    tags::{Store, agent::{Edit, Failure, Proposal, Request, TagConfirmation, ToolResult}},
};
use gpui_kit::{AppContext, Context};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(in crate::workspace) type TagProposals = BTreeMap<(String, String, Uuid), PendingTagEdit>;

pub(in crate::workspace) struct PendingTagEdit {
    request_id: String,
    edit: Proposal,
}

impl Workspace {
    pub(in crate::workspace) fn tag_request(&mut self, request: &WorkspaceToolRequest, cx: &mut Context<Self>) {
        if request.action == WorkspaceToolAction::Cancel {
            self.tag_proposals.retain(|(project, session, _), pending| {
                request.project_key != *project
                    || request.session_id != *session
                    || request.id != pending.request_id
            });
            return;
        }
        let result = self.handle_tag_request(request, cx);
        match result {
            Ok(Some(result)) => self.ai.read(cx).respond_workspace_tool(request, result),
            Ok(None) => {}
            Err(error) => self.ai.read(cx).respond_workspace_tool(request, error.result()),
        }
    }

    fn tag_tool_store(&self, request: &WorkspaceToolRequest) -> ToolResult<Store> {
        let project = self.directory_workspace().ok_or_else(|| {
            Failure::new(
                "directory_workspace_required",
                crate::i18n::tags::ERR_AGENT_TAGS_PROJECT_OR_DEFAULT.get(),
            )
        })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
        if request.project_key != crate::ai::project_key(&root) {
            return Err(Failure::new(
                "project_not_active",
                crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get(),
            ));
        }
        Ok(Store::project(&root))
    }

    fn handle_tag_request(
        &mut self,
        request: &WorkspaceToolRequest,
        cx: &mut Context<Self>,
    ) -> ToolResult<Option<Value>> {
        let operation: Request = serde_json::from_value(request.arguments.clone())
            .map_err(|error| Failure::new("invalid_request", error.to_string()))?;
        let owner = |proposal| (request.project_key.clone(), request.session_id.clone(), proposal);
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
                let inventory = self.tree.files().to_vec();
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
                let (snapshot, result) = store.agent_commit(self.tree.files(), pending.edit)?;
                self.use_tag_snapshot(snapshot, cx);
                return Ok(Some(result));
            }
            Request::Discard(_) => unreachable!("discard can run without an active project"),
        };
        let proposal = store.agent_preview(self.tree.files(), edit)?;
        let Some(confirmation) = proposal.confirmation() else {
            return Ok(Some(json!({ "status": "ok", "updated": 0 })));
        };
        self.tag_proposals.insert(
            owner(id),
            PendingTagEdit {
                request_id: request.id.clone(),
                edit: proposal,
            },
        );
        Ok(Some(
            json!({ "status": "ok", "confirmation": confirmation }),
        ))
    }

    pub(in crate::workspace) fn undo_tag_confirmations(
        &mut self,
        items: &[TagConfirmation],
        cx: &mut Context<Self>,
    ) {
        let mut state = (self, cx);
        for item in items {
            item.undo(
                &mut state,
                |(workspace, _)| &workspace.tags.snapshot,
                |(workspace, cx), change| {
                    workspace.change_tags(change, cx);
                },
            );
        }
    }

    fn run_tag_query(
        &mut self,
        request: &WorkspaceToolRequest,
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
                app.ai.read(cx).respond_workspace_tool(&request, result);
            });
        })
        .detach();
    }
}
