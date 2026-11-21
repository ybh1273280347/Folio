//! Shared completion contract for TeX compilation and PDF export.

use std::{path::PathBuf, time::Duration};

pub(crate) mod compiler;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuildStatus {
    Idle,
    Success,
    Failed,
    Cancelled,
}

pub(crate) struct BuildResult {
    pub status: BuildStatus,
    pub elapsed: Duration,
    pub log: String,
    pub preview: Option<PathBuf>,
}
