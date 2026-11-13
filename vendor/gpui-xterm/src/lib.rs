pub mod clipboard;
pub mod colors;
pub mod event;
mod glyphs;
pub mod input;
pub mod mouse;
pub mod render;
pub mod terminal;
pub mod view;

pub use clipboard::Clipboard;
pub use colors::{ColorPalette, ColorPaletteBuilder};
pub use event::{GpuiEventProxy, TerminalEvent};
pub use render::TerminalRenderer;
pub use terminal::TerminalState;
pub use view::{
    BellCallback, ClipboardStoreCallback, ExitCallback, ExitObserver, KeyHandler, ResizeCallback,
    TerminalConfig, TerminalView, TitleCallback, TitleObserver,
};
