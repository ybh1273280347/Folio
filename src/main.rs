//! Isolated native desktop entry point; Electron sources and data stay untouched.
#![cfg_attr(windows, windows_subsystem = "windows")]
mod compiler;
mod files;
mod markdown;
mod math;
mod model;
mod pdf;
mod preferences;
mod richtext;
mod storage;
mod ui;
mod workspace;

use gpui_kit::*;
use workspace::Workspace;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-s", workspace::Save, None),
            KeyBinding::new("ctrl-o", workspace::OpenProject, None),
            KeyBinding::new("ctrl-enter", workspace::Compile, None),
            KeyBinding::new("ctrl-b", workspace::ToggleSidebar, None),
            KeyBinding::new("ctrl-p", workspace::FindFile, None),
            KeyBinding::new("ctrl-w", workspace::CloseTab, None),
            KeyBinding::new("ctrl-shift-s", workspace::SaveAll, None),
            KeyBinding::new("ctrl-c", gpui_kit::component::input::Copy, Some("FolioPdf")),
            KeyBinding::new("ctrl-e", markdown::ToggleSource, None),
            KeyBinding::new("escape", workspace::Dismiss, None),
        ]);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1480.), px(960.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Folio · LaTeX 工作台".into()),
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
