//! Knowledge reference anchors linking canvas nodes back to Folio notes, PDFs, and KB hits.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeAnchor {
    MarkdownFile {
        path: String,
    },
    MarkdownHeading {
        path: String,
        heading: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        heading_path: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_hint: Option<usize>,
    },
    MarkdownQuote {
        path: String,
        quote: String,
        #[serde(default)]
        occurrence: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        heading_path: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_hint: Option<usize>,
    },
    PdfPage {
        path: String,
        /// 1-based page number matching Folio's user-facing PDF jump API.
        page: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
    },
    PdfAnnotation {
        path: String,
        annotation_id: u64,
        /// 1-based page number.
        page: usize,
        #[serde(default)]
        quote: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rects: Vec<NormalizedRect>,
    },
    KbChunk {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        docid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_start: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snippet: Option<String>,
    },
}

impl KnowledgeAnchor {
    pub fn target_path(&self) -> &str {
        match self {
            Self::MarkdownFile { path }
            | Self::MarkdownHeading { path, .. }
            | Self::MarkdownQuote { path, .. }
            | Self::PdfPage { path, .. }
            | Self::PdfAnnotation { path, .. }
            | Self::KbChunk { path, .. } => path,
        }
    }

    pub fn set_target_path(&mut self, new_path: String) {
        match self {
            Self::MarkdownFile { path }
            | Self::MarkdownHeading { path, .. }
            | Self::MarkdownQuote { path, .. }
            | Self::PdfPage { path, .. }
            | Self::PdfAnnotation { path, .. }
            | Self::KbChunk { path, .. } => *path = new_path,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::MarkdownFile { .. } => "md",
            Self::MarkdownHeading { .. } => "heading",
            Self::MarkdownQuote { .. } => "quote",
            Self::PdfPage { .. } => "pdf",
            Self::PdfAnnotation { .. } => "pdf-ann",
            Self::KbChunk { .. } => "kb",
        }
    }

    /// Standard JSON Canvas `subpath` representation for cross-tool interoperability.
    pub fn to_json_canvas_subpath(&self) -> Option<String> {
        match self {
            Self::MarkdownHeading { heading, .. } if !heading.trim().is_empty() => {
                Some(format!("#{}", heading.trim()))
            }
            Self::MarkdownQuote { heading_path, .. } => {
                heading_path.last().filter(|h| !h.trim().is_empty()).map(|h| format!("#{}", h.trim()))
            }
            Self::PdfPage { page, .. } | Self::PdfAnnotation { page, .. } => {
                Some(format!("#page={page}"))
            }
            Self::KbChunk {
                page: Some(page), ..
            } => Some(format!("#page={page}")),
            Self::KbChunk {
                heading: Some(heading),
                ..
            } if !heading.trim().is_empty() => Some(format!("#{}", heading.trim())),
            _ => None,
        }
    }

    /// Remaps the target path when a file or folder is renamed/moved inside the workspace.
    pub fn relocate_paths(&mut self, moves: &[(String, String)], is_folder: bool) -> bool {
        let Some(next) = remap_rel_path(self.target_path(), moves, is_folder) else {
            return false;
        };
        self.set_target_path(next);
        true
    }
}

pub fn remap_rel_path(
    path: &str,
    moves: &[(String, String)],
    is_folder: bool,
) -> Option<String> {
    let normalized = path.replace('\\', "/");
    for (old, new) in moves {
        let old_norm = old.replace('\\', "/");
        let new_norm = new.replace('\\', "/");
        if normalized == old_norm {
            return Some(new_norm);
        }
        if is_folder {
            let prefix = format!("{old_norm}/");
            if let Some(rest) = normalized.strip_prefix(&prefix) {
                return Some(format!("{new_norm}/{rest}"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_relocates_file_and_folder_paths() {
        let mut anchor = KnowledgeAnchor::MarkdownQuote {
            path: "notes/deep/intro.md".into(),
            quote: "hello".into(),
            occurrence: 0,
            heading_path: vec!["Intro".into()],
            line_hint: Some(3),
        };
        assert!(!anchor.relocate_paths(&[("notes/other.md".into(), "notes/x.md".into())], false));
        assert!(anchor.relocate_paths(&[("notes/deep".into(), "archive".into())], true));
        assert_eq!(anchor.target_path(), "archive/intro.md");
    }
}
