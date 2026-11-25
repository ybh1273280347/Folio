//! Native HTML table layout and cell editing. The HTML block remains the
//! document's source so its row groups, spans and attributes can round-trip.

use std::sync::Arc;

use gpui::AppContext as _;
use gpui::{
    AnyElement, App, AvailableSpace, Axis, Bounds, Div, Element, ElementId, Focusable as _,
    FontWeight, GlobalElementId, InspectorElementId, InteractiveElement as _, IntoElement as _,
    LayoutId, ParentElement as _, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, TextAlign, WeakEntity, Window, canvas, div, px, size,
};
use gpui_kit::{
    base::{
        ScrollableMask, Scrollbar, ScrollbarMode, Theme,
        text::{
            MarkdownExtensions, MarkdownNode,
            editor_host::{BlockNode, FlowPosition, RenderContext},
        },
    },
    component::{
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{Textarea, TextareaState},
    },
    prelude::FluentBuilder as _,
};
use html5ever::{
    QualName, local_name, namespace_url, ns, parse_fragment, serialize, tendril::TendrilSink as _,
};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};

use super::{BlockEditorState, BlockId};

pub(crate) const NAME: &str = "editor-html-table";
pub(crate) const EDITABLE_NAME: &str = "editor-html-table-editable";

/// A view-only binding between a parsed table and the current editor block.
pub(crate) struct EditableTable {
    pub(crate) node: MarkdownNode,
    pub(crate) editor: WeakEntity<BlockEditorState>,
    pub(crate) block: BlockId,
}

pub(crate) struct HtmlTable {
    columns: u16,
    cells: Vec<Cell>,
    extensions: Arc<MarkdownExtensions>,
}

struct Cell {
    rows: std::ops::Range<i16>,
    columns: std::ops::Range<i16>,
    text: SharedString,
    blocks: Vec<BlockNode>,
    align: TextAlign,
    header: bool,
}

fn tag(node: &Handle) -> &str {
    match &node.data {
        NodeData::Element { name, .. } if name.ns == ns!(html) => &name.local,
        _ => "",
    }
}

pub(crate) fn starts_with_html_block(source: &str) -> bool {
    let tag = source
        .trim_start()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '>')
        .next();
    tag.is_some_and(|tag| tag.eq_ignore_ascii_case("<table") || tag.eq_ignore_ascii_case("<div"))
}

fn html_fragment(source: &str) -> Handle {
    let context = QualName::new(None, ns!(html), local_name!("body"));
    let dom = parse_fragment(RcDom::default(), Default::default(), context, vec![]).one(source);
    let root = dom.document.children.borrow_mut().remove(0);
    root
}

/// Resolve a standalone table, including a table wrapped in layout-only divs.
fn table_element(root: &Handle) -> Option<Handle> {
    let mut container = root.clone();
    loop {
        let child = {
            let nodes = container.children.borrow();
            let mut content = nodes.iter().filter(|node| match &node.data {
                NodeData::Text { contents } => !contents.borrow().trim().is_empty(),
                NodeData::Comment { .. } => false,
                _ => true,
            });
            let child = content.next()?;
            if content.next().is_some() {
                return None;
            }
            child.clone()
        };
        match tag(&child) {
            "table" => return Some(child),
            "div" => container = child,
            _ => return None,
        }
    }
}

fn row_groups(table: &Handle) -> Vec<Vec<Handle>> {
    // The HTML parser inserts a tbody for bare tr elements. The HTML display
    // order puts the header first and footer last regardless of source order.
    let children = table.children.borrow();
    let mut groups: Vec<_> = children
        .iter()
        .filter(|node| matches!(tag(node), "thead" | "tbody" | "tfoot"))
        .collect();
    groups.sort_by_key(|node| match tag(node) {
        "thead" => 0,
        "tfoot" => 2,
        _ => 1,
    });
    groups
        .iter()
        .map(|group| {
            group
                .children
                .borrow()
                .iter()
                .filter(|node| tag(node) == "tr")
                .cloned()
                .collect()
        })
        .collect()
}

