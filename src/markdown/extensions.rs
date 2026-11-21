//! Content extensions shared by the editor, AI replies, and annotation placement.

use crate::math::{MathDisplaySize, MathRenderer};
use gpui_kit::{base::text::MarkdownExtensions, *};
use markdown_editor::{callout_container, callout_header};

pub(super) const CALLOUTS: &[(&str, &str, u32)] = &[
    ("note", "ⓘ  Note", 0x0969da),
    ("info", "ⓘ  Info", 0x0969da),
    ("abstract", "≡  Abstract", 0x0969da),
    ("summary", "≡  Summary", 0x0969da),
    ("tldr", "≡  TL;DR", 0x0969da),
    ("tip", "✓  Tip", 0x1a7f37),
    ("success", "✓  Success", 0x1a7f37),
    ("check", "✓  Check", 0x1a7f37),
    ("done", "✓  Done", 0x1a7f37),
    ("important", "◉  Important", 0x8250df),
    ("warning", "⚠  Warning", 0x9a6700),
    ("question", "?  Question", 0x9a6700),
    ("help", "?  Help", 0x9a6700),
    ("faq", "?  FAQ", 0x9a6700),
    ("todo", "☐  Todo", 0x0969da),
    ("caution", "⊗  Caution", 0xcf222e),
    ("danger", "⊗  Danger", 0xcf222e),
    ("error", "⊗  Error", 0xcf222e),
    ("failure", "✕  Failure", 0xcf222e),
    ("fail", "✕  Fail", 0xcf222e),
    ("missing", "✕  Missing", 0xcf222e),
    ("bug", "✦  Bug", 0xcf222e),
    ("example", "❖  Example", 0x8250df),
    ("quote", "❝  Quote", 0x6e7781),
    ("cite", "❝  Cite", 0x6e7781),
];

/// Build Folio's shared Markdown extensions (math typesetting + 25 callout surfaces).
pub(crate) fn markdown_extensions(
    math: &Entity<MathRenderer>,
    display_size: MathDisplaySize,
    cx: &App,
) -> MarkdownExtensions {
    let mut extensions = MarkdownExtensions::default()
        .plugin(math.read(cx).plugin(false, math.clone(), display_size))
        .plugin(math.read(cx).plugin(true, math.clone(), display_size));
    for &(name, title, color) in CALLOUTS {
        let hsla: Hsla = gpui_kit::rgb(color).into();
        extensions = extensions.block_surface(name, move |_, _| {
            callout_container(hsla).child(callout_header(title.into(), hsla).pb(px(6.)))
        });
    }
    extensions
}
