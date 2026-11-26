//! Appearance contracts can run without starting a GPU window or opening user files.
#![allow(dead_code)]
#[path = "../src/i18n/mod.rs"]
mod i18n;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/theme.rs"]
mod theme;
#[path = "../src/ui/mod.rs"]
mod ui;

use gpui_kit::px;
use model::AppSettings;
use theme::{ColorRole, Palette};

#[test]
fn existing_preferences_keep_the_light_theme_and_their_typography() {
    let settings: AppSettings = serde_json::from_str(
        r#"{"markdownFontSize":22,"blockCodeFont":"Consolas","inlineCodeScale":0.8}"#,
    )
    .unwrap();
    assert_eq!(settings.theme.palette, Palette::light());
    assert_eq!(settings.markdown_font_size, 22);
    assert_eq!(settings.block_code_font, "Consolas");
    assert_eq!(settings.inline_code_scale, 0.8);
}

#[test]
fn custom_colors_survive_save_switch_and_preferences_round_trip() {
    let mut settings = AppSettings::default();
    settings.editor_font = "Consolas".into();
    settings.markdown_font = "Georgia".into();
    settings.markdown_font_size = 23;
    settings.ai_font = "Verdana".into();
    settings.ai_font_size = 16;
    settings.math_scale = 2.1;
    settings.theme.select("深色");
    settings.theme.palette.set(ColorRole::Accent, 0x84a578);
    let custom = settings.theme.palette;
    settings.theme.save("夜间写作").unwrap();
    settings.theme.select("浅色");
    assert_eq!(settings.theme.palette, Palette::light());
    let mut restored: AppSettings =
        serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
    restored.theme.select("夜间写作");
    assert_eq!(restored.theme.palette, custom);
    assert_eq!(restored.editor_font, "Consolas");
    assert_eq!(restored.markdown_font, "Georgia");
    assert_eq!(restored.markdown_font_size, 23);
    assert_eq!(restored.ai_font, "Verdana");
    assert_eq!(restored.ai_font_size, 16);
    assert_eq!(restored.math_scale, 2.1);
    restored.theme.select("深色");
    assert_eq!(restored.theme.palette, Palette::dark());
}

#[test]
fn preset_overrides_persist_and_reset_restores_both_builtins() {
    let mut settings = AppSettings::default();
    settings.theme.save("自用主题").unwrap();
    for name in ["浅色", "深色"] {
        settings.theme.select(name);
        settings.theme.palette.set(ColorRole::Accent, 0x84a578);
        settings.theme.save(name).unwrap();
    }
    let mut restored: AppSettings =
        serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
    for name in ["浅色", "深色"] {
        restored.theme.select(name);
        assert_eq!(restored.theme.palette.accent, 0x84a578);
    }
    restored.theme.restore_presets();
    assert_eq!(restored.theme.palette, Palette::light());
    restored.theme.select("深色");
    assert_eq!(restored.theme.palette, Palette::dark());
    restored.theme.select("自用主题");
    assert_eq!(restored.theme.active, "自用主题");
    assert_eq!(restored.theme.palette, Palette::light());
}

#[test]
fn fenced_code_scales_with_body_size_and_keeps_its_selected_font() {
    let mut settings = AppSettings::default();
    settings.block_code_font = "Consolas".into();
    settings.block_code_scale = 0.75;
    settings.markdown_font_size = 16;
    settings.ai_font_size = 24;
    let markdown = ui::block_code_style(&settings, px(settings.markdown_font_size as f32));
    let ai = ui::block_code_style(&settings, px(settings.ai_font_size as f32));
    assert_eq!(markdown.text.font_size.unwrap().to_pixels(px(16.)), px(12.));
    assert_eq!(ai.text.font_size.unwrap().to_pixels(px(16.)), px(18.));
    assert_eq!(markdown.text.font_family.as_deref(), Some("Consolas"));
    assert_eq!(ai.text.font_family.as_deref(), Some("Consolas"));
}

#[test]
fn legacy_builtin_palettes_migrate_without_overwriting_custom_overrides() {
    let mut light = AppSettings::default();
    light.theme.active = "浅色".into();
    light.theme.palette = Palette::LEGACY_LIGHT;
    light.theme.migrate_builtin_defaults();
    assert_eq!(light.theme.palette, Palette::light());

    let mut dark = AppSettings::default();
    dark.theme.active = "深色".into();
    dark.theme.palette = Palette::LEGACY_DARK;
    dark.theme.migrate_builtin_defaults();
    assert_eq!(dark.theme.palette, Palette::dark());

    let mut custom_override = AppSettings::default();
    custom_override.theme.active = "浅色".into();
    custom_override.theme.palette = Palette::LEGACY_LIGHT;
    custom_override.theme.saved.insert("浅色".into(), Palette::LEGACY_LIGHT);
    custom_override.theme.migrate_builtin_defaults();
    assert_eq!(custom_override.theme.palette, Palette::LEGACY_LIGHT);
}
