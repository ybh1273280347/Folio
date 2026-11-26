//! Shared Markdown outline panel: heading sanitization, collapsible tree,
//! active-section tracking, and smooth left-side docked / narrow-overlay layout.
use super::{MarkdownPane, OutlineHeading};
use gpui_kit::{
    assets::IconName,
    base::{Scrollbar, ScrollbarMode, StyledExt},
    component::tooltip::Tooltip,
    prelude::FluentBuilder,
    *,
};
use std::collections::BTreeSet;
use std::time::Duration;

pub(super) const PANEL_WIDTH: f32 = 225.0;
pub(super) const AUTO_OPEN_WIDTH: f32 = 1080.0;
pub(super) const OVERLAY_WIDTH: f32 = 940.0;
pub(super) const ANIMATION_MS: u64 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutlineNode {
    pub(super) index: usize,
    pub(super) level: u8,
    pub(super) title: SharedString,
    pub(super) children: Vec<OutlineNode>,
}

impl OutlineNode {
    fn contains_index(&self, target: usize) -> bool {
        self.index == target
            || self
                .children
                .iter()
                .any(|child| child.contains_index(target))
    }
}

/// Build a nested heading tree from flat document headings using a level stack.
pub(super) fn build_tree(headings: &[OutlineHeading]) -> Vec<OutlineNode> {
    let mut roots: Vec<OutlineNode> = Vec::new();
    let mut stack: Vec<OutlineNode> = Vec::new();

    for (index, heading) in headings.iter().enumerate() {
        let node = OutlineNode {
            index,
            level: heading.level,
            title: heading.display_title.as_str().into(),
            children: Vec::new(),
        };
        while let Some(top) = stack.last() {
            if top.level < node.level {
                break;
            }
            let finished = stack.pop().expect("checked non-empty");
            if let Some(parent) = stack.last_mut() {
                parent.children.push(finished);
            } else {
                roots.push(finished);
            }
        }
        stack.push(node);
    }

    while let Some(finished) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(finished);
        } else {
            roots.push(finished);
        }
    }

    roots
}

