//! Workspace-side tool handlers resolve the active context and route results back to AI.

mod compile;
mod drawio;

pub(super) use compile::CompileRequests;
