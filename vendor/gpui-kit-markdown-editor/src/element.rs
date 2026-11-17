//! `BlockEditor` view (design §7).
//!
//! `render` takes the cached projection and renders it with gpui-kit's own
//! `BlockNode` renderer, so the editor looks exactly like `TextView`. The
//! [`EditorRoot`] element installs a frame-local [`FrameSink`] while its
//! children prepaint: keyed text leaves report their layouts into it and take
//! their selection backgrounds from it (`text::editor_seam`, D3). The root
//! then paints the caret and the composition underline from that geometry.
//! Paint never updates the editor entity.

use std::{cell::Cell, collections::HashMap, ops::Range, rc::Rc};
use std::sync::{Arc, Mutex};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, CursorStyle, DefiniteLength, DispatchPhase, Edges, Element, ElementId,
    ElementInputHandler, Entity, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Refineable, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Style, StyleRefinement, Styled, TextStyle, Window,
    canvas, div, fill, point, px, size,
};

use super::{
    code_preview::{self, BoundsMap},
    cull::Measured,
    id::BlockId,
    projection::Snapshot,
    state::*,
    style::BlockEditorStyle,
};
use crate::support::secondary_modifier;
use gpui_kit::{
    base::{
        StyledExt as _,
        text::{
            MarkdownExtensions,
            editor_host::{BlockNode, BlockState, FlowPosition, collect_leaf_geometry, RenderContext},
        },
    },
    component::{Icon, IconName},
};

type AtomBounds = Arc<Mutex<HashMap<BlockId, Bounds<Pixels>>>>;

fn noop_surface() -> BlockNode {
    BlockNode::Surface {
        name: "".into(),
        children: Vec::new(),
        span: None,
    }
}

fn capitalize_alert_name(name: &str) -> SharedString {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = String::with_capacity(part.len());
                    out.extend(first.to_uppercase());
                    out.push_str(&chars.as_str().to_ascii_lowercase());
                    out
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

/// Add measuring / fold / code-preview surfaces to the view without mutating
/// the document model.
fn with_live_surfaces(
    node: &BlockNode,
    id: BlockId,
    state: &BlockEditorState,
    editor: &Entity<BlockEditorState>,
    include_atoms: bool,
    force_preview: bool,
    preview_bounds: Option<&BoundsMap>,
) -> BlockNode {
    let doc = &state.model.doc;
    if include_atoms
        && matches!(
            doc.get(id).map(|entry| &entry.content),
            Some(super::document::BlockContent::Atom(_))
        )
    {
        return BlockNode::Surface {
            name: format!("editor-atom-{}", id.as_u64()).into(),
            children: vec![node.clone()],
            span: None,
        };
    }
    if let BlockNode::CodeBlock(code) = node {
        let preview = state
            .code_preview_registration()
            .filter(|registration| {
                code.lang()
                    .is_some_and(|language| language.eq_ignore_ascii_case(&registration.language))
            });
        if force_preview || state.code_preview_source() != Some(id) {
            if let Some(registration) = preview {
                return BlockNode::Custom {
                    node: code_preview::node(
                        editor.clone(),
                        id,
                        doc.block_revision(id),
                        code.clone(),
                        registration.renderer,
                        preview_bounds.cloned(),
                        !force_preview,
                    ),
                    state: BlockState::default(),
                };
            }
        }
        if !force_preview {
            return BlockNode::Surface {
                name: format!("editor-code-source-{}", id.as_u64()).into(),
                children: vec![node.clone()],
                span: None,
            };
        }
    }
    if matches!(doc.kind(id), Some(super::kind::BlockKindData::Fold { .. })) {
        if let BlockNode::Root { children, .. } = node {
            let doc_children = doc.children(id);
            if let (Some(title_node), Some(&title_id)) = (children.first(), doc_children.first()) {
                let title_is_empty = doc.inline(title_id).is_some_and(|text| text.is_empty());
                let mut title_children = Vec::with_capacity(2);
                if title_is_empty {
                    title_children.push(noop_surface());
                }
                title_children.push(with_live_surfaces(
                    title_node,
                    title_id,
                    state,
                    editor,
                    include_atoms,
                    force_preview,
                    preview_bounds,
                ));
                let title_surface = BlockNode::Surface {
                    name: format!("editor-fold-title-{}", id.as_u64()).into(),
                    children: title_children,
                    span: None,
                };
                let header_surface = BlockNode::Surface {
                    name: format!("editor-fold-header-{}", id.as_u64()).into(),
                    children: vec![title_surface],
                    span: None,
                };
                let mut fold_children = vec![header_surface];
                if force_preview || state.is_fold_open(id) {
                    for (child, child_id) in children[1..].iter().zip(&doc_children[1..]) {
                        fold_children.push(with_live_surfaces(
                            child,
                            *child_id,
                            state,
                            editor,
                            include_atoms,
                            force_preview,
                            preview_bounds,
                        ));
                    }
                }
                return BlockNode::Surface {
                    name: format!("editor-fold-{}", id.as_u64()).into(),
                    children: fold_children,
                    span: None,
                };
            }
        }
    }
    let mut node = node.clone();
    match &mut node {
        BlockNode::Root { children, .. }
        | BlockNode::Blockquote { children, .. }
        | BlockNode::Surface { children, .. }
        | BlockNode::List { children, .. }
        | BlockNode::ListItem { children, .. } => {
            for (child, id) in children.iter_mut().zip(doc.children(id)) {
                *child = with_live_surfaces(
                    child,
                    *id,
                    state,
                    editor,
                    include_atoms,
                    force_preview,
                    preview_bounds,
                );
            }
        }
        _ => {}
    }
    node
}

fn register_fold_surfaces(
    extensions: &mut MarkdownExtensions,
    state: &BlockEditorState,
    fold_bounds: Option<&AtomBounds>,
    theme: &gpui_kit::base::Theme,
) {
    let accent = theme.tokens.colors.primary;
    let border = theme.tokens.colors.border;
    let muted = theme.tokens.colors.muted;
    let is_dark = theme.tokens.colors.background.l < 0.5;

    for (&id, entry) in &state.model.doc.blocks {
        let super::kind::BlockKindData::Fold { syntax, .. } = &entry.kind else {
            continue;
        };
        let open = state.is_fold_open(id);
        let has_body = state.model.doc.children(id).len() > 1;
        let fold_bounds = fold_bounds.cloned();

        match syntax {
            super::kind::FoldSyntax::Details { .. } => {
                let card_bg = muted.opacity(if is_dark { 0.45 } else { 0.55 });
                *extensions = std::mem::take(extensions)
                    .block_surface(format!("editor-fold-{}", id.as_u64()), move |_, _| {
                        div()
                            .relative()
                            .overflow_hidden()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(border)
                            .bg(card_bg)
                            .pl(px(16.))
                            .pr(px(14.))
                            .py(px(10.))
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(3.))
                                    .bg(accent),
                            )
                    })
                    .block_surface(format!("editor-fold-header-{}", id.as_u64()), move |_, _| {
                        let toggle = div()
                            .relative()
                            .w(px(20.))
                            .h(px(22.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .bg(accent.opacity(0.10))
                            .hover(|s| s.bg(accent.opacity(0.20)))
                            .cursor_pointer()
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(13.))
                                .text_color(accent),
                            )
                            .when_some(fold_bounds.clone(), |this, bounds| {
                                this.child(
                                    canvas(
                                        move |area, _, _| {
                                            bounds.lock().expect("fold bounds").insert(id, area);
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            });
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(8.))
                            .when(open && has_body, |this| {
                                this.pb(px(8.))
                                    .border_b_1()
                                    .border_color(border.opacity(0.55))
                            })
                            .child(toggle)
                    })
                    .block_surface(format!("editor-fold-title-{}", id.as_u64()), move |_, _| {
                        div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM)
                    });
            }
            super::kind::FoldSyntax::Alert { name } => {
                let key = name.to_ascii_lowercase();
                let (badge_title, color) = state
                    .callout_styles
                    .get(key.as_str())
                    .map(|style| (style.title.clone(), style.color))
                    .unwrap_or_else(|| (capitalize_alert_name(name), accent));
                *extensions = std::mem::take(extensions)
                    .block_surface(format!("editor-fold-{}", id.as_u64()), move |_, _| {
                        super::style::callout_container(color)
                    })
                    .block_surface(format!("editor-fold-header-{}", id.as_u64()), move |_, _| {
                        let trigger = div()
                            .relative()
                            .flex_shrink_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(4.))
                            .px(px(4.))
                            .ml(px(-4.))
                            .py(px(1.))
                            .hover(|s| s.bg(color.opacity(0.10)))
                            .cursor_pointer()
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(13.))
                                .text_color(color),
                            )
                            .child(super::style::callout_header(badge_title.clone(), color))
                            .when_some(fold_bounds.clone(), |this, bounds| {
                                this.child(
                                    canvas(
                                        move |area, _, _| {
                                            bounds.lock().expect("fold bounds").insert(id, area);
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            });
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .when(open && has_body, |this| this.pb(px(6.)))
                            .child(trigger)
                    })
                    .block_surface(format!("editor-fold-title-{}", id.as_u64()), move |_, _| {
                        div().flex_1().min_w_0()
                    });
            }
        }
    }
}
fn register_code_surfaces(
    extensions: &mut MarkdownExtensions,
    editor: &Entity<BlockEditorState>,
    context: &RenderContext,
    cx: &App,
) {
    let ids: Vec<BlockId> = editor
        .read(cx)
        .model
        .doc
        .blocks
        .iter()
        .filter_map(|(&id, entry)| {
            matches!(&entry.kind, super::kind::BlockKindData::CodeBlock { .. }).then_some(id)
        })
        .collect();
    for id in ids {
        let editor = editor.clone();
        let context = context.clone();
        *extensions = std::mem::take(extensions).block_surface(
            format!("editor-code-source-{}", id.as_u64()),
            move |window, cx| {
                super::code_ui::container(&context)
                    .child(super::code_ui::header(&editor, id, window, cx))
            },
        );
    }
}



