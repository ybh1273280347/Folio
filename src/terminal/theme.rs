//! Project a dark editor palette into terminal defaults and ANSI colors.
use crate::{model::TerminalSettings, theme::Palette};
use gpui_kit::{Edges, px};
use gpui_xterm::{ColorPalette, TerminalConfig};

pub fn terminal_palette(palette: Palette) -> Palette {
    if palette.is_dark() {
        palette
    } else {
        Palette::dark()
    }
}

pub fn config(settings: &TerminalSettings, palette: Palette) -> TerminalConfig {
    let palette = terminal_palette(palette);
    let rgb = |value: u32| ((value >> 16) as u8, (value >> 8) as u8, value as u8);
    let (r, g, b) = rgb(palette.surface);
    let colors = ColorPalette::builder().background(r, g, b);
    let (r, g, b) = rgb(palette.text);
    let colors = colors.foreground(r, g, b).cursor(r, g, b);
    let (r, g, b) = rgb(palette.selection);
    let colors = colors.selection(r, g, b);
    let colors = colors
        .black(0x20, 0x20, 0x1e)
        .red(0xec, 0x83, 0x7b)
        .green(0x9a, 0xbd, 0x81)
        .yellow(0xe7, 0xbd, 0x7c)
        .blue(0x8b, 0xb6, 0xdb)
        .magenta(0xc5, 0xa1, 0xdc)
        .cyan(0x88, 0xc5, 0xc0)
        .white(0xd8, 0xd5, 0xcb)
        .bright_black(0x86, 0x84, 0x7e)
        .bright_red(0xff, 0xa0, 0x96)
        .bright_green(0xb6, 0xd7, 0x9c)
        .bright_yellow(0xff, 0xd9, 0x98)
        .bright_blue(0xa8, 0xd0, 0xf2)
        .bright_magenta(0xdf, 0xbc, 0xf5)
        .bright_cyan(0xa4, 0xe0, 0xda)
        .bright_white(0xfa, 0xf9, 0xf5);
    TerminalConfig {
        font_family: if settings.font_family.trim().is_empty() {
            "Consolas".into()
        } else {
            settings.font_family.clone()
        },
        font_size: px(settings.font_size as f32),
        line_height_multiplier: 1.2,
        padding: Edges::all(px(8.)),
        colors: colors.build(),
        ..Default::default()
    }
}
