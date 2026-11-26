use crate::i18n::{BiText, Locale, preferences as pref_i18n};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct ShortcutDef {
    pub id: &'static str,
    pub category: BiText,
    pub label: BiText,
    pub default_key: &'static str,
}

pub const SHORTCUT_DEFS: &[ShortcutDef] = &[
    ShortcutDef {
        id: "save",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_SAVE,
        default_key: "ctrl-s",
    },
    ShortcutDef {
        id: "save_all",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_SAVE_ALL,
        default_key: "ctrl-alt-s",
    },
    ShortcutDef {
        id: "open_project",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_OPEN_PROJECT,
        default_key: "ctrl-o",
    },
    ShortcutDef {
        id: "open_file",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_OPEN_FILE,
        default_key: "ctrl-shift-o",
    },
    ShortcutDef {
        id: "compile",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_COMPILE,
        default_key: "ctrl-enter",
    },
    ShortcutDef {
        id: "toggle_sidebar",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_TOGGLE_SIDEBAR,
        default_key: "ctrl-\\",
    },
    ShortcutDef {
        id: "find_file",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_FIND_FILE,
        default_key: "ctrl-p",
    },
    ShortcutDef {
        id: "toggle_ai",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_TOGGLE_AI,
        default_key: "ctrl-shift-a",
    },
    ShortcutDef {
        id: "toggle_terminal",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_TOGGLE_TERMINAL,
        default_key: "ctrl-`",
    },
    ShortcutDef {
        id: "toggle_zen",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_TOGGLE_ZEN,
        default_key: "f11",
    },
    ShortcutDef {
        id: "close_tab",
        category: pref_i18n::CAT_WORKSPACE,
        label: pref_i18n::SC_CLOSE_TAB,
        default_key: "ctrl-w",
    },
    ShortcutDef {
        id: "toggle_source",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_TOGGLE_SOURCE,
        default_key: "ctrl-/",
    },
    ShortcutDef {
        id: "md_bold",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_BOLD,
        default_key: "ctrl-b",
    },
    ShortcutDef {
        id: "md_italic",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_ITALIC,
        default_key: "ctrl-i",
    },
    ShortcutDef {
        id: "md_underline",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_UNDERLINE,
        default_key: "ctrl-u",
    },
    ShortcutDef {
        id: "md_strikethrough",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_STRIKETHROUGH,
        default_key: "ctrl-shift-s",
    },
    ShortcutDef {
        id: "md_code",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_CODE,
        default_key: "ctrl-e",
    },
    ShortcutDef {
        id: "md_highlight",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_HIGHLIGHT,
        default_key: "ctrl-shift-h",
    },
    ShortcutDef {
        id: "md_link",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_LINK,
        default_key: "ctrl-k",
    },
    ShortcutDef {
        id: "md_find",
        category: pref_i18n::CAT_MARKDOWN,
        label: pref_i18n::SC_MD_FIND,
        default_key: "ctrl-f",
    },
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
    #[serde(default)]
    pub recent_project_ids: Vec<String>,
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
            recent_project_ids: Vec::new(),
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

/// Characters that remain once whitespace is removed.
///
/// Counts Unicode scalar values, so ordinary spaces, tabs, line breaks, the
/// ideographic space and other separators are all excluded. Syntax characters
/// stay in the count because the visible source is the document.
pub fn nonspace_char_count(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

#[derive(Clone, Debug)]
pub struct DocumentBuffer {
    pub text: String,
    pub saved_text: String,
    pub revision: String,
    pub conflict: Option<DiskDocument>,
    pub deleted: bool,
    pub dirty: bool,
    /// Recomputed on every buffer write so editors can render it per frame.
    pub char_count: usize,
}

impl DocumentBuffer {
    pub fn from_disk(document: DiskDocument) -> Self {
        Self {
            char_count: nonspace_char_count(&document.text),
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
        self.char_count = nonspace_char_count(&text);
        self.text = text;
    }

    pub fn replace_from_disk(&mut self, document: DiskDocument) {
        self.char_count = nonspace_char_count(&document.text);
        self.text = document.text.clone();
        self.saved_text = document.text;
        self.revision = document.revision;
        self.conflict = None;
        self.deleted = false;
        self.dirty = false;
    }
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
            panel_width: 420.,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RagConfig {
    pub embed_url: String,
    pub embed_model: String,
    pub embed_dim: usize,
    pub rerank_url: String,
    pub rerank_model: String,
    pub server_bin: String,
    pub embed_model_path: String,
    pub rerank_model_path: String,
    pub auto_manage_server: bool,
    pub idle_release_secs: u64,
    pub paddleocr_token: String,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            embed_url: "http://127.0.0.1:18101".to_owned(),
            embed_model: "Qwen3-Embedding-0.6B".to_owned(),
            embed_dim: 1024,
            rerank_url: "http://127.0.0.1:18102".to_owned(),
            rerank_model: "Qwen3-Reranker-0.6B".to_owned(),
            server_bin: String::new(),
            embed_model_path: String::new(),
            rerank_model_path: String::new(),
            auto_manage_server: true,
            idle_release_secs: 300,
            paddleocr_token: String::new(),
        }
    }
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

pub const MAX_TEXT_COLOR_PRESETS: usize = 10;

pub const DEFAULT_TEXT_COLOR_PRESETS: &[&str] = &[
    "#dc2626", // Red
    "#ea580c", // Orange
    "#16a34a", // Green
    "#0d9488", // Teal
    "#2563eb", // Blue
    "#7c3aed", // Purple
];

pub fn default_text_color_presets() -> Vec<String> {
    DEFAULT_TEXT_COLOR_PRESETS
        .iter()
        .map(|hex| (*hex).to_owned())
        .collect()
}

pub fn normalize_hex_color(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix('#').unwrap_or_else(|| raw.trim());
    let expanded = match hex.len() {
        3 => {
            let mut chars = hex.chars();
            let (r, g, b) = (chars.next()?, chars.next()?, chars.next()?);
            if !r.is_ascii_hexdigit() || !g.is_ascii_hexdigit() || !b.is_ascii_hexdigit() {
                return None;
            }
            format!("{r}{r}{g}{g}{b}{b}")
        }
        6 => {
            if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            hex.to_owned()
        }
        _ => return None,
    };
    Some(format!("#{}", expanded.to_ascii_lowercase()))
}

pub fn parse_rgb_hex(raw: &str) -> Option<u32> {
    let normalized = normalize_hex_color(raw)?;
    u32::from_str_radix(&normalized[1..], 16).ok()
}

pub fn sanitize_text_color_presets(list: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for item in list {
        if let Some(normalized) = normalize_hex_color(&item)
            && !out.contains(&normalized)
        {
            out.push(normalized);
            if out.len() >= MAX_TEXT_COLOR_PRESETS {
                break;
            }
        }
    }
    out
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    /// Interface language (Simplified Chinese or English).
    pub locale: Locale,
    /// Themes store colors only; typography and layout remain independent settings.
    pub theme: crate::theme::ThemeSettings,
    /// A local image path enables the workspace backdrop.
    pub background_image: Option<PathBuf>,
    /// Opacity of the image layer, as a percentage from 0 to 100.
    pub background_image_opacity: u8,
    pub interface_font: String,
    pub editor_font: String,
    pub markdown_font: String,
    pub ai_font: String,
    /// Shared glyph fallback. Empty lets the platform choose fallback faces.
    pub cjk_fallback_font: String,
    /// Scale factor applied to CJK characters relative to Latin text.
    pub cjk_font_scale: f32,
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
    pub ai_font_size: u8,
    pub math_scale: f32,
    pub inline_math_scale: f32,
    pub math_font: MathFont,
    pub line_width_limited: bool,
    pub internal_link_open_target: InternalLinkOpenTarget,
    pub tool_execution_view: ToolExecutionView,
    /// Shared by TeX and Markdown source editors; rendered Markdown has no source-line gutter.
    pub show_line_numbers: bool,
    #[serde(default = "default_smooth_caret")]
    pub smooth_caret: bool,
    pub editor_ratio: f32,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub log_width: f32,
    /// User-configurable terminal launch and display settings.
    pub terminal: TerminalSettings,
    pub translate: TranslateSettings,
    pub ai: AiSettings,
    pub rag: RagConfig,
    pub keybindings: BTreeMap<String, String>,
    /// User-editable foreground text color presets for the Markdown floating toolbar.
    #[serde(default = "default_text_color_presets")]
    pub text_color_presets: Vec<String>,
}

fn default_smooth_caret() -> bool {
    true
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
            locale: Locale::default(),
            theme: crate::theme::ThemeSettings::default(),
            background_image: None,
            background_image_opacity: 35,
            interface_font: String::new(),
            editor_font: String::new(),
            markdown_font: String::new(),
            ai_font: String::new(),
            cjk_fallback_font: String::new(),
            cjk_font_scale: 1.0,
            inline_code_font: String::new(),
            block_code_font: String::new(),
            inline_code_scale: 0.875,
            block_code_scale: 13. / 14.,
            interface_font_size: 14,
            editor_font_size: 14,
            markdown_font_size: 14,
            ai_font_size: 14,
            math_scale: 1.7,
            inline_math_scale: 1.,
            math_font: MathFont::StixTwo,
            line_width_limited: true,
            internal_link_open_target: InternalLinkOpenTarget::MainEditor,
            tool_execution_view: ToolExecutionView::default(),
            show_line_numbers: false,
            smooth_caret: default_smooth_caret(),
            editor_ratio: 0.5,
            sidebar_open: true,
            sidebar_width: 248.,
            log_width: 360.,
            terminal: TerminalSettings::default(),
            translate: TranslateSettings::default(),
            ai: AiSettings::default(),
            rag: RagConfig::default(),
            keybindings: BTreeMap::new(),
            text_color_presets: default_text_color_presets(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_color_normalization_and_parsing() {
        assert_eq!(normalize_hex_color("DC2626").as_deref(), Some("#dc2626"));
        assert_eq!(normalize_hex_color(" #f0a ").as_deref(), Some("#ff00aa"));
        assert_eq!(normalize_hex_color("#12345"), None);
        assert_eq!(normalize_hex_color("#zzzzzz"), None);
        assert_eq!(parse_rgb_hex("#2563EB"), Some(0x2563eb));
    }

    #[test]
    fn preset_sanitization_dedups_and_enforces_cap() {
        let input: Vec<String> = (0..15)
            .map(|i| format!("#{i:02x}1122"))
            .chain(["invalid".to_owned(), "#001122".to_owned()])
            .collect();
        let sanitized = sanitize_text_color_presets(input);
        assert_eq!(sanitized.len(), MAX_TEXT_COLOR_PRESETS);
        assert_eq!(sanitized[0], "#001122");
        assert_eq!(sanitized[MAX_TEXT_COLOR_PRESETS - 1], "#091122");
    }

    #[test]
    fn missing_presets_in_json_use_defaults() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.text_color_presets, default_text_color_presets());
    }
}