/// Items per cull unit of a long top-level list.
const LIST_CHUNK: usize = 32;

const CONTAINER_EXTRA_GAP: Pixels = px(6.);

fn is_container_node(node: &BlockNode) -> bool {
    match node {
        BlockNode::Surface { .. }
        | BlockNode::Table(_)
        | BlockNode::CodeBlock(_)
        | BlockNode::Blockquote { .. } => true,
        BlockNode::Custom { node, .. } => node.name() == code_preview::NAME,
        _ => false,
    }
}

fn is_invisible_node(node: &BlockNode) -> bool {
    matches!(node, BlockNode::Break { .. } | BlockNode::Definition { .. })
}

fn container_extra_gap(
    curr_is_container: bool,
    curr_is_heading: bool,
    next_is_container: bool,
    next_is_heading: bool,
) -> Pixels {
    if (curr_is_container && !next_is_heading) || (!curr_is_heading && next_is_container) {
        CONTAINER_EXTRA_GAP
    } else {
        px(0.)
    }
}

/// A block-based WYSIWYG Markdown editor (spike). Hold an
/// `Entity<BlockEditorState>` and pass it here in `render`. Instance styling
/// applies to the native input root; it installs no padding or height defaults.
#[derive(IntoElement)]
pub struct BlockEditor {
    state: Entity<BlockEditorState>,
    style: StyleRefinement,
}