/// Collect indices of all headings that have at least one child heading.
pub(super) fn parent_indices(nodes: &[OutlineNode]) -> BTreeSet<usize> {
    fn walk(nodes: &[OutlineNode], out: &mut BTreeSet<usize>) {
        for node in nodes {
            if !node.children.is_empty() {
                out.insert(node.index);
                walk(&node.children, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(nodes, &mut out);
    out
}

/// Strip LaTeX math delimiters/commands, Markdown syntax, and inline HTML tags
/// from heading text while preserving numbering (e.g. `一、`, `5.1`, `1.`).
pub(super) fn clean_outline_title(raw: &str) -> String {
    let step1 = strip_markdown_links(raw);
    let step2 = strip_inline_html(&step1);
    let step3 = strip_math_and_latex(&step2);
    let step4 = strip_markdown_marks(&step3);
    let collapsed = collapse_whitespace(&step4);
    if collapsed.is_empty() {
        collapse_whitespace(raw)
    } else {
        collapsed
    }
}

fn strip_markdown_links(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let is_image = bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[';
        if bytes[i] == b'[' || is_image {
            let label_start = if is_image { i + 2 } else { i + 1 };
            if let Some(close_bracket) = find_matching_bracket(input, label_start)
                && close_bracket + 1 < bytes.len()
                && bytes[close_bracket + 1] == b'('
                && let Some(close_paren) = find_matching_paren(input, close_bracket + 2)
            {
                let label = &input[label_start..close_bracket];
                out.push_str(&strip_markdown_links(label));
                i = close_paren + 1;
                continue;
            }
        }
        let ch = input[i..].chars().next().expect("valid utf-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find_matching_bracket(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn find_matching_paren(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn strip_inline_html(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<'
            && let Some(next) = bytes.get(i + 1).copied()
            && (next == b'/' || next.is_ascii_alphabetic())
            && let Some(rel_end) = input[i + 1..].find('>')
        {
            let tag_inner = &input[i + 1..i + 1 + rel_end];
            // Only strip plausible HTML tags so math like `a < b > c` stays intact.
            let name = tag_inner
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric()) {
                i = i + 1 + rel_end + 1;
                continue;
            }
        }
        let ch = input[i..].chars().next().expect("valid utf-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn strip_math_and_latex(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_math = false;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            match next {
                b'$' | b'{' | b'}' | b'_' | b'%' | b'#' | b'&' => {
                    out.push(next as char);
                    i += 2;
                    continue;
                }
                b',' | b';' | b':' | b' ' => {
                    out.push(' ');
                    i += 2;
                    continue;
                }
                b'!' => {
                    i += 2;
                    continue;
                }
                b if b.is_ascii_alphabetic() => {
                    let cmd_start = i + 1;
                    let mut cmd_end = cmd_start;
                    while cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
                        cmd_end += 1;
                    }
                    let cmd = &input[cmd_start..cmd_end];
                    if let Some(replacement) = latex_symbol(cmd) {
                        out.push_str(replacement);
                        i = cmd_end;
                        continue;
                    }
                    if is_latex_text_wrapper(cmd)
                        && cmd_end < bytes.len()
                        && bytes[cmd_end] == b'{'
                        && let Some(close) = find_matching_brace(input, cmd_end + 1)
                    {
                        let inner = &input[cmd_end + 1..close];
                        out.push_str(&strip_math_and_latex(inner));
                        i = close + 1;
                        continue;
                    }
                    if cmd == "frac"
                        && cmd_end < bytes.len()
                        && bytes[cmd_end] == b'{'
                        && let Some(c1) = find_matching_brace(input, cmd_end + 1)
                        && c1 + 1 < bytes.len()
                        && bytes[c1 + 1] == b'{'
                        && let Some(c2) = find_matching_brace(input, c1 + 2)
                    {
                        let num = strip_math_and_latex(&input[cmd_end + 1..c1]);
                        let den = strip_math_and_latex(&input[c1 + 2..c2]);
                        out.push_str(&num);
                        out.push('/');
                        out.push_str(&den);
                        i = c2 + 1;
                        continue;
                    }
                    if matches!(cmd, "left" | "right" | "big" | "Big" | "bigg" | "Bigg") {
                        i = cmd_end;
                        continue;
                    }
                    // Preserve unknown backslash sequences outside math, drop backslash inside math.
                    if !in_math {
                        out.push('\\');
                    }
                    out.push_str(cmd);
                    i = cmd_end;
                    continue;
                }
                _ => {}
            }
        }
        if bytes[i] == b'$' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                i += 2;
            } else {
                i += 1;
            }
            in_math = !in_math;
            continue;
        }
        if in_math && (bytes[i] == b'{' || bytes[i] == b'}') {
            i += 1;
            continue;
        }
        let ch = input[i..].chars().next().expect("valid utf-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn is_latex_text_wrapper(cmd: &str) -> bool {
    matches!(
        cmd,
        "text"
            | "textrm"
            | "textbf"
            | "textit"
            | "texttt"
            | "mathrm"
            | "mathbf"
            | "mathit"
            | "mathsf"
            | "mathtt"
            | "mathcal"
            | "mathbb"
            | "boldsymbol"
            | "operatorname"
            | "bm"
    )
}

fn latex_symbol(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "times" => "×",
        "cdot" => "·",
        "pm" => "±",
        "div" => "÷",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "to" | "rightarrow" => "→",
        "leftarrow" => "←",
        "Rightarrow" => "⇒",
        "Leftrightarrow" => "⇔",
        "infty" => "∞",
        "in" => "∈",
        "notin" => "∉",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "theta" => "θ",
        "lambda" => "λ",
        "mu" => "μ",
        "pi" => "π",
        "sigma" => "σ",
        "omega" => "ω",
        "Delta" => "Δ",
        "Sigma" => "Σ",
        "Omega" => "Ω",
        "quad" | "qquad" => " ",
        "dots" | "ldots" | "cdots" => "…",
        _ => return None,
    })
}

