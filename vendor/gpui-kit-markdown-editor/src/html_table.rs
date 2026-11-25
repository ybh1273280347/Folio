//! Native HTML table layout and cell editing. The HTML block remains the
//! document's source so its row groups, spans and attributes can round-trip.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use gpui::{
    AnyElement, App, AvailableSpace, Axis, Bounds, Div, Element, ElementId, FontWeight,
    GlobalElementId, InspectorElementId, InteractiveElement as _, IntoElement as _, LayoutId,
    ParentElement as _, Pixels, ScrollHandle, StatefulInteractiveElement as _, Styled as _,
    TextAlign, Window, canvas, div, px, size,
};
use gpui_kit::{
    base::{
        ScrollableMask, Scrollbar, ScrollbarMode, Theme,
        text::{
            MarkdownExtensions, MarkdownNode,
            editor_host::{BlockNode, FlowPosition, RenderContext},
        },
    },
    prelude::FluentBuilder as _,
};
use html5ever::{
    QualName, local_name, namespace_url, ns, parse_fragment, serialize, tendril::TendrilSink as _,
};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};

use super::BlockId;

pub(crate) const NAME: &str = "editor-html-table";
pub(crate) struct HtmlTable {
    pub(super) columns: u16,
    pub(super) rows: u16,
    pub(super) cells: Vec<Cell>,
    pub(super) extensions: Arc<MarkdownExtensions>,
    pub(super) table_id: Option<BlockId>,
}

pub(super) struct Cell {
    pub(super) rows: std::ops::Range<i16>,
    pub(super) columns: std::ops::Range<i16>,
    pub(super) blocks: Vec<BlockNode>,
    pub(super) align: TextAlign,
    pub(super) header: bool,
    id: Option<BlockId>,
}

impl Cell {
    pub(super) fn projected(
        rows: std::ops::Range<i16>,
        columns: std::ops::Range<i16>,
        paragraph: gpui_kit::base::text::editor_host::Paragraph,
        align: TextAlign,
        header: bool,
        id: BlockId,
    ) -> Self {
        Self {
            rows,
            columns,
            blocks: vec![BlockNode::Paragraph(paragraph)],
            align,
            header,
            id: Some(id),
        }
    }
}

pub(super) fn tag(node: &Handle) -> &str {
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

pub(super) fn html_fragment(source: &str) -> Handle {
    let context = QualName::new(None, ns!(html), local_name!("body"));
    let dom = parse_fragment(RcDom::default(), Default::default(), context, vec![]).one(source);
    let root = dom.document.children.borrow_mut().remove(0);
    root
}

/// Resolve a standalone table, including a table wrapped in layout-only divs.
pub(super) fn table_element(root: &Handle) -> Option<Handle> {
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

pub(super) fn group_elements(table: &Handle) -> Vec<Handle> {
    // The HTML parser inserts a tbody for bare tr elements. The HTML display
    // order puts the header first and footer last regardless of source order.
    let children = table.children.borrow();
    let mut groups: Vec<_> = children
        .iter()
        .filter(|node| matches!(tag(node), "thead" | "tbody" | "tfoot"))
        .cloned()
        .collect();
    groups.sort_by_key(|node| match tag(node) {
        "thead" => 0,
        "tfoot" => 2,
        _ => 1,
    });
    groups
}

pub(super) fn group_rows(group: &Handle) -> Vec<Handle> {
    group
        .children
        .borrow()
        .iter()
        .filter(|node| tag(node) == "tr")
        .cloned()
        .collect()
}

fn row_groups(table: &Handle) -> Vec<Vec<Handle>> {
    group_elements(table).iter().map(group_rows).collect()
}

pub(super) fn row_cells(row: &Handle) -> Vec<Handle> {
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
                    blocks,
                    align,
                    header,
                    id: None,
                });
            }
            row_index += 1;
        }
    }
    (!cells.is_empty()).then_some(HtmlTable {
        columns: occupied_until.len() as u16,
        rows: (row_index - 1) as u16,
        cells,
        extensions,
        table_id: None,
    })
}

#[derive(Default)]
pub(crate) struct Geometry {
    pub(crate) viewport: Bounds<Pixels>,
    pub(crate) columns: Vec<(Pixels, Pixels)>,
    pub(crate) rows: Vec<(Pixels, Pixels)>,
    pub(crate) cells: HashMap<BlockId, Bounds<Pixels>>,
}

pub(crate) type Layouts = Arc<Mutex<HashMap<BlockId, Geometry>>>;

pub(crate) fn render(node: &MarkdownNode, window: &mut Window, cx: &mut App) -> Div {
    render_live(node, None, None, window, cx)
}

