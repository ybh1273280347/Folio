//! Color-only themes. Persisted palettes own the colors; the UI-thread snapshot
//! serves shared controls and PDF style callbacks that do not receive an App.
use std::{cell::Cell, collections::BTreeMap};

use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Palette {
    pub background: u32,
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub hover: u32,
    pub pressed: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub code_background: u32,
    pub selection: u32,
    pub danger: u32,
}

impl Default for Palette {
    fn default() -> Self {
        Self::light()
    }
}

impl Palette {
    /// Previous built-in light palette, kept for one-time migration of unmodified settings.
    pub const LEGACY_LIGHT: Self = Self {
        background: 0xf7f6f5,
        surface: 0xffffff,
        text: 0x37352f,
        muted: 0x787774,
        border: 0xe8e7e3,
        hover: 0xefefed,
        pressed: 0xe4e3df,
        accent: 0x52769a,
        on_accent: 0xffffff,
        code_background: 0xf5f5f5,
        selection: 0xe8e7e4,
        danger: 0xd4373f,
    };

    /// Previous built-in dark palette (warm charcoal), kept for one-time migration.
    pub const LEGACY_DARK: Self = Self {
        background: 0x141413,
        surface: 0x20201e,
        text: 0xfaf9f5,
        muted: 0xb0aea5,
        border: 0x3d3c38,
        hover: 0x2d2c29,
        pressed: 0x383732,
        accent: 0xd97757,
        on_accent: 0x141413,
        code_background: 0x191918,
        selection: 0x44352e,
        danger: 0xf08c86,
    };

    /// Claude Classic Light: crisp paper white, whisper-light neutral chrome and terracotta accent.
    pub const fn light() -> Self {
        Self {
            background: 0xfafafa,
            surface: 0xffffff,
            text: 0x161616,
            muted: 0x70696b,
            border: 0xe9e9e9,
            hover: 0xf2f2f1,
            pressed: 0xe6e6e5,
            accent: 0xd97757,
            on_accent: 0xffffff,
            code_background: 0xf7f7f7,
            selection: 0xebebea,
            danger: 0xd4373f,
        }
    }

    /// Obsidian Blue-Dark: unified blue-black chrome, subtle elevated paper, cool-white ink and cobalt accent.
    pub const fn dark() -> Self {
        Self {
            background: 0x13161d,
            surface: 0x161922,
            text: 0xf4f9ff,
            muted: 0x88929f,
            border: 0x23262e,
            hover: 0x22252d,
            pressed: 0x2c313d,
            accent: 0x4d6bfe,
            on_accent: 0xf2f7ff,
            code_background: 0x1b1e26,
            selection: 0x262b36,
            danger: 0xef8585,
        }
    }

    pub fn is_dark(self) -> bool {
        let c = rgb(self.background);
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b < 0.5
    }

    pub fn get(self, role: ColorRole) -> u32 {
        *self.field(role)
    }

    pub fn set(&mut self, role: ColorRole, color: u32) {
        *self.field_mut(role) = color;
    }

    fn field(&self, role: ColorRole) -> &u32 {
        match role {
            ColorRole::Background => &self.background,
            ColorRole::Surface => &self.surface,
            ColorRole::Text => &self.text,
            ColorRole::Muted => &self.muted,
            ColorRole::Border => &self.border,
            ColorRole::Hover => &self.hover,
            ColorRole::Pressed => &self.pressed,
            ColorRole::Accent => &self.accent,
            ColorRole::OnAccent => &self.on_accent,
            ColorRole::Code => &self.code_background,
            ColorRole::Selection => &self.selection,
            ColorRole::Danger => &self.danger,
        }
    }

    fn field_mut(&mut self, role: ColorRole) -> &mut u32 {
        match role {
            ColorRole::Background => &mut self.background,
            ColorRole::Surface => &mut self.surface,
            ColorRole::Text => &mut self.text,
            ColorRole::Muted => &mut self.muted,
            ColorRole::Border => &mut self.border,
            ColorRole::Hover => &mut self.hover,
            ColorRole::Pressed => &mut self.pressed,
            ColorRole::Accent => &mut self.accent,
            ColorRole::OnAccent => &mut self.on_accent,
            ColorRole::Code => &mut self.code_background,
            ColorRole::Selection => &mut self.selection,
            ColorRole::Danger => &mut self.danger,
        }
    }