fn strip_markdown_marks(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 1 < bytes.len()
            && matches!(bytes[i + 1], b'*' | b'_' | b'`' | b'~' | b'[' | b']')
        {
            out.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            i += 1;
            continue;
        }
        if i + 1 < bytes.len()
            && ((bytes[i] == b'*' && bytes[i + 1] == b'*')
                || (bytes[i] == b'_' && bytes[i + 1] == b'_')
                || (bytes[i] == b'~' && bytes[i + 1] == b'~'))
        {
            i += 2;
            continue;
        }
        let ch = input[i..].chars().next().expect("valid utf-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_space = true;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn outline_font_family() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Noto Sans SC"
    }
    #[cfg(target_os = "macos")]
    {
        "PingFang SC"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "sans-serif"
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

#[derive(Clone, Copy)]
struct OutlineColors {
    panel_bg: Rgba,
    panel_border: Rgba,
    header_text: Rgba,
    header_hover_text: Rgba,
    item_text_root: Rgba,
    item_text_nested: Rgba,
    item_text_hover: Rgba,
    item_text_active: Rgba,
    item_bg_active: Rgba,
    item_bg_hover: Rgba,
    item_bg_pressed: Rgba,
    accent_bar: Rgba,
    chevron: Rgba,
    chevron_hover: Rgba,
    guide_line: Rgba,
    scroll_thumb: Rgba,
    scroll_thumb_hover: Rgba,
    scroll_thumb_active: Rgba,
}

impl OutlineColors {
    fn current() -> Self {
        let palette = crate::theme::palette();
        let dark = palette.is_dark();
        let text = gpui_kit::rgb(palette.text);
        let muted = gpui_kit::rgb(palette.muted);
        let hover = gpui_kit::rgb(palette.hover);
        let pressed = gpui_kit::rgb(palette.pressed);
        let border = gpui_kit::rgb(palette.border);
        Self {
            panel_bg: palette.elevated_surface(),
            panel_border: border,
            header_text: muted.opacity(0.85),
            header_hover_text: text,
            item_text_root: text.opacity(if dark { 0.80 } else { 0.78 }),
            item_text_nested: muted,
            item_text_hover: text,
            item_text_active: text,
            item_bg_active: hover.opacity(if dark { 0.88 } else { 0.76 }),
            item_bg_hover: hover.opacity(if dark { 0.60 } else { 0.65 }),
            item_bg_pressed: pressed,
            accent_bar: gpui_kit::rgb(palette.accent),
            chevron: muted.opacity(0.58),
            chevron_hover: text,
            guide_line: border,
            scroll_thumb: palette.scroll_thumb(),
            scroll_thumb_hover: palette.scroll_thumb_hover(),
            scroll_thumb_active: palette.scroll_thumb_active(),
        }
    }
}

fn render_panel_body(
    pane: &MarkdownPane,
    overlay: bool,
    cx: &mut Context<MarkdownPane>,
) -> Stateful<Div> {
    let mut colors = OutlineColors::current();
    if pane.backdrop_visible {
        // Docked outline shares the document backdrop transparently; only the narrow-screen
        // overlay drawer keeps an opaque-ish surface so it stays legible over document text.
        colors.panel_bg = colors.panel_bg.opacity(0.90);
        colors.item_bg_active = colors.item_bg_active.opacity(0.55);
        colors.item_bg_hover = colors.item_bg_hover.opacity(0.45);
        colors.item_bg_pressed = colors.item_bg_pressed.opacity(0.6);
        colors.guide_line = colors.guide_line.opacity(0.55);
        colors.scroll_thumb = colors.scroll_thumb.opacity(0.6);
    }
    let tree = &pane.outline_tree;
    let all_parents = &pane.outline_parents;
    let has_any_parents = !all_parents.is_empty();
    let all_collapsed = has_any_parents
        && all_parents
            .iter()
            .all(|ix| pane.outline_collapsed.contains(ix));

    let header = div()
        .flex_shrink_0()
        .pt(if overlay { px(16.) } else { px(24.) })
        .pb(px(10.))
        .pl(px(16.))
        .pr(px(12.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .id("md-outline-header-toggle")
                .flex()
                .items_center()
                .gap(px(4.))
                .py(px(2.))
                .px(px(4.))
                .rounded(px(4.))
                .text_size(px(12.))
                .font_medium()
                .text_color(colors.header_text)
                .when(has_any_parents, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.text_color(colors.header_hover_text))
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            if all_collapsed {
                                pane.outline_collapsed.clear();
                            } else {
                                pane.outline_collapsed.clone_from(&pane.outline_parents);
                            }
                            cx.notify();
                        }))
                })
                .child(crate::i18n::markdown::OUTLINE_TITLE.get())
                .when(has_any_parents, |el| {
                    el.child(
                        crate::ui::icon(if all_collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .w(px(12.))
                        .h(px(12.)),
                    )
                }),
        );

    let list_content = if tree.is_empty() {
        div()
            .px_2()
            .py_3()
            .text_size(px(12.5))
            .text_color(colors.header_text)
            .child(crate::i18n::markdown::OUTLINE_EMPTY.get())
            .into_any_element()
    } else {
        div()
            .w_full()
            .min_w_0()
            .ml(px(4.))
            .pl(px(8.))
            .border_l_1()
            .border_color(colors.guide_line)
            .child(render_nodes(
                tree,
                0,
                pane.active_heading,
                &pane.outline_collapsed,
                colors,
                cx,
            ))
            .into_any_element()
    };

    let scroll_area = div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            div()
                .id("markdown-outline-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&pane.outline_scroll)
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .pl(px(16.))
                .pr(px(12.))
                .pb(px(28.))
                .child(list_content),
        )
        .child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(&pane.outline_scroll)
                    .mode(ScrollbarMode::Hover)
                    .styles(move |styles| {
                        styles
                            .thumb(move |style| {
                                style
                                    .bg(colors.scroll_thumb)
                                    .width(px(4.))
                                    .inset(px(2.))
                                    .radius(px(2.))
                                    .min_length(px(28.))
                            })
                            .thumb_hover(move |style| {
                                style
                                    .bg(colors.scroll_thumb_hover)
                                    .width(px(4.))
                                    .inset(px(2.))
                                    .radius(px(2.))
                            })
                            .thumb_active(move |style| {
                                style
                                    .bg(colors.scroll_thumb_active)
                                    .width(px(4.))
                                    .inset(px(2.))
                                    .radius(px(2.))
                            })
                    }),
            ),
        );

    div()
        .id("markdown-outline-panel")
        .w(px(PANEL_WIDTH))
        .h_full()
        .flex_shrink_0()
        .when(overlay, |panel| {
            panel
                .bg(colors.panel_bg)
                .border_r_1()
                .border_color(colors.panel_border)
                .shadow(vec![
                    BoxShadow::new(px(6.), px(0.), hsla(0., 0., 0., 0.08)).blur_radius(px(20.)),
                    BoxShadow::new(px(1.), px(0.), hsla(0., 0., 0., 0.04)).blur_radius(px(4.)),
                ])
        })
        .font_family(outline_font_family())
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .child(scroll_area)
}