pub(crate) fn render_live(
    node: &MarkdownNode,
    layouts: Option<&Layouts>,
    reveal: Option<BlockId>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let table = node.data::<HtmlTable>().expect("HTML table preview data");
    let key = table.table_id.map_or_else(
        || node.source_range().map_or(0, |span| span.start as u64),
        BlockId::as_u64,
    );
    let scroll = window
        .use_keyed_state(("html-table-scroll", key), cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone();
    let measured = table.table_id.zip(layouts.cloned());
    if let Some((id, layouts)) = &measured {
        layouts.lock().expect("HTML table geometry").insert(
            *id,
            Geometry {
                rows: vec![(px(0.), px(0.)); table.rows as usize],
                columns: vec![(px(0.), px(0.)); table.columns as usize],
                ..Default::default()
            },
        );
    }
    let grid = render_grid(table, false, measured.as_ref(), window, cx);
    div()
        .relative()
        .w_full()
        .when_some(measured, |view, (id, layouts)| {
            let scroll = scroll.clone();
            let measured = layouts.clone();
            view.child(
                canvas(
                    move |bounds, _, _| {
                        layouts
                            .lock()
                            .expect("HTML table geometry")
                            .get_mut(&id)
                            .unwrap()
                            .viewport = bounds
                    },
                    move |bounds, (), window, _| {
                        // Run after measurements, and only for an editor reveal
                        // request so manually scrolling does not snap back.
                        let cell = reveal.and_then(|cell| {
                            measured
                                .lock()
                                .expect("HTML table geometry")
                                .get(&id)?
                                .cells
                                .get(&cell)
                                .copied()
                        });
                        if let Some(cell) = cell {
                            let delta =
                                if cell.left() < bounds.left() || cell.left() >= bounds.right() {
                                    bounds.left() - cell.left()
                                } else if cell.right() > bounds.right()
                                    && cell.size.width <= bounds.size.width
                                {
                                    bounds.right() - cell.right()
                                } else {
                                    px(0.)
                                };
                            if delta != px(0.) {
                                let offset = scroll.offset();
                                scroll.set_offset(gpui::point(offset.x + delta, offset.y));
                                window.refresh();
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
        })
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
        grid: render_grid(
            node.data::<HtmlTable>().expect("HTML table preview data"),
            true,
            None,
            window,
            cx,
        )
        .into_any_element(),
        width,
    }
    .into_any_element()
}

fn render_grid(
    table: &HtmlTable,
    document: bool,
    measured: Option<&(BlockId, Layouts)>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let border = Theme::global(cx).tokens.colors.border;
    let render_context = RenderContext::from_app(cx)
        .with_markdown_extensions(table.extensions.clone())
        .with_body_font_size(window.text_style().font_size.to_pixels(window.rem_size()));
    div()
        .grid()
        // Min-content tracks ask Taffy to allocate an intrinsic contribution
        // across all tracks a cell spans. Zero-minimum tracks ignore it.
        .grid_cols_min_content(table.columns)
        .grid_rows_min_content(table.rows)
        .when(!document, |this| this.min_w_full())
        .border_t_1()
        .border_l_1()
        .border_color(border)
        .children(table.cells.iter().enumerate().map(|(index, cell)| {
            let id = ("html-cell", cell.id.map_or(index as u64, BlockId::as_u64));
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
                .relative()
                .overflow_hidden()
                .px(px(12.))
                .py(px(7.))
                .border_b_1()
                .border_r_1()
                .border_color(border)
                .whitespace_normal()
                .text_align(cell.align)
                .when(cell.header, |this| this.font_weight(FontWeight::SEMIBOLD))
                .children(window.with_id(id, |window| {
                    let nodes: Vec<_> = cell.blocks.iter().collect();
                    nodes
                        .iter()
                        .enumerate()
                        .map(|(index, block)| {
                            block.render(
                                index,
                                FlowPosition::at(&nodes, index, index + 1 == nodes.len()),
                                &render_context,
                                window,
                                cx,
                            )
                        })
                        .collect::<Vec<_>>()
                }))
                .id(id)
                .when_some(measured.zip(cell.id), |this, ((table, layouts), cell)| {
                    let (table, layouts) = (*table, layouts.clone());
                    this.cursor_text().child(
                        canvas(
                            move |bounds, _, _| {
                                layouts
                                    .lock()
                                    .expect("HTML table geometry")
                                    .get_mut(&table)
                                    .unwrap()
                                    .cells
                                    .insert(cell, bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                })
        }))
        .when_some(measured, |grid, (table_id, layouts)| {
            let table_id = *table_id;
            let columns = (0..table.columns as i16).map(|c| {
                let layouts = layouts.clone();
                canvas(
                    move |bounds, _, _| {
                        layouts
                            .lock()
                            .expect("HTML table geometry")
                            .get_mut(&table_id)
                            .unwrap()
                            .columns[c as usize] = (bounds.left(), bounds.right());
                    },
                    |_, _, _, _| {},
                )
                .row_start(1)
                .row_end(2)
                .col_start(c + 1)
                .col_end(c + 2)
                .size_full()
            });
            let rows = (0..table.rows as i16).map(|r| {
                let layouts = layouts.clone();
                canvas(
                    move |bounds, _, _| {
                        layouts
                            .lock()
                            .expect("HTML table geometry")
                            .get_mut(&table_id)
                            .unwrap()
                            .rows[r as usize] = (bounds.top(), bounds.bottom());
                    },
                    |_, _, _, _| {},
                )
                .col_start(1)
                .col_end(2)
                .row_start(r + 1)
                .row_end(r + 2)
                .size_full()
            });
            grid.children(columns).children(rows)
        })
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
                canvas(
                    |_, _, _| (),
                    |bounds, (), window, _| window.vector_block(bounds),
                )
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