    /// Map legacy literal tints onto the active palette's semantic roles.
    fn resolve(self, original: u32) -> Rgba {
        use ColorRole::*;
        let role = match original {
            0xffffff => Surface,
            0xf7f6f5 | 0xfafaf9 | 0xfbfbfa | 0xf7f6f3 | 0xf7f6f4 | 0xf7f7f5 | 0xf4f3ef
            | 0xf1f1ef | 0xf2f3f5 | 0xf4f4f2 => Background,
            0x37352f | 0x1f2328 | 0x343a40 | 0x1e293b | 0x5d5d5a => Text,
            0x787774 | 0x7d838b | 0x57606a | 0x64748b | 0x777774 | 0x999996 | 0x9b9a97
            | 0x94a3b8 | 0x8b989f | 0x7c7c78 | 0x5b5b58 => Muted,
            0xe8e7e3 | 0xe6e5e1 | 0xdcdbd7 | 0xcfcec9 | 0xc5c4bf | 0xd3d1cb | 0xe2e1dd
            | 0xe2e1dc | 0xe2e4e8 | 0xe5e6eb | 0xf1f0eb | 0xd8d6d0 | 0xc4c7cc | 0xa8a7a2 => Border,
            0xefefed | 0xe9ebed | 0xeae9e5 => Hover,
            0xe4e3df | 0xdcdfe3 | 0xedece9 | 0xdddcda => Pressed,
            0xe8e7e4 | 0xe0ecff | 0xe8f3ff | 0xeef4fb | 0xe8eff5 | 0xedf2fa | 0xf2f7ff | 0xdde9f6
            | 0xb9d8fb | 0xd0e6ff => Selection,
            0x52769a | 0x47698c | 0x486c90 | 0x3f6082 | 0x3d5d7e | 0x355373 | 0x165dff
            | 0x0e42d2 | 0x1d4ed8 | 0x2b6cb0 | 0x2b5b94 | 0x4b7bb5 | 0x5c7f9f => Accent,
            0xd4373f | 0xbf2f37 | 0xc42f37 | 0xae2830 | 0xa9262e | 0x962128 | 0xcf1322
            | 0xa8071a | 0xd93025 | 0xb42318 | 0x8a1c13 | 0xb92d2d | 0xdf5151 | 0xa16b60 => Danger,
            // Status backgrounds need a dark tint, while stored annotation colors
            // remain document data and bypass this presentation-only resolver.
            0xfff1f0 | 0xffe4e1 | 0xffd3cd | 0xffccc7 | 0xffa8a0 | 0xff8f85 | 0xffdcd8
                if self.is_dark() =>
            {
                return mix(self.surface, self.danger, 0.16);
            }
            0xfef3c7 | 0xfef9c3 | 0xfff4e5 | 0xfffaf0 | 0xf0dec0 if self.is_dark() => {
                return rgb(0x3c3323);
            }
            0xb45309 | 0x854d0e | 0x5c4628 if self.is_dark() => return rgb(0xe7bd7c),
            0x4b7b59 if self.is_dark() => return rgb(0x63c59a),
            _ => return rgb(original),
        };
        rgb(self.get(role))
    }

    /// Slightly crisper border for cards, popovers and inputs (Claude `--card-border`).
    pub fn card_border(self) -> Rgba {
        mix(self.border, self.text, if self.is_dark() { 0.06 } else { 0.05 })
    }

    /// Elevated surface for popovers and modals (`#1b1e26` in Obsidian Dark, `#ffffff` in Claude Light).
    pub fn elevated_surface(self) -> Rgba {
        if self.is_dark() {
            rgb(self.code_background)
        } else {
            rgb(self.surface)
        }
    }

    /// Hover tint for primary accent controls.
    pub fn accent_hover(self) -> Rgba {
        if self.is_dark() {
            mix(self.accent, self.text, 0.16)
        } else {
            mix(self.accent, 0x000000, 0.10)
        }
    }

    /// Pressed tint for primary accent controls.
    pub fn accent_active(self) -> Rgba {
        if self.is_dark() {
            mix(self.accent, self.text, 0.08)
        } else {
            mix(self.accent, 0x000000, 0.18)
        }
    }

    /// Hover tint for danger controls.
    pub fn danger_hover(self) -> Rgba {
        if self.is_dark() {
            mix(self.danger, self.text, 0.14)
        } else {
            mix(self.danger, 0x000000, 0.10)
        }
    }

    /// Pressed tint for danger controls.
    pub fn danger_active(self) -> Rgba {
        if self.is_dark() {
            mix(self.danger, 0x000000, 0.12)
        } else {
            mix(self.danger, 0x000000, 0.20)
        }
    }

