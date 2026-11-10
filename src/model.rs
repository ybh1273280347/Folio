use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Xelatex,
    Pdflatex,
    Lualatex,
}

impl Default for Engine {
    fn default() -> Self {
        Self::Xelatex
    }
}

impl Engine {
    pub fn latexmk_flag(self) -> &'static str {
        match self {
            Self::Xelatex => "-xelatex",
            Self::Pdflatex => "-pdf",
            Self::Lualatex => "-lualatex",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub main_file: String,
    pub engine: Engine,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub projects: Vec<Project>,
    pub active_id: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Asset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    pub path: String,
    pub kind: FileKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskDocument {
    pub text: String,
    pub revision: String,
}

#[derive(Clone, Debug)]
pub struct DocumentBuffer {
    pub text: String,
    pub saved_text: String,
    pub revision: String,
    pub conflict: Option<DiskDocument>,
    pub deleted: bool,
    pub dirty: bool,
}

impl DocumentBuffer {
    pub fn from_disk(document: DiskDocument) -> Self {
        Self {
            text: document.text.clone(),
            saved_text: document.text,
            revision: document.revision,
            conflict: None,
            deleted: false,
            dirty: false,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_text(&mut self, text: String) {
        self.dirty = text != self.saved_text;
        self.text = text;
    }

    pub fn replace_from_disk(&mut self, document: DiskDocument) {
        self.text = document.text.clone();
        self.saved_text = document.text;
        self.revision = document.revision;
        self.conflict = None;
        self.deleted = false;
        self.dirty = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildStatus {
    Idle,
    Success,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub interface_font: String,
    pub editor_font: String,
    pub interface_font_size: u8,
    pub editor_font_size: u8,
    pub markdown_font_size: u8,
    pub line_width_limited: bool,
    pub editor_ratio: f32,
    pub sidebar_open: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            interface_font: String::new(),
            editor_font: String::new(),
            interface_font_size: 14,
            editor_font_size: 14,
            markdown_font_size: 14,
            line_width_limited: true,
            editor_ratio: 0.5,
            sidebar_open: true,
        }
    }
}