fn render_nodes(
    nodes: &[OutlineNode],
    depth: usize,
    active_heading: Option<usize>,
    collapsed: &BTreeSet<usize>,
    colors: OutlineColors,
    cx: &mut Context<MarkdownPane>,
) -> Div {
    let any_has_children = nodes.iter().any(|n| !n.children.is_empty());
    let list_gap = if depth == 0 { px(4.) } else { px(2.) };
    let mut list = div().flex().flex_col().gap(list_gap).w_full().min_w_0();

    for node in nodes {
        let index = node.index;
        let has_children = !node.children.is_empty();
        let is_collapsed = has_children && collapsed.contains(&index);
        let is_direct_active = active_heading == Some(index);
        let is_collapsed_ancestor_active =
            is_collapsed && active_heading.is_some_and(|active_ix| node.contains_index(active_ix));
        let is_active = is_direct_active || is_collapsed_ancestor_active;

        let font_size = if depth == 0 { px(12.) } else { px(11.5) };
        let line_height = if depth == 0 { px(19.5) } else { px(18.5) };
        let row_py = if depth == 0 { px(5.) } else { px(4.) };
        let rest_color = if is_active {
            colors.item_text_active
        } else if depth == 0 {
            colors.item_text_root
        } else {
            colors.item_text_nested
        };
        let full_title = node.title.clone();

        let row = div()
            .id(("md-outline-item", index))
            .relative()
            .flex()
            .items_start()
            .gap(px(4.))
            .w_full()
            .min_w_0()
            .px(px(8.))
            .py(row_py)
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(font_size)
            .line_height(line_height)
            .text_color(rest_color)
            .when(is_active, |row| {
                row.bg(colors.item_bg_active).font_medium().child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(5.))
                        .bottom(px(5.))
                        .w(px(2.))
                        .rounded_full()
                        .bg(colors.accent_bar),
                )
            })
            .when(!is_active, |row| {
                row.hover(move |s| {
                    s.bg(colors.item_bg_hover)
                        .text_color(colors.item_text_hover)
                })
                .active(move |s| s.bg(colors.item_bg_pressed))
            })
            .when(any_has_children, |row| {
                if has_children {
                    row.child(
                        div()
                            .id(("md-outline-fold", index))
                            .w(px(14.))
                            .h(line_height)
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_color(colors.chevron)
                            .hover(move |s| s.text_color(colors.chevron_hover))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                cx.stop_propagation();
                                if !pane.outline_collapsed.remove(&index) {
                                    pane.outline_collapsed.insert(index);
                                }
                                cx.notify();
                            }))
                            .child(
                                crate::ui::icon(if is_collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .w(px(10.))
                                .h(px(10.)),
                            ),
                    )
                } else {
                    row.child(div().w(px(14.)).h(line_height).flex_shrink_0())
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(node.title.clone()),
            )
            .tooltip(move |window, cx| Tooltip::new(full_title.clone()).build(window, cx))
            .on_click(cx.listener(move |pane, _, window, cx| {
                pane.on_outline_heading_click(index, window, cx);
            }));

        list = list.child(row);

        if has_children && !is_collapsed {
            let guide_ml = if depth == 0 {
                px(11.)
            } else if depth < 3 {
                px(9.)
            } else {
                px(7.)
            };
            let guide_pl = if depth < 3 { px(8.) } else { px(6.) };
            let child_group = div()
                .ml(guide_ml)
                .pl(guide_pl)
                .mt(px(2.))
                .mb(px(4.))
                .border_l_1()
                .border_color(colors.guide_line)
                .child(render_nodes(
                    &node.children,
                    depth + 1,
                    active_heading,
                    collapsed,
                    colors,
                    cx,
                ));
            list = list.child(child_group);
        }
    }

    list
}

