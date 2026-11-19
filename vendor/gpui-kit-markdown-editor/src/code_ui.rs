//! Code controls belong to the rendered block, independently of hover and caret state.
use std::time::Duration;
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, InteractiveElement,
    IntoElement, MouseButton, ParentElement, SharedString, Styled, Subscription, Task,
    Window, div, px,
};
use gpui_kit::{
    assets::IconName,
    base::text::editor_host::RenderContext,
    component::{
        ActiveTheme as _, Icon, Sizable,
        button::{Button, ButtonVariants},
        select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    },
};
use crate::{BlockEditorState, BlockId};

// Only the menu is bounded; imported fences retain their original language token.
const LANGUAGES: &[(&str, &str)] = &[
    ("", "纯文本"), ("rust", "Rust"), ("python", "Python"),
    ("javascript", "JavaScript"), ("typescript", "TypeScript"), ("tsx", "TSX"),
    ("c", "C"), ("cpp", "C++"), ("csharp", "C#"), ("java", "Java"),
    ("go", "Go"), ("bash", "Shell / Bash"), ("powershell", "PowerShell"),
    ("html", "HTML"), ("css", "CSS"), ("sql", "SQL"),
    ("latex", "LaTeX"), ("markdown", "Markdown"), ("mermaid", "Mermaid"),
];

#[derive(Clone)]
struct Language {
    token: String,
    label: SharedString,
}

impl SelectItem for Language {
    type Value = String;
    fn title(&self) -> SharedString { self.label.clone() }
    fn value(&self) -> &String { &self.token }
    fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        self.token.to_lowercase().contains(&query) || self.label.to_lowercase().contains(&query)
    }
}

fn languages(current: &str) -> SearchableVec<Language> {
    let mut items: Vec<_> = LANGUAGES.iter().map(|&(token, label)| Language {
        token: token.to_owned(), label: label.into(),
    }).collect();
    if !LANGUAGES.iter().any(|&(token, _)| token == current) {
        items.push(Language { token: current.to_owned(), label: current.to_owned().into() });
    }
    SearchableVec::new(items)
}

// Window-keyed state disappears when this block leaves the rendered viewport.
struct CodeTools {
    selector: Entity<SelectState<SearchableVec<Language>>>,
    copied: bool,
    reset_copy: Task<()>,
    _selection: Subscription,
}

impl CodeTools {
    fn new(editor: Entity<BlockEditorState>, block: BlockId, language: &str,
        window: &mut Window, cx: &mut Context<Self>) -> Self
    {
        let selector = cx.new(|cx| {
            let mut state = SelectState::new(languages(language), None, window, cx).searchable(true);
            state.set_selected_value(&language.to_owned(), window, cx);
            state
        });
        let selection = cx.subscribe_in(&selector, window,
            move |_, _, event: &SelectEvent<SearchableVec<Language>>, window, cx| {
                if let SelectEvent::Confirm(Some(token)) = event {
                    editor.update(cx, |editor, cx| {
                        editor.set_code_language(block,
                            (!token.is_empty()).then(|| token.clone()), cx);
                    });
                    // Select restores its own focus after confirmation; return to writing afterwards.
                    let editor = editor.clone();
                    window.defer(cx, move |window, cx| {
                        editor.update(cx, |editor, cx| editor.focus(window, cx));
                    });
                }
            });
        Self { selector, copied: false, reset_copy: Task::ready(()), _selection: selection }
    }

    fn copied(&mut self, cx: &mut Context<Self>) {
        self.copied = true;
        cx.notify();
        // Replacing the task restarts the feedback interval on repeated copies.
        self.reset_copy = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        });
    }
}

// One surface owns the header and body background and outline.
pub(crate) fn container(context: &RenderContext) -> gpui::Div {
    let style = context.style();
    div()
        .relative()
        .w_full()
        .min_w_0()
        .bg(style.code_block().background.clone()
            .unwrap_or_else(|| style.code_background().into()))
        .border_1()
        .border_color(style.border())
        .rounded(px(8.))
}

pub(crate) fn header(
    state: &Entity<BlockEditorState>, block: BlockId,
    window: &mut Window, cx: &mut App,
) -> AnyElement {
    let language = state.read(cx).model().document().block(block)
        .and_then(|block| block.code_language()).unwrap_or("").to_owned();
    let readonly = state.read(cx).is_readonly();
    let tools = window.use_keyed_state(("code-tools", block.as_u64() as usize), cx,
        |window, cx| CodeTools::new(state.clone(), block, &language, window, cx));
    let selector = tools.read(cx).selector.clone();
    // Only external language changes (including undo) resync; typing a query must retain it.
    if selector.read(cx).selected_value() != Some(&language) {
        selector.update(cx, |selector, cx| {
            selector.set_items(languages(&language), window, cx);
            selector.set_selected_value(&language, window, cx);
        });
    }
    let copied = tools.read(cx).copied;
    let source = state.read(cx).code_preview_source() == Some(block);
    let copy_editor = state.clone();
    let source_editor = state.clone();
    let theme = cx.theme();
    let copy = Button::new(("copy", block.as_u64() as usize))
        .ghost().compact().w(px(28.)).h(px(28.))
        .icon(Icon::new(if copied { IconName::Check } else { IconName::Copy })
            .w(px(14.)).h(px(14.)))
        .tooltip(if copied { "已复制" } else { "复制代码" })
        .on_click(move |_, _, cx| {
            let text = copy_editor.read(cx).model().document().block(block)
                .and_then(|block| block.text()).map(str::to_owned);
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                tools.update(cx, |tools, cx| tools.copied(cx));
            }
        });
    let mut actions = div().flex().items_center().gap(px(4.)).child(copy);
    if source {
        actions = actions.child(
            Button::new(("code-preview", block.as_u64() as usize))
                .ghost().compact().w(px(28.)).h(px(28.))
                .icon(Icon::new(IconName::ChartNetwork).w(px(14.)).h(px(14.)))
                .tooltip("显示图表")
                .on_click(move |_, _, cx| {
                    source_editor.update(cx, |state, cx| state.exit_code_preview_source(cx));
                }),
        );
    }
    if state.read(cx).content_folding {
        actions = actions.child(super::element::content_fold_button(state, block, "代码块", cx));
    }
    let header = div()
        .id(("code-tools", block.as_u64() as usize))
        .w_full().px_2().py_1().flex().items_center().justify_between()
        .border_color(theme.border.opacity(0.5));
    let header = if state.read(cx).is_fold_open(block) { header.border_b_1() } else { header };
    header
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border.opacity(0.65))
                .bg(theme.popover.opacity(0.35))
                .px_1()
                .child(
                    Select::new(&selector).small().appearance(false).w(px(160.))
                        .text_size(px(13.))
                        .menu_width(px(230.)).search_placeholder("搜索语言…")
                        .disabled(readonly),
                ),
        )
        .child(actions)
        .into_any_element()
}
