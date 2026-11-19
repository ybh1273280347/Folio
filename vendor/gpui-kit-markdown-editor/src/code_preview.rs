use std::{collections::HashMap, sync::{Arc, Mutex}};

use gpui::{
    AnyElement, App, ClipboardItem, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, Styled, Window, canvas, div, px,
};
use gpui_kit::{
    assets::IconName,
    base::text::{
        editor_host::CodeBlock,
        MarkdownExtensions, MarkdownNode,
    },
    component::{button::{Button, ButtonVariants}, ActiveTheme as _, Icon},
};

use crate::{BlockEditorState, BlockId};

pub(crate) const NAME: &str = "editor-code-preview";
pub(crate) type BoundsMap = Arc<Mutex<HashMap<BlockId, gpui::Bounds<gpui::Pixels>>>>;
pub(crate) type Renderer = Arc<dyn Fn(BlockId, u64, &CodeBlock, bool, &mut Window, &mut App) -> AnyElement + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Registration {
    pub(crate) language: SharedString,
    pub(crate) renderer: Renderer,
}

pub(crate) struct Data {
    pub(crate) state: Entity<BlockEditorState>,
    pub(crate) block: BlockId,
    pub(crate) revision: u64,
    pub(crate) code: CodeBlock,
    pub(crate) renderer: Renderer,
    pub(crate) bounds: Option<BoundsMap>,
    pub(crate) toolbar: bool,
}

pub(crate) fn node(
    state: Entity<BlockEditorState>,
    block: BlockId,
    revision: u64,
    code: CodeBlock,
    renderer: Renderer,
    bounds: Option<BoundsMap>,
    toolbar: bool,
) -> MarkdownNode {
    let text: SharedString = code.code();
    MarkdownNode::new(
        NAME,
        Data {
            state,
            block,
            revision,
            code,
            renderer,
            bounds,
            toolbar,
        },
    )
    .text(text)
    .accessibility_label("Mermaid")
}

pub(crate) fn register_renderer(extensions: &mut MarkdownExtensions) {
    *extensions = std::mem::take(extensions).block_renderer(NAME, |node, window, cx| {
        let data = node.data::<Data>().expect("editor code preview data");
        render(data, window, cx)
    });
}

fn render(data: &Data, window: &mut Window, cx: &mut App) -> AnyElement {
    let chart = (data.renderer)(data.block, data.revision, &data.code, data.toolbar, window, cx);
    let block = data.block;
    let chart = div().relative().w_full().child(chart);
    let bounds = data.bounds.clone();

    let state = data.state.clone();
    let readonly = state.read(cx).is_readonly();
    let toolbar = if data.toolbar {
        let edit_state = state.clone();
        let copy_state = state.clone();
        let mut actions = div().flex().items_center().gap(px(4.));
        if !readonly {
            actions = actions.child(
                Button::new(("editor-code-preview-edit", block.as_u64() as usize))
                    .ghost()
                    .compact()
                    .w(px(28.)).h(px(28.))
                    .icon(Icon::new(IconName::PencilLine).w(px(14.)).h(px(14.)))
                    .tooltip("编辑源码")
                    .on_click(move |_, window, cx| {
                        edit_state.update(cx, |state, cx| {
                            state.focus(window, cx);
                            state.enter_code_preview_source(block, cx);
                        });
                    }),
            );
        }
        actions = actions.child(
            Button::new(("editor-code-preview-copy", block.as_u64() as usize))
                .ghost()
                .compact()
                .w(px(28.)).h(px(28.))
                .icon(Icon::new(IconName::Copy).w(px(14.)).h(px(14.)))
                .tooltip("复制源码")
                .on_click(move |_, _, cx| {
                    let code = copy_state.read(cx).model().document().block(block)
                        .and_then(|block| block.text())
                        .map(str::to_owned)
                        .expect("rendered preview retains its code text");
                    cx.write_to_clipboard(ClipboardItem::new_string(code));
                }),
        );
        if state.read(cx).content_folding {
            actions = actions.child(super::element::content_fold_button(&state, block, "代码块", cx));
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(8.))
            .h(px(32.))
            .bg(cx.theme().popover.opacity(0.35))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(12.)).child("Mermaid"))
            .child(actions)
            .into_any_element()
    } else {
        div().into_any_element()
    };

    let state = data.state.clone();
    div()
        .id(("editor-code-preview", block.as_u64() as usize))
        .relative()
        .w_full()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            state.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_code_preview(block, event.modifiers.shift, cx);
            });
        })
        .child(toolbar)
        .child(chart)
        .child(
            canvas(
                move |area, _, _| {
                    if let Some(bounds) = &bounds {
                        bounds.lock().expect("code preview bounds").insert(block, area);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}
