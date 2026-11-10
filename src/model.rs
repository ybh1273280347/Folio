use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub struct ShortcutDef {
    pub id: &'static str,
    pub category: &'static str,
    pub label: &'static str,
    pub default_key: &'static str,
}

pub const SHORTCUT_DEFS: &[ShortcutDef] = &[
    ShortcutDef {
        id: "save",
        category: "工作区与全局",
        label: "保存当前文件",
        default_key: "ctrl-s",
    },
    ShortcutDef {
        id: "save_all",
        category: "工作区与全局",
        label: "保存全部文件",
        default_key: "ctrl-alt-s",
    },
    ShortcutDef {
        id: "open_project",
        category: "工作区与全局",
        label: "打开本地项目",
        default_key: "ctrl-o",
    },
    ShortcutDef {
        id: "compile",
        category: "工作区与全局",
        label: "编译文档",
        default_key: "ctrl-enter",
    },
    ShortcutDef {
        id: "toggle_sidebar",
        category: "工作区与全局",
        label: "显示 / 隐藏左侧栏",
        default_key: "ctrl-\\",
    },
    ShortcutDef {
        id: "find_file",
        category: "工作区与全局",
        label: "查找项目文件",
        default_key: "ctrl-p",
    },
    ShortcutDef {
        id: "close_tab",
        category: "工作区与全局",
        label: "关闭当前标签页",
        default_key: "ctrl-w",
    },
    ShortcutDef {
        id: "toggle_source",
        category: "Markdown 编辑",
        label: "切换源码 / 所见即所得",
        default_key: "ctrl-/",
    },
    ShortcutDef {
        id: "md_bold",
        category: "Markdown 编辑",
        label: "加粗",
        default_key: "ctrl-b",
    },
    ShortcutDef {
        id: "md_italic",
        category: "Markdown 编辑",
        label: "斜体",
        default_key: "ctrl-i",
    },
    ShortcutDef {
        id: "md_underline",
        category: "Markdown 编辑",
        label: "下划线",
        default_key: "ctrl-u",
    },
    ShortcutDef {
        id: "md_strikethrough",
        category: "Markdown 编辑",
        label: "删除线",
        default_key: "ctrl-shift-s",
    },
    ShortcutDef {
        id: "md_code",
        category: "Markdown 编辑",
        label: "行内代码",
        default_key: "ctrl-e",
    },
    ShortcutDef {
        id: "md_highlight",
        category: "Markdown 编辑",
        label: "荧光笔高亮",
        default_key: "ctrl-shift-h",
    },
    ShortcutDef {
        id: "md_link",
        category: "Markdown 编辑",
        label: "插入 / 编辑链接",
        default_key: "ctrl-k",
    },
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationItem {
    pub id: u64,
    pub file: String,
    pub page: usize,
    pub quote: String,
    pub occurrence: usize,
    pub color: String,
    pub comment: String,
    #[serde(default)]
    pub region: Option<AnnotationRect>,
    #[serde(default)]
    pub rects: Vec<AnnotationRect>,
}

/// 5 fluorescent highlight colors (name, hex string for Markdown `<mark>`, RGB hex for UI).
pub const HIGHLIGHT_COLORS: &[(&str, &str, u32)] = &[
    ("yellow", "#fef08a", 0xfacc15),
    ("green", "#bbf7d0", 0x4ade80),
    ("blue", "#bae6fd", 0x38bdf8),
    ("pink", "#fbcfe8", 0xf472b6),
    ("orange", "#fed7aa", 0xfb923c),
];

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

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MathFont {
    #[default]
    StixTwo,
    LatinModern,
    TexGyrePagella,
}
impl MathFont {
    pub const ALL: [Self; 3] = [Self::StixTwo, Self::LatinModern, Self::TexGyrePagella];
    pub fn family(self) -> &'static str {
        match self {
            Self::StixTwo => "STIX Two Math",
            Self::LatinModern => "Latin Modern Math",
            Self::TexGyrePagella => "TeX Gyre Pagella Math",
        }
    }
    pub fn filename(self) -> &'static str {
        match self {
            Self::StixTwo => "STIXTwoMath-Regular.otf",
            Self::LatinModern => "latinmodern-math.otf",
            Self::TexGyrePagella => "texgyrepagella-math.otf",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub interface_font: String,
    pub editor_font: String,
    pub interface_font_size: u8,
    pub editor_font_size: u8,
    pub markdown_font_size: u8,
    pub math_scale: f32,
    pub inline_math_scale: f32,
    pub math_font: MathFont,
    pub line_width_limited: bool,
    pub editor_ratio: f32,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub log_height: f32,
    pub keybindings: BTreeMap<String, String>,
}

impl AppSettings {
    pub fn key_for(&self, id: &str) -> String {
        if let Some(custom) = self.keybindings.get(id).filter(|k| !k.trim().is_empty()) {
            return custom.clone();
        }
        SHORTCUT_DEFS
            .iter()
            .find(|def| def.id == id)
            .map(|def| def.default_key.to_owned())
            .unwrap_or_default()
    }

    pub fn format_key_badge(raw: &str) -> String {
        raw.split('-')
            .filter(|part| !part.is_empty())
            .map(|part| match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => "Ctrl".to_owned(),
                "alt" => "Alt".to_owned(),
                "shift" => "Shift".to_owned(),
                "cmd" | "super" | "win" => "Win".to_owned(),
                "enter" => "Enter".to_owned(),
                "escape" | "esc" => "Esc".to_owned(),
                "backspace" => "Backspace".to_owned(),
                "delete" => "Del".to_owned(),
                "space" => "Space".to_owned(),
                "\\" => "\\".to_owned(),
                "/" => "/".to_owned(),
                other => other.to_ascii_uppercase(),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            interface_font: String::new(),
            editor_font: String::new(),
            interface_font_size: 14,
            editor_font_size: 14,
            markdown_font_size: 14,
            math_scale: 1.7,
            inline_math_scale: 1.,
            math_font: MathFont::StixTwo,
            line_width_limited: true,
            editor_ratio: 0.5,
            sidebar_open: true,
            sidebar_width: 232.,
            log_height: 220.,
            keybindings: BTreeMap::new(),
        }
    }
}