    /// Link ink derived from the active palette (Claude ink-blue in terracotta light, luminous cobalt in dark).
    pub fn link_color(self) -> Rgba {
        if self.is_dark() {
            mix(self.accent, self.text, 0.35)
        } else if self.accent == Self::light().accent {
            rgb(0x184f95)
        } else {
            rgb(self.accent)
        }
    }

    /// Subtle translucent scrollbar thumb matching the active mode.
    pub fn scroll_thumb(self) -> Rgba {
        let mut c = mix(self.surface, self.muted, if self.is_dark() { 0.36 } else { 0.30 });
        c.a = if self.is_dark() { 0.78 } else { 0.72 };
        c
    }

    pub fn scroll_thumb_hover(self) -> Rgba {
        let mut c = mix(self.surface, self.muted, if self.is_dark() { 0.56 } else { 0.52 });
        c.a = if self.is_dark() { 0.88 } else { 0.84 };
        c
    }

    pub fn scroll_thumb_active(self) -> Rgba {
        let mut c = mix(self.surface, self.muted, if self.is_dark() { 0.72 } else { 0.68 });
        c.a = 0.92;
        c
    }

    /// Clean, modern card surface token derived from theme `surface` (near-white in light mode).
    pub fn paper_surface(self) -> Rgba {
        rgb(self.surface)
    }

    /// Subtle header surface token, just a whisper deeper than the main card surface.
    pub fn paper_header(self) -> Rgba {
        mix(self.surface, self.background, if self.is_dark() { 0.35 } else { 0.42 })
    }

    /// Hover tint on card surfaces.
    pub fn paper_hover(self) -> Rgba {
        mix(self.surface, self.hover, 0.55)
    }

    /// Single subtle back-stacked sheet background token.
    pub fn paper_stack(self) -> Rgba {
        mix(self.surface, self.background, if self.is_dark() { 0.45 } else { 0.52 })
    }

    /// Crisp border token for cards.
    pub fn paper_border(self) -> Rgba {
        rgb(self.border)
    }

    /// Subtle interior rule / divider line between excerpts.
    pub fn paper_divider(self) -> Rgba {
        let mut c = rgb(self.border);
        c.a = if self.is_dark() { 0.62 } else { 0.68 };
        c
    }

    /// Clean light-grey metadata token for page numbers and line numbers.
    pub fn paper_folio(self) -> Rgba {
        let mut c = rgb(self.muted);
        c.a = if self.is_dark() { 0.78 } else { 0.80 };
        c
    }

    /// Clean, high-contrast search match highlight color on card surfaces.
    pub fn paper_highlight(self) -> Rgba {
        if self.is_dark() {
            mix(self.surface, 0xd97706, 0.28)
        } else {
            rgb(0xfef3c7)
        }
    }

    /// Derived outer workbench canvas behind floating Islands.
    pub fn island_canvas(self) -> Rgba {
        if self.is_dark() {
            mix(self.background, 0x000000, 0.25)
        } else {
            mix(self.background, self.border, 0.55)
        }
    }

    /// Hover tint for controls sitting directly on the outer Islands shell.
    pub fn island_shell_hover(self) -> Rgba {
        if self.is_dark() {
            mix(self.background, self.text, 0.08)
        } else {
            mix(self.border, self.text, 0.04)
        }
    }

    /// Active surface for controls sitting directly on the outer Islands shell.
    pub fn island_shell_active(self) -> Rgba {
        if self.is_dark() {
            rgb(self.selection)
        } else {
            rgb(self.surface)
        }
    }
}

#[derive(Clone, Copy)]
pub enum ColorRole {
    Background,
    Surface,
    Text,
    Muted,
    Border,
    Hover,
    Pressed,
    Accent,
    OnAccent,
    Code,
    Selection,
    Danger,
}

