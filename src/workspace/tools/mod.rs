//! Workspace-side tool handlers resolve the active context and route results back to AI.

mod compile;
mod drawio;
mod tags;

pub(super) use compile::CompileRequests;
pub(super) use tags::TagProposals;
