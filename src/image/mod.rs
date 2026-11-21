//! Clipboard image/SVG extraction and zoomable/pannable image viewer pane.
pub mod clipboard;
pub mod viewer;

pub use clipboard::{
    markdown_paste_image_dir, markdown_relative_image_ref, resolve_markdown_image_source,
    save_clipboard_images_to_dir,
};
pub use viewer::ImagePane;
