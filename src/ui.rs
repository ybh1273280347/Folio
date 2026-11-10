//! Shared Lucide controls use the same restrained icon treatment as Folio's web UI.
use gpui_kit::{
    component::{Icon, IconName},
    *,
};

pub fn label(text: SharedString) -> AnyElement {
    let (icon, caption) = match text.as_ref() {
        "×" => (IconName::Close, ""),
        "+" => (IconName::Plus, ""),
        "−" => (IconName::Minus, ""),
        "□" => (IconName::WindowMaximize, ""),
        "☰" => (IconName::PanelLeft, ""),
        "⌕" => (IconName::Search, ""),
        "▱" => (IconName::FolderOpen, ""),
        "◫" => (IconName::PanelRight, ""),
        "☷" => (IconName::Menu, ""),
        "▦" => (IconName::Frame, ""),
        "▧" => (IconName::GalleryVerticalEnd, ""),
        "‹" => (IconName::ChevronLeft, ""),
        "›" => (IconName::ChevronRight, ""),
        "↓" => (IconName::ArrowDown, ""),
        "✓" => (IconName::Check, ""),
        "⚙  设置" => (IconName::Settings, "设置"),
        "终端" => (IconName::SquareTerminal, "终端"),
        "▶  编译" => (IconName::Play, "编译"),
        "■  停止" => (IconName::Square, "停止"),
        _ => return div().child(text).into_any_element(),
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(Icon::new(icon).w(px(14.)).h(px(14.)))
        .children((!caption.is_empty()).then_some(caption))
        .into_any_element()
}