fn row_cells(row: &Handle) -> Vec<Handle> {
    row.children
        .borrow()
        .iter()
        .filter(|node| matches!(tag(node), "td" | "th"))
        .cloned()
        .collect()
}

pub(crate) fn parse(source: &str, extensions: Arc<MarkdownExtensions>) -> Option<HtmlTable> {
    let source = source.trim();
    let closing = source.rsplit_once('<')?.1.strip_suffix('>')?.trim_end();
    // A blank line can end a CommonMark HTML block before its table closes.
    if !closing.eq_ignore_ascii_case("/table") && !closing.eq_ignore_ascii_case("/div") {
        return None;
    }
    // Keep the fragment alive while visiting its descendants. Dropping an
    // RcDom node drains its subtree even if a descendant handle was cloned.
    let root = html_fragment(source);
    let table = table_element(&root)?;
    let mut cells = Vec::new();
    let mut occupied_until = Vec::new();
    let mut row_index = 1usize;
    for rows in row_groups(&table) {
        let group_end = row_index + rows.len();
        for row in rows {
            let mut column = 0;
            for node in row_cells(&row) {
                let header = tag(&node) == "th";
                let NodeData::Element { attrs, .. } = &node.data else {
                    unreachable!()
                };
                let attrs = attrs.borrow();
                let attr = |name| {
                    attrs
                        .iter()
                        .find(|attr| attr.name.local == name)
                        .map(|attr| attr.value.as_ref())
                };
                let colspan = attr(local_name!("colspan"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let rowspan = attr(local_name!("rowspan"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(1);
                let row_end = if rowspan == 0 {
                    group_end
                } else {
                    row_index + rowspan.min(group_end - row_index)
                };

                // Every covered track is reserved until the cell's ending row.
                // This keeps cells after rowspans on their original columns.
                while let Some(offset) = occupied_until
                    .iter()
                    .skip(column)
                    .take(colspan)
                    .position(|&end| end > row_index)
                {
                    column += offset + 1;
                }
                let column_end = column + colspan;
                let columns =
                    i16::try_from(column + 1).ok()?..i16::try_from(column_end + 1).ok()?;
                if column_end > occupied_until.len() {
                    occupied_until.resize(column_end, 0);
                }
                occupied_until[column..column_end].fill(row_end);
                column = column_end;

                let align = attr(local_name!("style"))
                    .into_iter()
                    .flat_map(|style| style.rsplit(';'))
                    .filter_map(|declaration| declaration.split_once(':'))
                    .filter(|(property, _)| property.trim().eq_ignore_ascii_case("text-align"))
                    .map(|(_, value)| value)
                    .chain(attr(local_name!("align")))
                    .find_map(|value| match value.trim().to_ascii_lowercase().as_str() {
                        "left" | "start" => Some(TextAlign::Left),
                        "center" => Some(TextAlign::Center),
                        "right" | "end" => Some(TextAlign::Right),
                        _ => None,
                    })
                    .unwrap_or(if header {
                        TextAlign::Center
                    } else {
                        TextAlign::Left
                    });
                let mut text = String::new();
                append_cell_text(&node, &mut text);
                let mut markdown = Vec::new();
                serialize(
                    &mut markdown,
                    &SerializableHandle::from(node.clone()),
                    Default::default(),
                )
                .ok()?;
                let markdown = String::from_utf8(markdown).ok()?;
                let blocks = super::math::parse_markdown(&markdown, extensions.clone()).ok()?;
                cells.push(Cell {
                    rows: i16::try_from(row_index).ok()?..i16::try_from(row_end).ok()?,
                    columns,
                    text: text
                        .trim_matches(|ch: char| ch.is_ascii_whitespace())
                        .into(),
                    blocks,
                    align,
                    header,
                });
            }
            row_index += 1;
        }
    }
    (!cells.is_empty()).then_some(HtmlTable {
        columns: occupied_until.len() as u16,
        cells,
        extensions,
    })
}

fn append_cell_text(node: &Handle, text: &mut String) {
    if let NodeData::Text { contents } = &node.data {
        for ch in contents.borrow().chars() {
            if !ch.is_ascii_whitespace() {
                text.push(ch);
            } else if !text.is_empty() && !text.ends_with([' ', '\n']) {
                text.push(' ');
            }
        }
    } else {
        match tag(node) {
            "br" => text.push('\n'),
            "script" | "style" | "template" => (),
            _ => {
                for child in node.children.borrow().iter() {
                    append_cell_text(child, text);
                }
            }
        }
    }
}

/// Replace only the target cell's children. Serializing the fragment keeps
/// the other cells, wrapper elements and their HTML attributes intact.
pub(crate) fn replace_cell(node: &MarkdownNode, index: usize, value: &str) -> Option<MarkdownNode> {
    let source_table = node.data::<HtmlTable>().expect("HTML table data");
    let original = &source_table.cells[index];
    if original.text.as_ref() == value {
        return None;
    }
    let root = html_fragment(node.as_markdown());
    let table = table_element(&root).expect("an imported HTML table has one table element");
    let cell = row_groups(&table)
        .iter()
        .flatten()
        .flat_map(row_cells)
        .nth(index)
        .expect("cell index comes from the same parsed table");
    let escaped = value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace("\r\n", "\n")
        .replace('\n', "<br>");
    cell.children
        .replace(html_fragment(&escaped).children.take());
    let mut output = Vec::new();
    serialize(
        &mut output,
        &SerializableHandle::from(root),
        Default::default(),
    )
    .expect("serialization to memory is infallible");
    let source = String::from_utf8(output).expect("HTML serialization writes UTF-8");
    let table = parse(&source, source_table.extensions.clone())
        .expect("replacing cell text preserves the table structure");
    Some(
        MarkdownNode::new(NAME, table)
            .text(source.clone())
            .markdown(source),
    )
}

pub(crate) fn render(node: &MarkdownNode, window: &mut Window, cx: &mut App) -> Div {
    let grid = render_grid(node, false, window, cx);
    let key = node.data::<EditableTable>()
        .map(|editable| editable.block.as_u64())
        .unwrap_or_else(|| node.source_range().map_or(0, |span| span.start as u64));
    let scroll = window
        .use_keyed_state(("html-table-scroll", key), cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone();
    div()
        .relative()
        .w_full()
        .child(
            div()
                .id(("html-table-viewport", key))
                .w_full()
                .overflow_hidden()
                .track_scroll(&scroll)
                .child(grid),
        )
        .child(ScrollableMask::new(Axis::Horizontal, &scroll).id(("html-table-mask", key)))
        .child(
            Scrollbar::horizontal(&scroll)
                .mode(ScrollbarMode::Always)
                .styles(|styles| {
                    styles
                        .track(|track| track.width(px(8.)))
                        .thumb(|thumb| thumb.width(px(3.)).inset(px(2.)))
                }),
        )
}

/// The PDF layout runs before the window enters vector paint capture.
pub(crate) fn render_document(
    node: &MarkdownNode,
    width: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    DocumentTable {
        grid: render_grid(node, true, window, cx).into_any_element(),
        width,
    }.into_any_element()
}

fn render_grid(node: &MarkdownNode, document: bool, window: &mut Window, cx: &mut App) -> Div {
    let editable = node.data::<EditableTable>();
    let source = editable.map_or(node, |editable| &editable.node);
    let table = source.data::<HtmlTable>().expect("HTML table preview data");
    let border = Theme::global(cx).tokens.colors.border;
    let render_context = RenderContext::from_app(cx)
        .with_markdown_extensions(table.extensions.clone())
        .with_body_font_size(window.text_style().font_size.to_pixels(window.rem_size()));
    div()
        .grid()
        // Min-content tracks ask Taffy to allocate an intrinsic contribution
        // across all tracks a cell spans. Zero-minimum tracks ignore it.
        .grid_cols_min_content(table.columns)
        .when(!document, |this| this.min_w_full())
        .border_t_1()
        .border_l_1()
        .border_color(border)
        .children(table.cells.iter().enumerate().map(|(index, cell)| {
            div()
                .row_start(cell.rows.start)
                .row_end(cell.rows.end)
                .col_start(cell.columns.start)
                .col_end(cell.columns.end)
                .when(!document, |this| {
                    // A spanning cell contributes its own minimum to the total,
                    // while individual columns get floors from their own cells.
                    this.min_w(px(160.))
                })
                .overflow_hidden()
                .px(px(12.))
                .py(px(7.))
                .border_b_1()
                .border_r_1()
                .border_color(border)
                .whitespace_normal()
                .text_align(cell.align)
                .when(cell.header, |this| this.font_weight(FontWeight::SEMIBOLD))
                .children(window.with_id(("html-cell", index), |window| {
                    let nodes: Vec<_> = cell.blocks.iter().collect();
                    nodes.iter().enumerate().map(|(index, block)| {
                        block.render(
                            index,
                            FlowPosition::at(&nodes, index, index + 1 == nodes.len()),
                            &render_context,
                            window,
                            cx,
                        )
                    }).collect::<Vec<_>>()
                }))
                .id(("html-cell", index))
                .when_some(editable, |this, editable| {
                    let editor = editable.editor.clone();
                    let block = editable.block;
                    let text = cell.text.clone();
                    this.cursor_text().on_click(move |event, window, cx| {
                        if event.click_count() < 2 {
                            return;
                        }
                        cx.stop_propagation();
                        let input =
                            cx.new(|cx| TextareaState::new(window, cx).default_value(text.clone()));
                        let editor = editor.clone();
                        let focus = input.focus_handle(cx);
                        window.open_dialog(cx, move |dialog, _, _| {
                            let editor = editor.clone();
                            let input_for_save = input.clone();
                            dialog
                                .title("编辑表格单元格")
                                .child(Textarea::new(&input).h(px(100.)))
                                .footer(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            Button::new("cancel")
                                                .label("取消")
                                                .on_click(|_, window, cx| window.close_dialog(cx)),
                                        )
                                        .child(
                                            Button::new("save").primary().label("保存").on_click(
                                                move |_, window, cx| {
                                                    let value = input_for_save.read(cx).value();
                                                    let _ = editor.update(cx, |state, cx| {
                                                        state.edit_html_cell(
                                                            block, index, &value, cx,
                                                        )
                                                    });
                                                    window.close_dialog(cx);
                                                },
                                            ),
                                        ),
                                )
                        });
                        window.defer(cx, move |window, cx| focus.focus(window, cx));
                    })
                })
        }))
        .when(document, |grid| {
            // Use the existing pagination markers for connected row groups.
            // A boundary is safe only when no cell crosses it.
            let mut end = 1;
            let mut start = 1;
            let mut ranges = Vec::new();
            for cell in &table.cells {
                if cell.rows.start >= end && start < end {
                    ranges.push(start..end);
                    start = end;
                }
                end = end.max(cell.rows.end);
            }
            ranges.push(start..end);
            grid.children(ranges.into_iter().map(|rows| {
                canvas(|_, _, _| (), |bounds, (), window, _| window.vector_block(bounds))
                    .row_start(rows.start)
                    .row_end(rows.end)
                    .col_start(1)
                    .col_end(2)
                    .size_full()
            }))
        })
}

struct DocumentTable {
    grid: AnyElement,
    width: Pixels,
}

impl gpui::IntoElement for DocumentTable {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for DocumentTable {
    type RequestLayoutState = (gpui::Size<Pixels>, f32);
    type PrepaintState = ();

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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let grid = self.grid.layout_as_root(
            size(AvailableSpace::MinContent, AvailableSpace::MinContent),
            window,
            cx,
        );
        let width = self.width.max(px(1.));
        let target = grid.width.max(width);
        let natural = self.grid.layout_as_root(
            size(AvailableSpace::Definite(target), AvailableSpace::MinContent),
            window,
            cx,
        );
        let scale = f32::from(width) / f32::from(natural.width.max(width));
        let style = gpui::Style {
            size: size(natural.width * scale, natural.height * scale).map(Into::into),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), (natural, scale))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (natural, _): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_document_mask(Bounds::new(bounds.origin, *natural), |window| {
            self.grid.prepaint_at(bounds.origin, window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (natural, scale): &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_scaled_vector_paint(bounds, *natural, *scale, |window| {
            self.grid.paint(window, cx);
        });
    }
}
