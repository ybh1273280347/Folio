//! Shared Lucide controls use the same restrained icon treatment as Folio's web UI.
use gpui_kit::{
    assets::{IconName, icon_assets},
    base::StyledExt,
    component::Icon,
    prelude::FluentBuilder,
    *,
};
use std::borrow::Cow;

icon_assets!(
    FolioIcons,
    [
        ListTree,
        Images,
        Image,
        CodeXml,
        Code,
        PencilLine,
        Trash,
        SlidersHorizontal,
        FileCode,
        FileImage,
        Library,
        Columns2,
        Maximize2,
        PanelLeftClose,
        PanelLeftOpen,
        PanelRightOpen,
        Bold,
        Italic,
        Underline,
        Strikethrough,
        Highlighter,
        Heading1,
        Heading2,
        Heading3,
        Heading4,
        List,
        ListOrdered,
        ListTodo,
        TextQuote,
        Sigma,
        Table2,
        Ellipsis,
        Download,
        FolderPlus,
        Link,
        MessageSquarePlus,
        MessageSquareText,
        TextAlignStart,
        TextAlignCenter,
        TextAlignEnd,
        ListIndentIncrease,
        ListIndentDecrease,
        GripVertical,
        Keyboard,
        Palette,
        RotateCcw,
        Type,
        Sparkles,
        Check
    ]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // Features expose icon names; the app must also register their SVG source.
        if let Some(bytes) = FolioIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(FolioIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn icon(name: IconName) -> Icon {
    Icon::new(name).w(px(14.)).h(px(14.))
}

// ---------------------------------------------------------------------------
// Interaction feel
//
// Every clickable surface has three distinct states: rest, hover and pressed.
// The pressed state is what makes a click feel "solid": the surface darkens a
// step further, any outer lift shadow collapses and a faint inset shadow makes
// it read as pushed in. Keep new controls on these helpers instead of writing
// ad-hoc `.hover()` colours, otherwise the pressed feedback gets lost.
// ---------------------------------------------------------------------------

/// Neutral surface tints shared by ghost buttons, list rows and menu items.
pub const HOVER_BG: u32 = 0xefefed;
pub const PRESSED_BG: u32 = 0xe4e3df;

fn inset_press(alpha: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., alpha)).blur_radius(px(2.)).inset()]
}

/// Raised look for bordered/filled buttons: a short drop shadow plus a hairline
/// top highlight so the surface reads as slightly convex.
fn raised(alpha: f32, highlight: f32) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., alpha)).blur_radius(px(2.)),
        BoxShadow::new(px(0.), px(1.), hsla(0., 0., 1., highlight)).inset(),
    ]
}

pub fn control(id: impl Into<ElementId>, name: IconName) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(27.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(rgb(0x7d838b))
        .hover(|s| s.bg(rgb(0xe9ebed)).text_color(rgb(0x343a40)))
        .active(|s| {
            s.bg(rgb(0xdcdfe3))
                .text_color(rgb(0x1f2328))
                .shadow(inset_press(0.10))
        })
        .child(icon(name))
}

pub fn modal_overlay(
    title: impl IntoElement,
    description: impl IntoElement,
    actions: impl IntoElement,
) -> impl IntoElement {
    let card = div()
        .w(px(420.))
        .p_6()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(0xe6e5e1))
        .bg(rgb(0xffffff))
        .shadow(vec![
            BoxShadow::new(px(0.), px(16.), hsla(0., 0., 0., 0.16))
                .blur_radius(px(40.))
                .spread_radius(px(-8.)),
            BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.06)).blur_radius(px(6.)),
        ])
        .flex()
        .flex_col()
        .gap_4()
        .child(div().font_semibold().child(title))
        .child(div().text_color(rgb(0x787774)).child(description))
        .child(actions)
        // Rise in from slightly below while fading in; feels anchored, not popped.
        .relative()
        .with_animation(
            "modal-card",
            Animation::new(std::time::Duration::from_millis(180)).with_easing(ease_out_quint()),
            |card, t| card.top(px(10. * (1. - t))).opacity(0.4 + 0.6 * t),
        );
    div()
        .id("modal-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x1c1b1838))
        // Keep hover / clicks from leaking to the UI behind the dialog.
        .occlude()
        .child(card)
        .with_animation(
            "modal-backdrop-fade",
            Animation::new(std::time::Duration::from_millis(140)).with_easing(ease_out_quint()),
            |backdrop, t| backdrop.opacity(t),
        )
}

fn button_base(id: impl Into<ElementId>, text: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(27.))
        .px_2()
        .py_1()
        .rounded(px(5.))
        .cursor_pointer()
        // Pass "" to build a custom body (icon + text) with `.child(...)`.
        .when(!text.is_empty(), |el| el.child(label(text)))
}

/// Ghost button: transparent at rest, tinted on hover, pushed-in on press.
pub fn button(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .text_color(rgb(0x37352f))
        .hover(|s| s.bg(rgb(HOVER_BG)))
        .active(|s| s.bg(rgb(PRESSED_BG)).shadow(inset_press(0.08)))
}

