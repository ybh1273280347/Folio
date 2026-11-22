//! Workspace-side tool handlers resolve the active context and route results back to AI.

mod annotations;
mod compile;
mod drawio;
mod rag;
mod tags;

use super::Workspace;
use crate::ai::{WorkspaceToolAction, WorkspaceToolRequest};
use gpui_kit::{Context, Window};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, atomic::AtomicBool},
};

/// Tracks in-flight work until a handler publishes its result or receives cancellation.
#[derive(Default)]
pub(super) struct ToolRequests {
    annotation_requests: BTreeSet<String>,
    compile_requests: HashMap<String, Arc<AtomicBool>>,
    drawio_requests: HashMap<String, WorkspaceToolRequest>,
    rag_requests: HashMap<String, Arc<AtomicBool>>,
    tag_proposals: tags::TagProposals,
}

impl ToolRequests {
    pub(super) fn clear_tag_proposals(&mut self) {
        self.tag_proposals.clear();
    }
}

impl Workspace {
    pub(super) fn dispatch_workspace_tool(
        &mut self,
        request: &WorkspaceToolRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match request.channel.as_str() {
            "annotation" => self.annotation_request(request, window, cx),
            "compile" => self.compile_request(request, window, cx),
            "drawio" => self.drawio_request(request, cx),
            "rag" => self.rag_request(request, cx),
            "tag" => self.tag_request(request, cx),
            _ if request.action == WorkspaceToolAction::Request => {
                self.ai.read(cx).respond_workspace_tool(
                    request,
                    serde_json::json!({
                        "status": "error",
                        "error": {
                            "code": "unknown_workspace_tool",
                            "message": "Folio 未注册此 workspace tool 通道。",
                            "retryable": false
                        }
                    }),
                );
            }
            _ => {}
        }
    }
}
