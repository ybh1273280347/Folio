//! Native HTML table layout and cell editing. The HTML block remains the
//! document's source so its row groups, spans and attributes can round-trip.

use gpui::AppContext as _;
use gpui::{
    App, Div, Focusable as _, FontWeight, InteractiveElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, TextAlign, WeakEntity, Window, div,
    px,
};
use gpui_kit::{
    base::{Theme, text::MarkdownNode},
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
}

struct Cell {
    rows: std::ops::Range<i16>,
    columns: std::ops::Range<i16>,
    text: SharedString,
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

pub(crate) fn parse(source: &str) -> Option<HtmlTable> {
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
                cells.push(Cell {
                    rows: i16::try_from(row_index).ok()?..i16::try_from(row_end).ok()?,
                    columns,
                    text: text
                        .trim_matches(|ch: char| ch.is_ascii_whitespace())
                        .into(),
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
    let original = &node.data::<HtmlTable>().expect("HTML table data").cells[index];
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
    let table = parse(&source).expect("replacing cell text preserves the table structure");
    Some(
        MarkdownNode::new(NAME, table)
            .text(source.clone())
            .markdown(source),
    )
}

pub(crate) fn render(node: &MarkdownNode, _: &mut Window, cx: &mut App) -> Div {
    let editable = node.data::<EditableTable>();
    let source = editable.map_or(node, |editable| &editable.node);
    let table = source.data::<HtmlTable>().expect("HTML table preview data");
    let border = Theme::global(cx).tokens.colors.border;
    div()
        .grid()
        .grid_cols(table.columns)
        .w_full()
        .border_t_1()
        .border_l_1()
        .border_color(border)
        .children(table.cells.iter().enumerate().map(|(index, cell)| {
            div()
                .row_start(cell.rows.start)
                .row_end(cell.rows.end)
                .col_start(cell.columns.start)
                .col_end(cell.columns.end)
                .min_w_0()
                .px(px(12.))
                .py(px(7.))
                .border_b_1()
                .border_r_1()
                .border_color(border)
                .whitespace_normal()
                .text_align(cell.align)
                .when(cell.header, |this| this.font_weight(FontWeight::SEMIBOLD))
                .child(cell.text.clone())
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
}
