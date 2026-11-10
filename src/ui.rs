//! Shared Lucide controls use the same restrained icon treatment as Folio's web UI.
use gpui_kit::{
    assets::{IconName, icon_assets},
    component::Icon,
    *,
};
use std::borrow::Cow;

icon_assets!(
    FolioIcons,
    [
        ListTree,
        Images,
        CodeXml,
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
        Heading2,
        List,
        ListTodo,
        Sigma,
        Table2
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
        .child(icon(name))
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
        .flex_shrink_0()
        .child(self::icon(icon))
        .children((!caption.is_empty()).then_some(caption))
        .into_any_element()
}
