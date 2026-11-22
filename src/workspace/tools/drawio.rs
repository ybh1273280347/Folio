//! Routes Draw.io tool requests to project files or the active canvas.

use crate::{
    ai::WorkspaceToolRequest,
    drawio::file::{Failure, Operation, Request, execute},
    files,
};
use gpui_kit::Context;
use serde_json::Value;
use std::path::Path;

use super::super::Workspace;

impl Workspace {
    pub(in crate::workspace) fn drawio_request(
        &mut self,
        request: &WorkspaceToolRequest,
        cx: &mut Context<Self>,
    ) {
        if request.action == crate::ai::WorkspaceToolAction::Cancel {
            self.drawio_requests.remove(&request.id);
            return;
        }
        let operation: Request = match serde_json::from_value(request.arguments.clone()) {
            Ok(operation) => operation,
            Err(error) => {
                self.ai.read(cx).respond_workspace_tool(
                    request,
                    Failure::new("invalid_request", error.to_string()).result(),
                );
                return;
            }
        };
        let Some(project) = self.directory_workspace() else {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("project_not_active", "Draw.io 工具需要当前项目或默认空间。").result(),
            );
            return;
        };
        let root = match dunce::canonicalize(&project.folder) {
            Ok(root) => root,
            Err(error) => {
                self.ai.read(cx).respond_workspace_tool(
                    request,
                    Failure::new("project_unavailable", error.to_string()).result(),
                );
                return;
            }
        };
        if request.project_key != crate::ai::project_key(&root) {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("project_not_active", "AI 会话所属项目已切换，请在当前项目重试。").result(),
            );
            return;
        }
        match operation.operation {
            Operation::GetSelection | Operation::EditCells => {
                self.drawio_canvas_request(request, operation, cx);
            }
            Operation::Read | Operation::Create | Operation::Update => {
                let result = execute(&root, operation).unwrap_or_else(Failure::result);
                self.ai.read(cx).respond_workspace_tool(request, result);
            }
        }
    }

    fn drawio_canvas_request(
        &mut self,
        request: &WorkspaceToolRequest,
        operation: Request,
        cx: &mut Context<Self>,
    ) {
        let path = self.documents.active();
        if path.is_empty() || !files::is_drawio(Path::new(path)) {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("drawio_not_active", "请先打开要操作的 Draw.io 图表并切换到该页签。").result(),
            );
            return;
        }
        let Some(pane) = self.documents.active_drawio().cloned() else {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("drawio_not_ready", "当前 Draw.io 画布尚未就绪，请稍后重试。").result(),
            );
            return;
        };
        let operation_name = match operation.operation {
            Operation::GetSelection => "get_selection",
            Operation::EditCells => "edit_cells",
            _ => unreachable!(),
        };
        let params = serde_json::to_value(operation.params).expect("Draw.io parameters serialize");
        self.drawio_requests.insert(request.id.clone(), request.clone());
        let dispatch = pane.update(cx, |pane, cx| {
            pane.request_tool(&request.id, operation_name, params, cx)
        });
        if let Err(code) = dispatch {
            self.drawio_requests.remove(&request.id);
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new(code, match code {
                    "drawio_bridge_unavailable" => "Folio Draw.io 工具桥尚未就绪，请稍后重试。",
                    "drawio_dispatch_failed" => "Folio 无法向当前 Draw.io 画布派发工具请求。",
                    _ => "当前 Draw.io 画布尚未就绪，请稍后重试。",
                }).result(),
            );
        }
    }

    pub(in crate::workspace) fn drawio_tool_result(
        &mut self,
        request_id: &str,
        result: Value,
        cx: &mut Context<Self>,
    ) {
        if let Some(request) = self.drawio_requests.remove(request_id) {
            self.ai.read(cx).respond_workspace_tool(&request, result);
        }
    }
}