impl BlockEditor {
    /// Use the editor's snapshot and node renderer without selection, caret or viewport culling.
    pub fn document(state: &Entity<BlockEditorState>, window: &mut Window, cx: &mut App) -> AnyElement {
        let (snapshot, extensions) = state.update(cx, |state, _| {
            (state.snapshot(), state.model.doc.extensions.clone())
        });
        let mut extensions = (*extensions).clone();
        code_preview::register_renderer(&mut extensions);
        let theme = gpui_kit::base::Theme::global(cx);
        let editor_state = state.read(cx);
        register_fold_surfaces(&mut extensions, editor_state, None, &theme);
        let context = editor_state.render_context(cx)
            .with_markdown_extensions(Arc::new(extensions))
            .with_body_font_size(window.text_style().font_size.to_pixels(window.rem_size()));
        let nodes: Vec<BlockNode> = snapshot
            .blocks
            .iter()
            .flat_map(|block| {
                block
                    .nodes
                    .iter()
                    .map(|node| with_live_surfaces(
                        node,
                        block.id,
                        editor_state,
                        state,
                        false,
                        true,
                        None,
                    ))
            })
            .collect();
        let node_refs: Vec<&BlockNode> = nodes.iter().collect();
        let mut children = Vec::with_capacity(node_refs.len());
        for (ix, node) in node_refs.iter().enumerate() {
            let extra_gap = if is_invisible_node(node) {
                px(0.)
            } else if let Some(next) = node_refs[ix + 1..].iter().find(|n| !is_invisible_node(n)) {
                container_extra_gap(
                    is_container_node(node),
                    matches!(node, BlockNode::Heading { .. }),
                    is_container_node(next),
                    matches!(next, BlockNode::Heading { .. }),
                )
            } else {
                px(0.)
            };
            let rendered = node.render(ix, FlowPosition::at(&node_refs, ix, false).last(ix + 1 == node_refs.len()), &context, window, cx);
            let block_el = div()
                .relative()
                .child(rendered)
                .child(canvas(|_,_,_| (), |bounds,_,window,_| window.vector_block(bounds)).absolute().size_full())
                .into_any_element();
            children.push(if extra_gap > px(0.) {
                div().w_full().pb(extra_gap).child(block_el).into_any_element()
            } else {
                block_el
            });
        }
        div().w_full().children(children).into_any_element()
    }

    pub fn new(state: &Entity<BlockEditorState>) -> Self {
        Self {
            state: state.clone(),
            style: StyleRefinement::default(),
        }
    }
}

