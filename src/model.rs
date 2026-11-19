use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
        id: "open_file",
        category: "工作区与全局",
        label: "打开本地文件",
        default_key: "ctrl-shift-o",
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
        id: "toggle_ai",
        category: "工作区与全局",
        label: "打开 / 收起 AI 助手",
        default_key: "ctrl-shift-a",
    },
    ShortcutDef {
        id: "toggle_terminal",
        category: "工作区与全局",
        label: "打开 / 收起终端",
        default_key: "ctrl-`",
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
    ShortcutDef {
        id: "md_find",
        category: "Markdown 编辑",
        label: "文档内查找",
        default_key: "ctrl-f",
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
    #[serde(default)]
    pub active_workspace: Option<WorkspaceLocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WorkspaceLocation {
    Default,
    Project(String),
    LooseFiles,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSession {
    pub tabs: Vec<String>,
    pub active: String,
    pub reading_panes: Vec<String>,
    #[serde(default)]
    pub collapsed_folders: Option<BTreeSet<String>>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active_id: None,
            active_workspace: None,
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

/// Persisted terminal launch and display preferences; live sessions stay runtime-only.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalSettings {
    /// Empty selects the automatic PowerShell shell.
    pub shell: String,
    /// Process arguments, passed individually without shell-style string splitting.
    pub args: Vec<String>,
    /// Font family used to render terminal cells.
    pub font_family: String,
    /// Terminal font size in pixels.
    pub font_size: u8,
    /// Preferred terminal panel width in pixels.
    pub panel_width: f32,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell: String::new(),
            args: vec!["-NoLogo".into()],
            font_family: "Consolas".into(),
            font_size: 14,
            panel_width: 520.,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InternalLinkOpenTarget {
    #[default]
    MainEditor,
    SplitPane,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolExecutionView {
    Minimal,
    #[default]
    Standard,
    Detailed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct TranslateSettings {
    pub baidu_app_id: String,
    pub baidu_api_key: String,
}

impl Default for TranslateSettings {
    fn default() -> Self {
        Self {
            baidu_app_id: "20261115002718243".into(),
            baidu_api_key: "7hcEk72SGfFqfKQBXQgg".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    pub base_url: String,
    #[serde(default = "default_title_model")]
    pub title_model_id: String,
    #[serde(default)]
    pub exa_api_key: String,
    #[serde(default)]
    pub baidu_qianfan_api_key: String,
}

fn default_title_model() -> String { "workbuddy-ai/hy4flash-free".into() }

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:3425".into(),
            title_model_id: default_title_model(),
            exa_api_key: String::new(),
            baidu_qianfan_api_key: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    /// Themes store colors only; typography and layout remain independent settings.
    pub theme: crate::theme::ThemeSettings,
    pub interface_font: String,
    pub editor_font: String,
    /// Empty lets the platform choose fallback glyphs for ordinary text.
    pub cjk_fallback_font: String,
    /// Empty selects the renderer's default monospace family.
    pub inline_code_font: String,
    /// Empty keeps the renderer's default monospace family for fenced code blocks.
    pub block_code_font: String,
    /// Inline code size relative to the body size of the current surface.
    pub inline_code_scale: f32,
    /// Fenced code size relative to the body size of the current surface.
    pub block_code_scale: f32,
    pub interface_font_size: u8,
    pub editor_font_size: u8,
    pub markdown_font_size: u8,
    pub math_scale: f32,
    pub inline_math_scale: f32,
    pub math_font: MathFont,
    pub line_width_limited: bool,
    pub internal_link_open_target: InternalLinkOpenTarget,
    pub tool_execution_view: ToolExecutionView,
    /// Shared by TeX and Markdown source editors; rendered Markdown has no source-line gutter.
    pub show_line_numbers: bool,
    pub editor_ratio: f32,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub log_height: f32,
    /// User-configurable terminal launch and display settings.
    pub terminal: TerminalSettings,
    pub translate: TranslateSettings,
    pub ai: AiSettings,
    pub keybindings: BTreeMap<String, String>,
}

impl AppSettings {
    pub fn interface_body_size(&self) -> f32 {
        (self.interface_font_size as f32).clamp(11., 24.)
    }

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
            theme: crate::theme::ThemeSettings::default(),
            interface_font: String::new(),
            editor_font: String::new(),
            cjk_fallback_font: String::new(),
            inline_code_font: String::new(),
            block_code_font: String::new(),
            inline_code_scale: 0.875,
            block_code_scale: 13. / 14.,
            interface_font_size: 14,
            editor_font_size: 14,
            markdown_font_size: 14,
            math_scale: 1.7,
            inline_math_scale: 1.,
            math_font: MathFont::StixTwo,
            line_width_limited: true,
            internal_link_open_target: InternalLinkOpenTarget::MainEditor,
            tool_execution_view: ToolExecutionView::default(),
            show_line_numbers: false,
            editor_ratio: 0.5,
            sidebar_open: true,
            sidebar_width: 232.,
            log_height: 220.,
            terminal: TerminalSettings::default(),
            translate: TranslateSettings::default(),
            ai: AiSettings::default(),
            keybindings: BTreeMap::new(),
        }
    }
}
