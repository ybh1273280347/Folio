//! Project the editor palette into terminal defaults and semantic ANSI colors.
use crate::{model::TerminalSettings, theme::Palette};
use gpui_kit::{Edges, px};
use gpui_xterm::{ColorPalette, TerminalConfig};

pub fn config(settings: &TerminalSettings, palette: Palette) -> TerminalConfig {
    let rgb = |value: u32| ((value >> 16) as u8, (value >> 8) as u8, value as u8);
    let (r, g, b) = rgb(palette.surface);
    let colors = ColorPalette::builder().background(r, g, b);
    let (r, g, b) = rgb(palette.text);
    let colors = colors.foreground(r, g, b).cursor(r, g, b);
    let (r, g, b) = rgb(palette.selection);
    let colors = colors.selection(r, g, b);
    // Light-mode ANSI white slots map to readable grays/charcoal to avoid the
    // white-on-white trap; gpui-xterm's minimum_contrast also guards TrueColor.
    let colors = if palette.is_dark() {
        colors
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
            .bright_white(0xfa, 0xf9, 0xf5)
    } else {
        colors
            .black(0x37, 0x35, 0x2f)
            .red(0xb0, 0x35, 0x35)
            .green(0x3e, 0x73, 0x42)
            .yellow(0x8c, 0x65, 0x16)
            .blue(0x32, 0x62, 0x9b)
            .magenta(0x86, 0x48, 0x9e)
            .cyan(0x28, 0x72, 0x75)
            .white(0x5c, 0x5b, 0x56)
            .bright_black(0x73, 0x72, 0x6f)
            .bright_red(0xca, 0x49, 0x49)
            .bright_green(0x4b, 0x85, 0x4e)
            .bright_yellow(0xa2, 0x76, 0x1b)
            .bright_blue(0x47, 0x7b, 0xb8)
            .bright_magenta(0xa1, 0x60, 0xb9)
            .bright_cyan(0x36, 0x8d, 0x90)
            .bright_white(0x37, 0x35, 0x2f)
    };
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