/// Render the docked left-side outline with smooth 200ms ease-out width + slide animation.
pub(super) fn render_docked(pane: &MarkdownPane, cx: &mut Context<MarkdownPane>) -> AnyElement {
    let panel = render_panel_body(pane, false, cx);
    let width = pane.docked_outline_panel_width();

    // Initial wide-screen auto-open renders statically without sliding in on document load.
    if !pane.outline_closing && pane.outline_anim_gen == 0 {
        return div()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .child(panel)
            .into_any_element();
    }

    let closing = pane.outline_closing;
    let anim_id = ("md-outline-dock-anim", pane.outline_anim_gen);
    let inner_anim_id = ("md-outline-dock-slide", pane.outline_anim_gen);
    let duration = Duration::from_millis(ANIMATION_MS);

    let sliding_inner = div()
        .relative()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .child(panel)
        .with_animation(
            inner_anim_id,
            Animation::new(duration).with_easing(ease_out_cubic),
            move |el, t| {
                let progress = if closing { 1.0 - t } else { t };
                el.left(px(-width * (1.0 - progress)))
            },
        );

    div()
        .h_full()
        .flex_shrink_0()
        .overflow_hidden()
        .child(sliding_inner)
        .with_animation(
            anim_id,
            Animation::new(duration).with_easing(ease_out_cubic),
            move |el, t| {
                let progress = if closing { 1.0 - t } else { t };
                el.w(px(width * progress))
            },
        )
        .into_any_element()
}

