//! Isolated native desktop entry point; Electron sources and data stay untouched.
#![cfg_attr(windows, windows_subsystem = "windows")]
mod ai;
mod annotations;
mod compiler;
mod build_log;
mod files;
mod html;
mod markdown;
mod markdown_pdf;
mod math;
mod math_layout;
mod mermaid;
mod model;
mod pdf;
mod preferences;
mod storage;
mod theme;
mod terminal;
mod trash;
mod translate;
mod ui;
mod workspace;

use gpui_kit::*;
use workspace::Workspace;

fn main() {
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
    }
    gpui_kit::application().with_assets(ui::Assets).run(|cx| {
        gpui_kit::init(cx);
        math_layout::register_fonts(cx);
        markdown_editor::init(cx);
        let initial_settings = storage::load_settings().unwrap_or_default();
        theme::apply_settings(&initial_settings, cx);
        workspace::bind_shortcuts(&initial_settings, cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1480.), px(960.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Folio".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_min_size: Some(size(px(900.), px(600.))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("无法打开 Folio 原生窗口");
    });
}