impl ColorRole {
    pub const ALL: [(Self, crate::i18n::BiText); 12] = [
        (Self::Background, crate::i18n::preferences::COLOR_BACKGROUND),
        (Self::Surface, crate::i18n::preferences::COLOR_SURFACE),
        (Self::Text, crate::i18n::preferences::COLOR_TEXT),
        (Self::Muted, crate::i18n::preferences::COLOR_MUTED),
        (Self::Border, crate::i18n::preferences::COLOR_BORDER),
        (Self::Hover, crate::i18n::preferences::COLOR_HOVER),
        (Self::Pressed, crate::i18n::preferences::COLOR_PRESSED),
        (Self::Accent, crate::i18n::preferences::COLOR_ACCENT),
        (Self::OnAccent, crate::i18n::preferences::COLOR_ON_ACCENT),
        (Self::Code, crate::i18n::preferences::COLOR_CODE),
        (Self::Selection, crate::i18n::preferences::COLOR_SELECTION),
        (Self::Danger, crate::i18n::preferences::COLOR_DANGER),
    ];
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeSettings {
    pub active: String,
    pub palette: Palette,
    pub saved: BTreeMap<String, Palette>,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            active: "浅色".into(),
            palette: Palette::light(),
            saved: BTreeMap::new(),
        }
    }
}

impl ThemeSettings {
    pub fn select(&mut self, name: &str) {
        // Saved colors may override a preset; built-in palettes remain the
        // authoritative defaults restored by the appearance reset action.
        let palette = self.saved.get(name).copied().or_else(|| match name {
            "浅色" => Some(Palette::light()),
            "深色" => Some(Palette::dark()),
            _ => None,
        });
        if let Some(palette) = palette {
            self.palette = palette;
            self.active = name.to_owned();
        }
    }

    pub fn save(&mut self, name: &str) -> Result<(), &'static str> {
        let name = name.trim();
        if name.is_empty() {
            return Err(crate::i18n::preferences::ERR_THEME_NAME_EMPTY.get());
        }
        if name == "自定义（未保存）" {
            return Err(crate::i18n::preferences::ERR_THEME_NAME_RESERVED.get());
        }
        self.saved.insert(name.to_owned(), self.palette);
        self.active = name.to_owned();
        Ok(())
    }

    /// Discard preset overrides while retaining separately named user themes.
    pub fn restore_presets(&mut self) {
        self.saved.remove("浅色");
        self.saved.remove("深色");
        self.select("浅色");
    }

    /// Upgrade unmodified legacy built-in palettes to the current Light / Dark defaults
    /// while leaving any user-saved overrides or custom edits untouched.
    pub fn migrate_builtin_defaults(&mut self) {
        if self.active == "浅色"
            && !self.saved.contains_key("浅色")
            && self.palette == Palette::LEGACY_LIGHT
        {
            self.palette = Palette::light();
        } else if self.active == "深色"
            && !self.saved.contains_key("深色")
            && self.palette == Palette::LEGACY_DARK
        {
            self.palette = Palette::dark();
        }
    }
}

thread_local! { static ACTIVE: Cell<Option<Palette>> = const { Cell::new(None) }; }

pub fn palette() -> Palette {
    ACTIVE.with(Cell::get).unwrap_or_else(Palette::light)
}
pub fn color(original: u32) -> Rgba {
    palette().resolve(original)
}
pub fn on_accent() -> Rgba {
    rgb(palette().on_accent)
}

fn mix(base: u32, tint: u32, amount: f32) -> Rgba {
    let base = rgb(base);
    let tint = rgb(tint);
    Rgba {
        r: base.r * (1. - amount) + tint.r * amount,
        g: base.g * (1. - amount) + tint.g * amount,
        b: base.b * (1. - amount) + tint.b * amount,
        a: 1.,
    }
}

pub(crate) fn tool_typography(settings: &crate::model::AppSettings) -> (SharedString, Pixels) {
    let code_font = if settings.block_code_font.is_empty() {
        "Consolas".into()
    } else {
        settings.block_code_font.clone().into()
    };
    (code_font, px(settings.ai_font_size as f32 * settings.block_code_scale))
}