// Presentation is applied inside EditorRoot so padding is part of its
// native hitbox and input bounds, not an unfocusable outer wrapper.
impl Styled for BlockEditor {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The accessible role (and heading level) of a top-level block (Phase 5,
/// per-block accessibility). `None` for blocks that are not exposed on
/// their own (definitions, breaks).
pub(crate) fn block_role(node: &BlockNode) -> Option<(Role, Option<usize>)> {
    Some(match node {
        BlockNode::Paragraph(_) => (Role::Paragraph, None),
        BlockNode::Heading { level, .. } => (Role::Heading, Some(*level as usize)),
        BlockNode::Blockquote { .. } => (Role::Blockquote, None),
        BlockNode::Surface { .. } => (Role::Blockquote, None),
        BlockNode::List { .. } => (Role::List, None),
        BlockNode::CodeBlock(_) => (Role::Code, None),
        BlockNode::Table(_) => (Role::Table, None),
        BlockNode::HorizontalRule { .. } => (Role::Splitter, None),
        BlockNode::Custom { .. } => (Role::Group, None),
        BlockNode::ListItem { .. } => (Role::ListItem, None),
        BlockNode::Root { .. }
        | BlockNode::Break { .. }
        | BlockNode::Definition { .. }
        | BlockNode::Unknown => return None,
        _ => return None,
    })
}

impl RenderOnce for BlockEditor {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = gpui_kit::base::Theme::global(cx);
        // Links are followed by the editor itself (Cmd/Ctrl-click); the shared
        // renderer must never open a URL on its own.
        let render_cx = self.state.read(cx).render_context(cx);
        let text_style = render_cx.style().clone();
        let mut style = self.state.read(cx).style.resolved(&theme.tokens);
        if style.selection_is_default() {
            style = style.with_selection(text_style.selection());
        }
        // Read the prior frame before request_layout can reset shared text
        // layouts. Manual scrolling must not become a reflow reveal request.
        let caret_was_visible = {
            let state = self.state.read(cx);
            state
                .scroll_handle
                .as_ref()
                .zip(state.model.caret())
                .is_some_and(|(handle, pos)| {
                    state
                        .frame
                        .borrow()
                        .caret_bounds(pos, style.caret_width())
                        .is_some_and(|caret| handle.bounds().contains(&caret.center()))
                })
        };
        let (focus, snapshot, backgrounds, extensions, keep, list_items) =
            self.state.update(cx, |state, _| {
                let snapshot = state.snapshot();
                let mut backgrounds: HashMap<u64, Vec<(Range<usize>, Hsla)>> = HashMap::new();
                for (block, range) in state.selection_ranges() {
                    // Atoms paint whole (O(1) lookup, not a scan of the leaves).
                    let range = match state.model.doc.get(block).map(|e| &e.content) {
                        Some(super::document::BlockContent::Atom(_)) => {
                            0..state.model.doc.leaf_len(block).unwrap_or(range.end)
                        }
                        _ => range,
                    };
                    backgrounds
                        .entry(block.as_u64())
                        .or_default()
                        .push((range, style.selection()));
                }
                (
                    state.focus_handle.clone(),
                    snapshot,
                    backgrounds,
                    state.model.doc.extensions.clone(),
                    state.pinned_blocks(),
                    // Only needed to rebuild the cached cull units.
                    (!state.cull.borrow().index.is_current(state.model.revision())).then(|| {
                        let doc = &state.model.doc;
                        doc.children(doc.root)
                            .iter()
                            .filter(|id| {
                                matches!(
                                    doc.kind(**id),
                                    Some(super::kind::BlockKindData::List { .. })
                                ) && doc.children(**id).len() > LIST_CHUNK
                            })
                            .map(|id| (*id, Rc::<[BlockId]>::from(doc.children(*id))))
                            .collect::<HashMap<_, _>>()
                    }),
                )
            });

        // Match the native root's effective inherited/instance typography
        // before deriving body-relative node metrics and culling geometry.
        let mut typography = window.text_style();
        typography.refine(&self.style.text);
        let atom_bounds: AtomBounds = Arc::default();
        let fold_bounds: AtomBounds = Arc::default();
        let code_preview_bounds: BoundsMap = Arc::default();
        let mut extensions = (*extensions).clone();
        code_preview::register_renderer(&mut extensions);
        register_code_surfaces(&mut extensions, &self.state, &render_cx, cx);
        for leaf in snapshot.leaves.iter().filter(|leaf| leaf.atomic) {
            let id = leaf.block;
            let bounds = atom_bounds.clone();
            extensions = extensions.block_surface(format!("editor-atom-{}", id.as_u64()), move |_, _| {
                let bounds = bounds.clone();
                div().relative().min_h(px(20.)).child(
                    canvas(move |area, _, _| { bounds.lock().expect("atom bounds").insert(id, area); }, |_, _, _, _| {})
                        .absolute().top_0().left_0().size_full(),
                )
            });
        }
        register_fold_surfaces(&mut extensions, self.state.read(cx), Some(&fold_bounds), &theme);
        // The outer surface paints the shared header and code background.
        let code_body_style = text_style.code_block().clone()
            .bg(gpui::transparent_black())
            .border_0();
        let node_cx = render_cx.clone()
            .with_style(text_style.clone().with_code_block(code_body_style))
            .with_markdown_extensions(Arc::new(extensions))
            .with_body_font_size(typography.font_size.to_pixels(window.rem_size()));
        let (cull, revision) = {
            let state = self.state.read(cx);
            (state.cull.clone(), state.model.revision())
        };
        let mut children: Vec<AnyElement> = Vec::new();
        {
            let mut cull_state = cull.borrow_mut();
            cull_state.measured.clear();
            let window_span = cull_state.visible.map(|(top, bottom)| {
                // Overscan two viewports each way, so a fast scroll stays
                // within laid-out blocks until the next frame catches up.
                let margin = ((bottom - top) * 2.).max(px(600.));
                (top - margin, bottom + margin)
            });
            let cull_state = &mut *cull_state;
            if let Some(list_items) = list_items {
                // Cull units: each top-level node, except that a long
                // top-level list is split into chunks of `LIST_CHUNK` items,
                // each rendered as a list of its own (continuing the
                // numbering, no gap between chunks). Rebuilt per revision.
                let nodes: Vec<&BlockNode> = snapshot
                    .blocks
                    .iter()
                    .flat_map(|b| b.nodes.iter())
                    .collect();
                let mut units = Vec::with_capacity(nodes.len());
                let mut node_ix = 0;
                for (block_ix, block) in snapshot.blocks.iter().enumerate() {
                    for (node_in_block, node) in block.nodes.iter().enumerate() {
                        let flow = FlowPosition::at(&nodes, node_ix, false);
                        let unit = |id, items| super::cull::UnitDesc {
                            id,
                            block: block_ix,
                            node: node_in_block,
                            node_ix,
                            items,
                            flow,
                        };
                        match (node, list_items.get(&block.id)) {
                            (BlockNode::List { children, .. }, Some(ids))
                                if ids.len() == children.len() =>
                            {
                                for start in (0..ids.len()).step_by(LIST_CHUNK) {
                                    let range = start..(start + LIST_CHUNK).min(ids.len());
                                    units.push(unit(ids[start], Some((range, ids.clone()))));
                                }
                            }
                            _ => units.push(unit(block.id, None)),
                        }
                        node_ix += 1;
                    }
                }
                cull_state.index.rebuild(
                    revision,
                    units.iter().map(|u| {
                        let covered: &[BlockId] = match &u.items {
                            Some((range, ids)) => &ids[range.clone()],
                            None => &[],
                        };
                        (u.id, covered)
                    }),
                    &cull_state.heights,
                );
                cull_state.units = units;
                cull_state.node_count = nodes.len();
            }
            let units = &cull_state.units;
            let count = cull_state.node_count;
            let index = &cull_state.index;
            let n = units.len();
            // Units to lay out: the visible range (binary search over the
            // prefix sums) plus pinned units (selection ends, IME).
            let (first, last) = match window_span {
                // Before prepaint provides the real viewport, measure only a
                // few screens. Measuring the whole document here defeats
                // virtualization precisely on the first, uncached frame.
                None => (
                    0,
                    (index.find(window.viewport_size().height * 3.) + 1).min(n),
                ),
                Some((top, bottom)) => {
                    let first = index.find(top.max(px(0.))).min(n);
                    (first, (index.find(bottom) + 1).min(n).max(first))
                }
            };
            let mut wanted: Vec<usize> = keep
                .iter()
                .filter_map(|id| index.unit_of.get(id).copied())
                .filter(|ix| *ix < first || *ix >= last)
                .collect();
            wanted.extend(first..last);
            wanted.sort_unstable();
            wanted.dedup();
            let mut cursor = px(0.);
            let mut rendered = 0;
            for unit_ix in wanted {
                let unit = &units[unit_ix];
                let node_ix = unit.node_ix;
                let top = index.prefix(unit_ix);
                if top > cursor {
                    children.push(div().h(top - cursor).into_any_element());
                }
                cursor = top + index.height(unit_ix);
                rendered += 1;
                let flow = unit.flow;
                let is_last_node = node_ix + 1 == count;
                let node = &snapshot.blocks[unit.block].nodes[unit.node];
                let live_node = if unit.items.is_none() {
                    with_live_surfaces(
                        node,
                        unit.id,
                        self.state.read(cx),
                        &self.state,
                        true,
                        false,
                        Some(&code_preview_bounds),
                    )
                } else {
                    node.clone()
                };
                let element = match (&unit.items, &live_node) {
                    (
                        Some((range, ids)),
                        BlockNode::List {
                            children: items,
                            ordered,
                            start,
                            ..
                        },
                    ) => {
                        let last_chunk = range.end == ids.len();
                        let chunk = BlockNode::List {
                            children: items[range.clone()].iter().zip(&ids[range.clone()])
                                .map(|(node, id)| with_live_surfaces(
                                    node,
                                    *id,
                                    self.state.read(cx),
                                    &self.state,
                                    true,
                                    false,
                                    Some(&code_preview_bounds),
                                ))
                                .collect(),
                            ordered: *ordered,
                            start: if *ordered {
                                Some(start.unwrap_or(1) + range.start as u32)
                            } else {
                                *start
                            },
                            span: None,
                        };
                        let flow = if range.start == 0 {
                            flow
                        } else {
                            flow.first(false)
                        };
                        chunk.render(
                            node_ix,
                            // Inner chunks drop the list's bottom gap.
                            flow.last(!last_chunk || is_last_node),
                            &node_cx,
                            window,
                            cx,
                        )
                    }
                    (_, BlockNode::CodeBlock(_)) => {
                        let code = node.render(node_ix, flow.last(is_last_node), &node_cx, window, cx);
                        super::code_ui::container(&render_cx)
                            .child(super::code_ui::header(&self.state, unit.id, window, cx))
                            .child(code)
                            .into_any_element()
                    }
                    (_, BlockNode::Table(_)) => {
                        let table = live_node.render(node_ix, flow.last(is_last_node), &node_cx, window, cx);
                        div()
                            .relative()
                            .w_full()
                            .child(table)
                            .children(super::table_ui::copy_controls(
                                &self.state,
                                unit.id,
                                window,
                                cx,
                            ))
                            .into_any_element()
                    }
                    _ => live_node.render(node_ix, flow.last(is_last_node), &node_cx, window, cx),
                };
                // Per-block accessibility node around the rendered block.
                let element = match block_role(node) {
                    Some((role, level)) => div()
                        .id(("block", unit.id.as_u64() as usize))
                        .role(role)
                        .when_some(level, |this, level| this.aria_level(level))
                        .child(element)
                        .into_any_element(),
                    None => element,
                };
                let is_inner_list_chunk = unit
                    .items
                    .as_ref()
                    .is_some_and(|(range, ids)| range.end < ids.len());
                let extra_gap = if is_inner_list_chunk || is_invisible_node(&live_node) {
                    px(0.)
                } else if let Some(next_unit) = units[unit_ix + 1..].iter().find(|u| {
                    !is_invisible_node(&snapshot.blocks[u.block].nodes[u.node])
                }) {
                    let next_block = &snapshot.blocks[next_unit.block];
                    let next_node = &next_block.nodes[next_unit.node];
                    let next_is_fold = matches!(
                        self.state.read(cx).model.doc.kind(next_block.id),
                        Some(super::kind::BlockKindData::Fold { .. })
                    );
                    container_extra_gap(
                        is_container_node(&live_node),
                        matches!(live_node, BlockNode::Heading { .. }),
                        is_container_node(next_node) || next_is_fold,
                        matches!(next_node, BlockNode::Heading { .. }),
                    )
                } else {
                    px(0.)
                };
                let element = if extra_gap > px(0.) {
                    div().w_full().pb(extra_gap).child(element).into_any_element()
                } else {
                    element
                };
                children.push(
                    Measured {
                        id: unit.id,
                        index: unit_ix,
                        state: cull.clone(),
                        child: Some(element),
                    }
                    .into_any_element(),
                );
            }
            let total = index.prefix(n);
            if total > cursor {
                children.push(div().h(total - cursor).into_any_element());
            }
            cull_state.rendered = rendered;
        }

        let content_bounds = Rc::new(Cell::new(Bounds::default()));
        let measured_content = content_bounds.clone();
        let mut root_style = Style::default();
        root_style.refine(&self.style);
        let s = &self.state;
        div()
            .id("block-editor")
            // D8: one multi-line text input over the document; the value is
            // built only while accessibility is active (cached per revision).
            .role(Role::MultilineTextInput)
            .when(window.is_a11y_active() || cfg!(test), |this| {
                this.aria_value(self.state.read(cx).accessible_value())
            })
            .key_context(self.state.read(cx).key_context())
            .track_focus(&focus)
            .cursor(if self.state.read(cx).grip_hover {
                CursorStyle::OpenHand
            } else if self.state.read(cx).pointer_hand {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            })
            .w_full()
            .on_action(window.listener_for(s, BlockEditorState::on_backspace))
            .on_action(window.listener_for(s, BlockEditorState::on_delete))
            .on_action(window.listener_for(s, BlockEditorState::on_delete_word_left))
            .on_action(window.listener_for(s, BlockEditorState::on_delete_word_right))
            .on_action(window.listener_for(s, BlockEditorState::on_delete_to_line_start))
            .on_action(window.listener_for(s, BlockEditorState::on_delete_to_line_end))
            .on_action(window.listener_for(s, BlockEditorState::on_move_paragraph_start))
            .on_action(window.listener_for(s, BlockEditorState::on_move_paragraph_end))
            .on_action(window.listener_for(s, BlockEditorState::on_select_paragraph_start))
            .on_action(window.listener_for(s, BlockEditorState::on_select_paragraph_end))
            .on_action(window.listener_for(s, BlockEditorState::on_enter))
            .on_action(window.listener_for(s, BlockEditorState::on_structural_enter))
            .on_action(window.listener_for(s, BlockEditorState::on_soft_break))
            .on_action(window.listener_for(s, BlockEditorState::on_exit_block))
            .on_action(window.listener_for(s, BlockEditorState::on_exit_code_block_source))
            .on_action(window.listener_for(s, BlockEditorState::on_move_left))
            .on_action(window.listener_for(s, BlockEditorState::on_move_right))
            .on_action(window.listener_for(s, BlockEditorState::on_move_up))
            .on_action(window.listener_for(s, BlockEditorState::on_move_down))
            .on_action(window.listener_for(s, BlockEditorState::on_move_home))
            .on_action(window.listener_for(s, BlockEditorState::on_move_end))
            .on_action(window.listener_for(s, BlockEditorState::on_select_left))
            .on_action(window.listener_for(s, BlockEditorState::on_move_word_left))
            .on_action(window.listener_for(s, BlockEditorState::on_move_word_right))
            .on_action(window.listener_for(s, BlockEditorState::on_select_word_left))
            .on_action(window.listener_for(s, BlockEditorState::on_select_word_right))
            .on_action(window.listener_for(s, BlockEditorState::on_select_right))
            .on_action(window.listener_for(s, BlockEditorState::on_select_up))
            .on_action(window.listener_for(s, BlockEditorState::on_select_down))
            .on_action(window.listener_for(s, BlockEditorState::on_select_home))
            .on_action(window.listener_for(s, BlockEditorState::on_select_end))
            .on_action(window.listener_for(s, BlockEditorState::on_move_doc_start))
            .on_action(window.listener_for(s, BlockEditorState::on_move_doc_end))
            .on_action(window.listener_for(s, BlockEditorState::on_select_doc_start))
            .on_action(window.listener_for(s, BlockEditorState::on_select_doc_end))
            .on_action(window.listener_for(s, BlockEditorState::on_select_all))
            .on_action(window.listener_for(s, BlockEditorState::on_undo))
            .on_action(window.listener_for(s, BlockEditorState::on_redo))
            .on_action(window.listener_for(s, BlockEditorState::on_normal_text))
            .on_action(window.listener_for(s, BlockEditorState::on_table_insert_column_left))
            .on_action(window.listener_for(s, BlockEditorState::on_table_insert_column_right))
            .on_action(window.listener_for(s, BlockEditorState::on_table_insert_row_above))
            .on_action(window.listener_for(s, BlockEditorState::on_table_insert_row_below))
            .on_action(window.listener_for(s, BlockEditorState::on_table_delete_line))
            .on_action(window.listener_for(s, BlockEditorState::on_close_table_menu))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_bold))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_toolbar))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_italic))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_underline))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_strikethrough))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_highlight))
            .on_action(window.listener_for(s, BlockEditorState::on_edit_link))
            .on_action(window.listener_for(s, BlockEditorState::on_toggle_code))
            .on_action(window.listener_for(s, BlockEditorState::on_indent))
            .on_action(window.listener_for(s, BlockEditorState::on_outdent))
            .on_action(window.listener_for(s, BlockEditorState::on_copy))
            .on_action(window.listener_for(s, BlockEditorState::on_cut))
            .on_action(window.listener_for(s, BlockEditorState::on_paste))
            .on_action(window.listener_for(s, BlockEditorState::on_native_paste))
            .on_action(window.listener_for(s, BlockEditorState::on_paste_plain))
            .children(super::table_ui::render_menu(&self.state, cx))
            .child(EditorRoot {
                state: self.state.clone(),
                child: Some(
                    div()
                        .w_full()
                        .refine_style(&self.style)
                        .child(
                            div().relative().w_full().children(children).child(
                                canvas(
                                    move |bounds, _, _| measured_content.set(bounds),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            ),
                        )
                        .into_any_element(),
                ),
                content_bounds,
                atom_bounds,
                fold_bounds,
                code_preview_bounds,
                padding: root_style.padding,
                typography,
                caret_was_visible,
                backgrounds: Some(backgrounds),
                snapshot,
                style,
            })
    }
}

