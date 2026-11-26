//! Shared surface containers, scrollbars, and compact tooltips.
use gpui_kit::{
    base::{Scrollbar, ScrollbarMode},
    component::tooltip::Tooltip,
    *,
};

/// Floating popover card matching Folio's paper-chrome aesthetic:
/// `10px` radius, `elevated_surface`, crisp `1px card_border`, and tight dual-layer shadow.
pub fn popover_card(id: impl Into<ElementId>) -> Stateful<Div> {
    let palette = crate::theme::palette();
    div()
        .id(id)
        .rounded(px(10.))
        .bg(palette.elevated_surface())
        .border_1()
        .border_color(palette.card_border())
        .shadow(palette.popover_shadow())
        .text_color(gpui_kit::rgb(palette.text))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_scroll(|_, _, cx| cx.stop_propagation())
}

/// Unified 5px hover-fade vertical scrollbar driven by the active `Palette`.
pub fn scrollbar_vertical(handle: &ScrollHandle) -> Scrollbar {
    let palette = crate::theme::palette();
    Scrollbar::vertical(handle)
        .mode(ScrollbarMode::Hover)
        .styles(move |styles| {
            styles
                .thumb(move |s| {
                    s.bg(palette.scroll_thumb())
                        .width(px(5.))
                        .inset(px(1.))
                        .radius(px(3.))
                        .min_length(px(24.))
                })
                .thumb_hover(move |s| {
                    s.bg(palette.scroll_thumb_hover())
                        .width(px(5.))
                        .inset(px(1.))
                        .radius(px(3.))
                })
                .thumb_active(move |s| {
                    s.bg(palette.scroll_thumb_active())
                        .width(px(5.))
                        .inset(px(1.))
                        .radius(px(3.))
                })
        })
}

/// Compact Folio tooltip aligned with the active palette's border and shadow tokens.
pub fn tooltip(text: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> AnyView {
    let palette = crate::theme::palette();
    Tooltip::new(text.into())
        .text_size(px(11.5))
        .rounded(px(6.))
        .px_2()
        .py(px(3.))
        .border_color(palette.card_border())
        .shadow(palette.popover_shadow())
        .build(window, cx)
}