/// Render the narrow-screen left overlay drawer with backdrop click-outside dismissal.
pub(super) fn render_overlay(pane: &MarkdownPane, cx: &mut Context<MarkdownPane>) -> AnyElement {
    let closing = pane.outline_closing;
    let anim_gen = pane.outline_anim_gen;
    let duration = Duration::from_millis(ANIMATION_MS);
    let panel = render_panel_body(pane, true, cx);

    let scrim = if crate::theme::palette().is_dark() {
        rgba(0x07090d70)
    } else {
        rgba(0x16161622)
    };
    let backdrop = div()
        .id("md-outline-overlay-backdrop")
        .absolute()
        .inset_0()
        .bg(scrim)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|pane, _, _, cx| {
                pane.close_outline(cx);
                cx.stop_propagation();
            }),
        )
        .with_animation(
            ("md-outline-backdrop-anim", anim_gen),
            Animation::new(duration).with_easing(ease_out_cubic),
            move |el, t| {
                let progress = if closing { 1.0 - t } else { t };
                el.opacity(progress)
            },
        );

    let drawer = div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(PANEL_WIDTH))
        .child(panel)
        .with_animation(
            ("md-outline-overlay-slide", anim_gen),
            Animation::new(duration).with_easing(ease_out_cubic),
            move |el, t| {
                let progress = if closing { 1.0 - t } else { t };
                el.left(px(-PANEL_WIDTH * (1.0 - progress)))
            },
        );

    div()
        .id("md-outline-overlay-layer")
        .absolute()
        .inset_0()
        .overflow_hidden()
        .child(backdrop)
        .child(drawer)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{OutlineHeading, build_tree, clean_outline_title, parent_indices};

    #[test]
    fn cleans_latex_markdown_and_keeps_numbering() {
        assert_eq!(
            clean_outline_title(r#"例子 1: $P = \text{"abaabcac"}$"#),
            r#"例子 1: P = "abaabcac""#
        );
        assert_eq!(
            clean_outline_title("一、底层逻辑：30秒讲透 $O(m+n)$"),
            "一、底层逻辑：30秒讲透 O(m+n)"
        );
        assert_eq!(
            clean_outline_title("为什么 `next[j]` 会退回"),
            "为什么 next[j] 会退回"
        );
        assert_eq!(
            clean_outline_title("5.1 **标准 KMP** 与 [真题解析](https://example.com/a(1))"),
            "5.1 标准 KMP 与 真题解析"
        );
        assert_eq!(
            clean_outline_title(r"当 $a < b$ 且 $x \le y \times 2$ 时"),
            "当 a < b 且 x ≤ y × 2 时"
        );
        assert_eq!(
            clean_outline_title("6.2 <mark>重要</mark>：__易错点__"),
            "6.2 重要：易错点"
        );
    }

    #[test]
    fn builds_nested_outline_tree_correctly() {
        let headings = vec![
            OutlineHeading {
                range: 0..1,
                line: 0,
                level: 2,
                title: "1. Intro".into(),
                display_title: "1. Intro".into(),
            },
            OutlineHeading {
                range: 2..3,
                line: 1,
                level: 2,
                title: "2. Core".into(),
                display_title: "2. Core".into(),
            },
            OutlineHeading {
                range: 4..5,
                line: 2,
                level: 3,
                title: "2.1 Sub A".into(),
                display_title: "2.1 Sub A".into(),
            },
            OutlineHeading {
                range: 6..7,
                line: 3,
                level: 4,
                title: "2.1.1 Deep".into(),
                display_title: "2.1.1 Deep".into(),
            },
            OutlineHeading {
                range: 8..9,
                line: 4,
                level: 3,
                title: "2.2 Sub B".into(),
                display_title: "2.2 Sub B".into(),
            },
            OutlineHeading {
                range: 10..11,
                line: 5,
                level: 2,
                title: "3. End".into(),
                display_title: "3. End".into(),
            },
        ];
        let tree = build_tree(&headings);
        assert_eq!(tree.len(), 3);
        assert!(tree[0].children.is_empty());
        assert_eq!(tree[1].children.len(), 2);
        assert_eq!(tree[1].children[0].children.len(), 1);
        assert_eq!(tree[1].children[0].children[0].title.as_ref(), "2.1.1 Deep");
        assert!(tree[2].children.is_empty());

        let parents = parent_indices(&tree);
        assert!(parents.contains(&1));
        assert!(parents.contains(&2));
        assert!(!parents.contains(&0));
    }
}