type LeafBackgrounds = HashMap<u64, Vec<(Range<usize>, Hsla)>>;

struct EditorRoot {
    state: Entity<BlockEditorState>,
    child: Option<AnyElement>,
    backgrounds: Option<LeafBackgrounds>,
    atom_bounds: AtomBounds,
    fold_bounds: AtomBounds,
    code_preview_bounds: BoundsMap,
    snapshot: Rc<Snapshot>,
    style: BlockEditorStyle,
    content_bounds: Rc<Cell<Bounds<Pixels>>>,
    padding: Edges<DefiniteLength>,
    typography: TextStyle,
    caret_was_visible: bool,
}

impl IntoElement for EditorRoot {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

pub(crate) struct Prepaint {
    hitbox: Hitbox,
    grip_hitbox: Hitbox,
}

impl EditorRoot {
    /// Scroll the caret into view in the owner's scroll container, once its
    /// block has been laid out (a culled block is pinned, so it is laid out
    /// in the frame after the caret moved there).
    fn reveal_caret(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let state = self.state.read(cx);
        if !state.reveal.get() {
            return;
        }
        let (Some(handle), Some(pos)) = (state.scroll_handle.clone(), state.model.caret()) else {
            state.reveal.set(false);
            return;
        };
        let Some(caret) = state
            .frame
            .borrow()
            .caret_bounds(pos, self.style.caret_width())
        else {
            return;
        };
        state.reveal.set(false);
        // The scroll owner stays at the region edge; text and the revealed
        // caret stay inside the root's presentation inset at either scroll end.
        let padding = self
            .padding
            .to_pixels(bounds.size.map(Into::into), window.rem_size());
        let mut view = handle.bounds();
        view.origin.y += padding.top;
        view.size.height = (view.size.height - padding.top - padding.bottom).max(Pixels::ZERO);
        let mut offset = handle.offset();
        if caret.top() < view.top() {
            offset.y += view.top() - caret.top();
        } else if caret.bottom() > view.bottom() {
            offset.y -= caret.bottom() - view.bottom();
        } else {
            return;
        }
        handle.set_offset(offset);
        window.refresh();
    }
}

impl Element for EditorRoot {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut child = self
            .child
            .take()
            .unwrap_or_else(|| div().into_any_element());
        let layout_id = child.request_layout(window, cx);
        (layout_id, child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        if let Some(backgrounds) = self.backgrounds.take() {
            let (_, geometries) = collect_leaf_geometry(backgrounds, || child.prepaint(window, cx));
            // Swap in the complete frame; handlers read it until the next one.
            let mut frame = super::frame::into_frame(
                geometries,
                self.snapshot.leaves.clone(),
                &self.snapshot.tables,
            );
            frame.insert_atoms(&self.snapshot.leaves, &self.atom_bounds.lock().expect("atom bounds"));
            frame.insert_code_previews(
                &self.snapshot.leaves,
                &self.code_preview_bounds.lock().expect("code preview bounds"),
            );
            {
                let state = self.state.read(cx);
                let doc = &state.model.doc;
                let shapes: Vec<super::frame::TableShape> = self
                    .snapshot
                    .tables
                    .iter()
                    .map(|(table, _)| super::frame::TableShape {
                        table: *table,
                        grid: super::table::grid(doc, *table),
                        aligns: match doc.kind(*table) {
                            Some(super::kind::BlockKindData::Table { aligns }) => aligns.clone(),
                            _ => Vec::new(),
                        },
                    })
                    .collect();
                // gpui-kit's cell padding is `px_3`: 0.75rem.
                let pad = window.rem_size() * 0.75;
                let right = frame.content.map(|c| c.right());
                frame.layout_tables(&shapes, pad, right);
            }
            *self.state.read(cx).frame.borrow_mut() = frame;
            let mask = window.content_mask().bounds;
            let content = self.content_bounds.get();
            let cull = self.state.read(cx).cull.clone();
            let geometry_changed = cull.borrow_mut().set_geometry(
                content.size.width,
                window.rem_size(),
                &self.typography,
            );
            // Retain a visible caret or an existing reveal request, but do not
            // interrupt manual scrolling. Wait for fresh spacer positions.
            if geometry_changed {
                if self.caret_was_visible {
                    self.state.read(cx).reveal.set(true);
                }
            } else {
                self.reveal_caret(bounds, window, cx);
            }
            if geometry_changed || cull.borrow_mut().finish_frame(content, mask) {
                // Geometry changed or scrolling left the laid-out span.
                let state = self.state.clone();
                window.on_next_frame(move |_, cx| state.update(cx, |_, cx| cx.notify()));
            }
        } else {
            child.prepaint(window, cx);
        }
        // Table row/column grips and whole-table corner handles sit just outside
        // the editor bounds in the host gutter; give them a matching hitbox.
        let grip_bounds = Bounds::from_corners(
            point(bounds.left() - px(24.), bounds.top() - px(20.)),
            point(bounds.right() + px(12.), bounds.bottom() + px(12.)),
        );
        Prepaint {
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            grip_hitbox: window.insert_hitbox(grip_bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = self.state.read(cx);
        let focus = state.focus_handle.clone();
        let collapsed = state.selection_ranges().is_empty();
        let marked = state.marked_range();
        let caret = state.model.caret();
        let selected_atoms: Vec<_> = state.selection_ranges().into_iter()
            .filter(|(_, range)| !range.is_empty())
            .filter_map(|(id, _)| {
                let frame = state.frame.borrow();
                let leaf = frame.leaf(id)?;
                leaf.info.atomic.then(|| leaf.bounds())
            }).collect();
        let show_caret = state.show_caret(window, cx) && collapsed;
        let frame = state.frame.clone();
        let menu_line = state.table_menu.as_ref().map(|m| (m.table, m.line));
        let grips = state.table_grips_at(window.mouse_position());
        // Whole-table corner handles to show: the hovered table's, and any
        // node-selected table's. `true` when the pointer is on the handle.
        let table_handles: Vec<(Bounds<Pixels>, bool)> = {
            let mouse = window.mouse_position();
            let on = state.table_handle_at(mouse);
            let selected: Vec<BlockId> = state
                .model
                .selected_blocks()
                .map(|(parent, range)| {
                    state
                        .model
                        .doc
                        .children(parent)
                        .get(range)
                        .unwrap_or_default()
                        .to_vec()
                })
                .unwrap_or_default();
            let frame = state.frame.borrow();
            state
                .table_handles()
                .into_iter()
                .filter(|(table, handle)| {
                    selected.contains(table)
                        || on == Some(*table)
                        || frame.grids.iter().any(|g| {
                            g.table == *table
                                && g.bounds
                                    .union(handle)
                                    .dilate(px(TABLE_HOVER_REACH))
                                    .contains(&mouse)
                        })
                })
                .map(|(table, handle)| (handle, on == Some(table) || selected.contains(&table)))
                .collect()
        };
        // Node selection (a table, code block, …): the blocks' laid-out area.
        let selected_area = state.model.selected_blocks().and_then(|(parent, range)| {
            let doc = &state.model.doc;
            let ids = doc.children(parent).get(range)?.to_vec();
            let frame = frame.borrow();
            frame
                .leaves
                .iter()
                .filter(|leaf| {
                    let path = doc.path(leaf.info.block);
                    ids.iter()
                        .any(|id| *id == leaf.info.block || path.contains(id))
                })
                .map(|leaf| leaf.bounds())
                .reduce(|a, b| a.union(&b))
        });

        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );
        let fold_buttons: Vec<(BlockId, Bounds<Pixels>)> = self
            .fold_bounds
            .lock()
            .expect("fold bounds")
            .iter()
            .map(|(id, bounds)| (*id, *bounds))
            .collect();
        child.paint(window, cx);

        let frame = frame.borrow();
        let style = &self.style;
        // Atomic content is painted before this overlay. Keep it translucent,
        // and let whole-block selection paint once instead of stacking two fills.
        if selected_area.is_none() {
            for bounds in selected_atoms {
                window.paint_quad(fill(bounds, style.selection().opacity(0.12)));
            }
        }
        // Block handle: a grip in the left edge of the hovered table or code
        // block; clicking the edge node-selects it (`block_handle_at`).
        let mouse = window.mouse_position();
        // Tables use the corner handle below instead.
        let hovered = frame
            .leaves
            .iter()
            .filter(|l| state_plain(&self.state, l.info.block, cx))
            .map(|l| l.bounds())
            .find(|b| {
                mouse.y >= b.top()
                    && mouse.y < b.bottom()
                    && mouse.x >= b.left() - px(24.)
                    && mouse.x < b.right()
            });
        if let Some(b) = hovered {
            let grip = Bounds::new(
                point(b.left() - px(14.), b.top() + px(2.)),
                size(px(4.), px(16.).min(b.size.height)),
            );
            window.paint_quad(fill(grip, style.caret().opacity(0.35)).corner_radii(px(2.)));
        }
        // Whole-table handle: a small rounded square with a 2x2 grid glyph
        // at the frame's outer top-left corner, distinct from the thin
        // row / column grips.
        for (handle, active) in table_handles {
            let alpha = if active { 0.9 } else { 0.45 };
            window.paint_quad(gpui::quad(
                handle,
                px(3.),
                style.selection().opacity(if active { 0.45 } else { 0.0 }),
                px(1.),
                style.caret().opacity(alpha),
                gpui::BorderStyle::Solid,
            ));
            let dot = px(2.);
            let c = handle.center();
            for (dx, dy) in [(-2.5, -2.5), (0.5, -2.5), (-2.5, 0.5), (0.5, 0.5)] {
                window.paint_quad(fill(
                    Bounds::new(point(c.x + px(dx), c.y + px(dy)), size(dot, dot)),
                    style.caret().opacity(alpha),
                ));
            }
        }
        // Table grips (D20): only for the hovered column / row, or the
        // line whose menu is open.
        for (table, line, grip) in grips {
            let open = menu_line == Some((table, line));
            let alpha = if open { 0.8 } else { 0.35 };
            window.paint_quad(fill(grip, style.caret().opacity(alpha)).corner_radii(px(2.)));
        }
        if let Some(area) = selected_area {
            let area = area.dilate(px(4.));
            window.paint_quad(gpui::quad(
                area,
                px(4.),
                style.selection().opacity(0.12),
                px(1.),
                style.caret().opacity(0.4),
                gpui::BorderStyle::Solid,
            ));
        }
        // D22: where a dragged block would drop.
        if let Some(line) = self.state.read(cx).drop_indicator() {
            window.paint_quad(fill(line, style.caret()).corner_radii(px(1.)));
        }
        if let Some((block, range)) = marked
            && let Some(leaf) = frame.leaf(block)
        {
            let a = leaf.caret_point(range.start);
            let b = leaf.caret_point(range.end);
            let thickness = style.caret_width();
            let underline = Bounds::new(
                point(a.x, a.y + leaf.line_height() - thickness),
                size((b.x - a.x).max(thickness), thickness),
            );
            window.paint_quad(fill(underline, style.composition_underline()));
        }
        if show_caret
            && let Some(pos) = caret
            && let Some(b) = frame.caret_bounds(pos, style.caret_width())
        {
            // Whole device pixels, like the input caret: a 1px line at a
            // fractional x would blur over two pixels.
            let b = crate::support::snap_caret_to_device_pixels(b, window.scale_factor());
            window.paint_quad(fill(b, style.caret()));
        }
        drop(frame);

        // Mouse: handlers update the entity at event time, never in paint.
        let hitbox = prepaint.hitbox.clone();
        let grip_hitbox = prepaint.grip_hitbox.clone();
        let preview_bounds = self.code_preview_bounds.clone();
        let entity = self.state.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            let in_editor = hitbox.is_hovered(window);
            let in_grip_gutter = grip_hitbox.is_hovered(window);
            if !in_editor && !in_grip_gutter {
                return;
            }
            // Preview controls and selection handle their own clicks; hit-testing
            // their synthetic text leaf would otherwise open the source editor.
            if preview_bounds.lock().expect("code preview bounds")
                .values().any(|bounds| bounds.contains(&event.position))
            {
                return;
            }
            if let Some((fold, _)) = fold_buttons.iter().find(|(_, bounds)| bounds.contains(&event.position)) {
                entity.update(cx, |state, cx| state.toggle_fold(*fold, cx));
                cx.stop_propagation();
                return;
            }
            // A table grip selects its row / column and opens the menu;
            // any other click closes an open menu.
            if !event.modifiers.shift
                && entity.update(cx, |state, cx| {
                    let hit = state.click_table_grip(event.position, cx);
                    if !hit {
                        state.close_table_menu(cx);
                    }
                    hit
                })
            {
                let focus = entity.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                cx.stop_propagation();
                return;
            }
            if !in_editor && entity.read(cx).block_handle_at(event.position).is_none() {
                return;
            }
            // A task checkbox toggles without moving the caret or starting
            // a drag; nothing else handles this mouse-down.
            if !event.modifiers.shift
                && entity.update(cx, |state, cx| state.click_checkbox(event.position, cx))
            {
                cx.stop_propagation();
                return;
            }
            entity.update(cx, |state, cx| {
                let focus = state.focus_handle.clone();
                window.focus(&focus, cx);
                if secondary_modifier().is_subset_of(&event.modifiers)
                    && state.follow_link_at(event.position, cx)
                {
                    return;
                }
                state.dragging = true;
                if !event.modifiers.shift {
                    state.arm_block_drag(event.position);
                }
                state.mouse_at(event.position, event.modifiers.shift, cx);
                if !event.modifiers.shift && event.click_count >= 2 {
                    state.select_unit(event.click_count >= 3, cx);
                }
            });
        });
        let entity = self.state.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if event.pressed_button.is_none() {
                entity.update(cx, |state, cx| {
                    state.hover_at(
                        event.position,
                        secondary_modifier().is_subset_of(&event.modifiers),
                        cx,
                    )
                });
                return;
            }
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            entity.update(cx, |state, cx| {
                if state.block_drag_to(event.position, cx) {
                    return;
                }
                if state.dragging {
                    state.mouse_at(event.position, true, cx);
                }
            });
        });
        let entity = self.state.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                entity.update(cx, |state, cx| {
                    state.dragging = false;
                    state.finish_block_drag(cx);
                });
            }
        });
    }
}

/// Pointer reach around a table where its corner handle shows.
const TABLE_HOVER_REACH: f32 = 28.;

fn state_plain(state: &Entity<BlockEditorState>, block: BlockId, cx: &App) -> bool {
    state.read(cx).model.doc.plain(block).is_some()
}
