//! Binds agent proposals to the initiating project and conversation. Approval
//! arrives through the host's existing per-conversation confirmation card.

use super::*;
use crate::tags::{
    Store,
    agent::{Edit, Failure, Proposal, Request, ToolResult},
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
                "标签工具可用于项目和默认空间。",
            )
        })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
        if request["projectKey"].as_str() != Some(crate::ai::project_key(&root).as_str()) {
            return Err(Failure::new(
                "project_not_active",
                "请切回发起会话所属的项目后重试。",
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
                        Failure::new("stale_preview", "这批标签变更已失效，请重新预览。")
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