pub fn apply_settings(settings: &crate::model::AppSettings, cx: &mut App) {
    cx.text_system().set_default_font_fallbacks(
        (!settings.cjk_fallback_font.is_empty()).then(|| {
            gpui_kit::FontFallbacks::from_fonts(vec![settings.cjk_fallback_font.clone()])
        }),
    );
    cx.text_system().set_cjk_font_scale(settings.cjk_font_scale);
    let document_typography = gpui_kit::base::Theme::global(cx).tokens.typography;
    apply(settings.theme.palette, cx);

    let (tool_font, tool_size) = tool_typography(settings);
    let ui_font: SharedString = if settings.interface_font.trim().is_empty() {
        "Segoe UI".into()
    } else {
        settings.interface_font.trim().to_owned().into()
    };
    Theme::update(cx, |theme| {
        // Native Diff reads the component monospace font at row layout time.
        theme.font_family = ui_font;
        theme.mono_font_family = tool_font;
        theme.mono_font_size = tool_size;
    });
    // The Base projection also serves Markdown and PDF. Keep their independent
    // code typography when syncing the native component theme for AI tools.
    let base = gpui_kit::base::Theme::global_mut(cx);
    base.tokens.typography.mono = document_typography.mono;
    base.tokens.typography.mono_md = document_typography.mono_md;
    base.scrollbar = base
        .scrollbar
        .clone()
        .with_mode(gpui_kit::base::ScrollbarMode::Hover)
        .with_motion(
            gpui_kit::base::ScrollbarMotion::default()
                .with_idle(std::time::Duration::from_millis(600))
                .with_enter(std::time::Duration::from_millis(120))
                .with_exit(std::time::Duration::from_millis(200))
                .with_expand(std::time::Duration::from_millis(120))
                .with_entrance(gpui_kit::base::ScrollbarEntrance::Fade)
                .with_thumb_hover_entrance(gpui_kit::base::ScrollbarEntrance::Fade),
        );
    gpui_kit::base::set_smooth_caret(
        settings.smooth_caret,
        Some(Hsla::from(rgb(settings.theme.palette.accent))),
        cx,
    );
}

/// Apply only colors. Font families, sizes, spacing and document data are untouched.
fn apply(palette: Palette, cx: &mut App) {
    if ACTIVE.with(|active| active.replace(Some(palette))) == Some(palette) {
        return;
    }
    let saved_font = Theme::global(cx).font_family.clone();
    let mode = if palette.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    // Changing mode loads a registered palette; project our colors afterwards.
    if Theme::global(cx).mode != mode {
        Theme::change(mode, None, cx);
    }
    let card_border: Hsla = palette.card_border().into();
    let elevated: Hsla = palette.elevated_surface().into();
    let link: Hsla = palette.link_color().into();
    let selection_alpha = if palette.is_dark() { 0.28 } else { 0.20 };
    Theme::update(cx, |theme| {
        theme.font_family = saved_font;
        let c = &mut theme.colors;
        c.background = rgb(palette.surface).into();
        c.foreground = rgb(palette.text).into();
        c.border = rgb(palette.border).into();
        c.input = card_border;
        c.muted = rgb(palette.code_background).into();
        c.muted_foreground = rgb(palette.muted).into();
        c.popover = elevated;
        c.popover_foreground = rgb(palette.text).into();
        c.primary = rgb(palette.accent).into();
        c.primary_hover = palette.accent_hover().into();
        c.primary_active = palette.accent_active().into();
        c.primary_foreground = rgb(palette.on_accent).into();
        c.accent = rgb(palette.hover).into();
        c.accent_foreground = rgb(palette.text).into();
        c.secondary = rgb(palette.background).into();
        c.secondary_foreground = rgb(palette.text).into();
        c.secondary_hover = rgb(palette.hover).into();
        c.secondary_active = rgb(palette.pressed).into();
        c.list = rgb(palette.surface).into();
        c.list_hover = rgb(palette.hover).into();
        c.list_active = rgb(palette.selection).into();
        c.list_active_border = rgb(palette.accent).into();
        c.link = link;
        c.link_hover = link;
        c.link_active = link;
        c.ring = rgb(palette.accent).into();
        c.caret = rgb(palette.accent).into();
        c.selection = Hsla::from(rgb(palette.accent)).alpha(selection_alpha);
        c.table = rgb(palette.surface).into();
        c.table_head = rgb(palette.code_background).into();
        c.table_head_foreground = rgb(palette.text).into();
        c.table_hover = rgb(palette.hover).into();
        c.table_row_border = rgb(palette.border).into();
        c.table_even = rgb(palette.surface).into();
        c.sidebar = rgb(palette.background).into();
        c.sidebar_foreground = rgb(palette.text).into();
        c.sidebar_border = rgb(palette.border).into();
        c.title_bar = rgb(palette.background).into();
        c.title_bar_border = rgb(palette.border).into();
        let syntax = &mut std::sync::Arc::make_mut(&mut theme.highlight_theme).style;
        syntax.editor_background = Some(rgb(palette.code_background).into());
        syntax.editor_foreground = Some(rgb(palette.text).into());
        syntax.editor_gutter_background = Some(rgb(palette.code_background).into());
        syntax.editor_line_number = Some(rgb(palette.muted).into());
        syntax.editor_active_line_number = Some(rgb(palette.text).into());
    });
}