/// Bordered neutral button with a slight lift (e.g. "编译", "取消").
pub fn button_secondary(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .text_color(rgb(0x37352f))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xdcdbd7))
        .shadow(raised(0.06, 0.0))
        .hover(|s| s.bg(rgb(0xf7f7f5)).border_color(rgb(0xcfcec9)))
        .active(|s| {
            s.bg(rgb(0xedece9))
                .border_color(rgb(0xc5c4bf))
                .shadow(inset_press(0.10))
        })
}

/// Filled brand button for the main affirmative action.
pub fn button_primary(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .px_3()
        .font_medium()
        .text_color(rgb(0xffffff))
        .bg(rgb(0x52769a))
        .border_1()
        .border_color(rgb(0x47698c))
        .shadow(raised(0.14, 0.14))
        .hover(|s| s.bg(rgb(0x486c90)).border_color(rgb(0x3f6082)))
        .active(|s| {
            s.bg(rgb(0x3d5d7e))
                .border_color(rgb(0x355373))
                .shadow(inset_press(0.22))
        })
}

/// Filled red button for destructive confirmations ("删除", "移除项目"...).
pub fn button_danger(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .px_3()
        .font_medium()
        .text_color(rgb(0xffffff))
        .bg(rgb(0xd4373f))
        .border_1()
        .border_color(rgb(0xbf2f37))
        .shadow(raised(0.14, 0.14))
        .hover(|s| s.bg(rgb(0xc42f37)).border_color(rgb(0xae2830)))
        .active(|s| {
            s.bg(rgb(0xa9262e))
                .border_color(rgb(0x962128))
                .shadow(inset_press(0.25))
        })
}

/// Tinted red button for inline destructive actions in toolbars.
pub fn button_danger_soft(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .text_color(rgb(0xcf1322))
        .bg(rgb(0xfff1f0))
        .border_1()
        .border_color(rgb(0xffccc7))
        .shadow(raised(0.05, 0.0))
        .hover(|s| s.bg(rgb(0xffe4e1)).border_color(rgb(0xffa8a0)))
        .active(|s| {
            s.bg(rgb(0xffd3cd))
                .border_color(rgb(0xff8f85))
                .text_color(rgb(0xa8071a))
                .shadow(inset_press(0.10))
        })
}

pub const MAIN_TEX_COLOR: u32 = 0x5c7f9f;

pub fn file_icon(path: &str, asset: bool, main: bool) -> AnyElement {
    if path.to_lowercase().ends_with(".pdf") {
        return div()
            .text_size(px(9.))
            .text_color(rgb(0xb45b57))
            .border_1()
            .border_color(rgb(0xd5a7a3))
            .rounded(px(3.))
            .px_1()
            .flex_shrink_0()
            .child("PDF")
            .into_any_element();
    }
    let name = if asset {
        IconName::FileImage
    } else if path.ends_with(".tex") {
        IconName::FileCode
    } else if path.ends_with(".bib") {
        IconName::Library
    } else {
        IconName::FileText
    };
    div()
        .flex_shrink_0()
        .text_color(rgb(if main {
            MAIN_TEX_COLOR
        } else if path.ends_with(".tex") {
            0x8b989f
        } else {
            0x787774
        }))
        .child(icon(name))
        .into_any_element()
}

pub fn label(text: SharedString) -> AnyElement {
    let (icon, caption) = match text.as_ref() {
        "×" => (IconName::Close, ""),
        "+" => (IconName::Plus, ""),
        "−" => (IconName::Minus, ""),
        "□" => (IconName::Maximize2, ""),
        "☰" => (IconName::PanelLeftClose, ""),
        "⌕" => (IconName::Search, ""),
        "▱" => (IconName::FolderOpen, ""),
        "◫" => (IconName::PanelRightOpen, ""),
        "☷" => (IconName::ListTree, ""),
        "▦" => (IconName::Table2, ""),
        "▧" => (IconName::Images, ""),
        "⌄" => (IconName::ChevronDown, ""),
        "源码" | "所见即所得" => (IconName::CodeXml, ""),
        "B" => (IconName::Bold, ""),
        "I" => (IconName::Italic, ""),
        "H" => (IconName::Heading2, ""),
        "•" => (IconName::List, ""),
        "☑" => (IconName::ListTodo, ""),
        "∑" => (IconName::Sigma, ""),
        "+  添加本地项目" => (IconName::Plus, "添加本地项目"),
        "‹" => (IconName::ChevronLeft, ""),
        "›" => (IconName::ChevronRight, ""),
        "↓" => (IconName::ArrowDown, ""),
        "✓" => (IconName::Check, ""),
        "📁+" => (IconName::FolderPlus, ""),
        "导出" => (IconName::Download, ""),
        "⚙  设置" => (IconName::Settings, "设置"),
        "终端" => (IconName::SquareTerminal, ""),
        "▶  编译" => (IconName::Play, ""),
        "■  停止" => (IconName::Square, ""),
        _ => return div().child(text).into_any_element(),
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .child(self::icon(icon))
        .children((!caption.is_empty()).then_some(caption))
        .into_any_element()
}
