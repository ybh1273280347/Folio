//! HTML table metadata belongs to the owning document nodes. The grid is
//! derived from ordered rows and spans, so layout and commands see the same
//! cells while Markdown tables retain their rectangular storage.

use std::{ops::Range, sync::Arc};

use gpui::{App, ParentElement as _, SharedString, TextAlign, Window, div};
use gpui_kit::base::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin,
    editor_host::{BlockNode, FlowPosition, RenderContext, TextMark},
};
use html5ever::{
    Attribute, QualName, local_name, namespace_url, ns, serialize,
    tokenizer::{BufferQueue, EndTag, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer},
};
use markup5ever_rcdom::{Handle, NodeData, SerializableHandle};

use super::{
    BlockEditorModel, BlockId, TableEdit, TableLine,
    command::CommandError,
    document::{BlockContent, EditorDocument},
    html_table::{
        self, HtmlTable, group_elements, group_rows, html_fragment, row_cells, table_element, tag,
    },
    import::{Defs, import_inline},
    inline::{InlineAtom, InlineText},
    kind::BlockKindData,
    position::BlockPosition,
    selection::BlockSelection,
    step::DetachedBlock,
    transaction::EditOrigin,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Source {
    /// Preserve the original bytes when the table and its cells are unchanged.
    html: SharedString,
    rows: Vec<OriginalRow>,
    groups: Vec<RowGroup>,
}

#[derive(Clone, Debug, PartialEq)]
struct RowGroup {
    header: bool,
    implied: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct OriginalRow {
    id: BlockId,
    cells: Vec<(BlockId, Cell, InlineText)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    group: usize,
    original: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cell {
    original: Option<(usize, usize)>,
    /// Zero keeps the HTML meaning of spanning to the end of this row group.
    rowspan: usize,
    colspan: usize,
    align: TextAlign,
    header: bool,
}

pub(crate) struct Grid {
    pub(crate) rows: Vec<BlockId>,
    pub(crate) cells: Vec<PlacedCell>,
    pub(crate) columns: usize,
}

pub(crate) struct PlacedCell {
    pub(crate) id: BlockId,
    pub(crate) rows: Range<usize>,
    pub(crate) columns: Range<usize>,
}

impl Grid {
    pub(crate) fn new(doc: &EditorDocument, table: BlockId) -> Self {
        let rows = doc.children(table).to_vec();
        let mut occupied_until = Vec::new();
        let mut cells = Vec::new();
        let mut row_index = 0;
        for group in rows.chunk_by(|a, b| row_kind(doc, *a).group == row_kind(doc, *b).group) {
            let group_end = row_index + group.len();
            for row in group {
                let mut column = 0;
                for &id in doc.children(*row) {
                    let cell = cell_kind(doc, id);
                    let row_end = if cell.rowspan == 0 {
                        group_end
                    } else {
                        row_index.saturating_add(cell.rowspan).min(group_end)
                    };
                    while let Some(offset) = occupied_until
                        .iter()
                        .skip(column)
                        .take(cell.colspan)
                        .position(|&end| end > row_index)
                    {
                        column += offset + 1;
                    }
                    let column_end = column + cell.colspan;
                    occupied_until.resize(occupied_until.len().max(column_end), 0);
                    occupied_until[column..column_end].fill(row_end);
                    cells.push(PlacedCell {
                        id,
                        rows: row_index..row_end,
                        columns: column..column_end,
                    });
                    column = column_end;
                }
                row_index += 1;
            }
        }
        Self {
            rows,
            cells,
            columns: occupied_until.len(),
        }
    }

    pub(crate) fn at(&self, row: usize, column: usize) -> Option<BlockId> {
        self.cells
            .iter()
            .find(|cell| cell.rows.contains(&row) && cell.columns.contains(&column))
            .map(|cell| cell.id)
    }

    /// Step past the current cell's entire span and skip holes in ragged rows.
    pub(crate) fn neighbour(&self, id: BlockId, vertical: bool, forward: bool) -> Option<BlockId> {
        let cell = self
            .cells
            .iter()
            .find(|cell| cell.id == id)
            .expect("HTML table cell");
        match (vertical, forward) {
            (true, true) => {
                (cell.rows.end..self.rows.len()).find_map(|row| self.at(row, cell.columns.start))
            }
            (true, false) => (0..cell.rows.start)
                .rev()
                .find_map(|row| self.at(row, cell.columns.start)),
            (false, true) => {
                (cell.columns.end..self.columns).find_map(|column| self.at(cell.rows.start, column))
            }
            (false, false) => (0..cell.columns.start)
                .rev()
                .find_map(|column| self.at(cell.rows.start, column)),
        }
    }
}

fn row_kind(doc: &EditorDocument, id: BlockId) -> &Row {
    let Some(BlockKindData::HtmlTableRow(row)) = doc.kind(id) else {
        unreachable!()
    };
    row
}

fn cell_kind(doc: &EditorDocument, id: BlockId) -> &Cell {
    let Some(BlockKindData::HtmlTableCell(cell)) = doc.kind(id) else {
        unreachable!()
    };
    cell
}

fn span(node: &Handle, name: &str) -> usize {
    let NodeData::Element { attrs, .. } = &node.data else {
        unreachable!()
    };
    attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .and_then(|attr| attr.value.trim().parse().ok())
        .unwrap_or(1)
}

pub(crate) fn import(node: &MarkdownNode, defs: &Defs) -> DetachedBlock {
    let parsed = node.data::<HtmlTable>().expect("HTML table layout");
    let root = html_fragment(node.as_markdown());
    let table = table_element(&root).expect("imported HTML table");
    let mut source = Source {
        html: node.as_markdown().into(),
        rows: Vec::new(),
        groups: Vec::new(),
    };
    let groups = group_elements(&table);
    let mut input = BufferQueue::default();
    input.push_back(node.as_markdown().into());
    let mut tokenizer = Tokenizer::new(SourceGroups::default(), Default::default());
    let _ = tokenizer.feed(&mut input);
    tokenizer.end();
    let mut source_groups = tokenizer.sink.groups;
    source_groups.sort_by_key(|(tag, _)| match tag.as_ref() {
        "thead" => 0,
        "tfoot" => 2,
        _ => 1,
    });
    let aligned = groups
        .iter()
        .map(tag)
        .eq(source_groups.iter().map(|(tag, _)| tag.as_ref()));
    let mut rows = Vec::new();
    let mut visual_cells = parsed.cells.iter();
    for (group, element) in groups.iter().enumerate() {
        source.groups.push(RowGroup {
            header: tag(element) == "thead",
            implied: aligned && source_groups[group].1,
        });
        for row in group_rows(element) {
            let original = source.rows.len();
            let mut original_cells = Vec::new();
            let mut children = Vec::new();
            for (column, element) in row_cells(&row).into_iter().enumerate() {
                let visual = visual_cells
                    .next()
                    .expect("HTML grid and source use the same row order");
                let cell = Cell {
                    original: Some((original, column)),
                    rowspan: span(&element, "rowspan"),
                    colspan: span(&element, "colspan").clamp(1, 1000),
                    header: visual.header,
                    align: visual.align,
                };
                let inline = import_content(&element, visual, defs, &parsed.extensions);
                let block = DetachedBlock::leaf(
                    BlockKindData::HtmlTableCell(cell.clone()),
                    BlockContent::Inline(inline.clone()),
                );
                original_cells.push((block.id, cell, inline));
                children.push(block);
            }
            let block = DetachedBlock::container(
                BlockKindData::HtmlTableRow(Row {
                    group,
                    original: Some(original),
                }),
                children,
            );
            source.rows.push(OriginalRow {
                id: block.id,
                cells: original_cells,
            });
            rows.push(block);
        }
    }
    DetachedBlock::container(BlockKindData::HtmlTable(Arc::new(source)), rows)
}

/// The DOM inserts tbody for bare rows. Record whether that tag was present
/// in the input so that saving a command can keep the original row structure.
#[derive(Default)]
struct SourceGroups {
    table_depth: usize,
    in_group: bool,
    groups: Vec<(html5ever::LocalName, bool)>,
}

impl TokenSink for SourceGroups {
    type Handle = ();

    fn process_token(&mut self, token: Token, _: u64) -> TokenSinkResult<()> {
        if let Token::TagToken(tag) = token {
            match (tag.kind, tag.name.as_ref(), self.table_depth) {
                (StartTag, "table", _) => self.table_depth += 1,
                (EndTag, "table", depth) if depth > 0 => {
                    self.table_depth -= 1;
                    if self.table_depth == 0 {
                        self.in_group = false;
                    }
                }
                (StartTag, "thead" | "tbody" | "tfoot", 1) => {
                    self.groups.push((tag.name, false));
                    self.in_group = true;
                }
                (EndTag, "thead" | "tbody" | "tfoot", 1)
                | (StartTag, "caption" | "colgroup", 1) => self.in_group = false,
                (StartTag, "tr" | "td" | "th", 1) if !self.in_group => {
                    self.groups.push((local_name!("tbody"), true));
                    self.in_group = true;
                }
                _ => {}
            }
        }
        TokenSinkResult::Continue
    }
}

// Keep HTML subtrees opaque when projecting them into Markdown would discard
// tags or attributes. The table owns their source and other cells remain editable.
fn import_content(
    element: &Handle,
    visual: &html_table::Cell,
    defs: &Defs,
    extensions: &Arc<MarkdownExtensions>,
) -> InlineText {
    let children = element.children.borrow();
    let is_html = |child: &Handle| match &child.data {
        NodeData::Text { .. } => false,
        NodeData::Element { attrs, .. } => tag(child) != "br" || !attrs.borrow().is_empty(),
        _ => true,
    };
    if !children.iter().any(is_html) {
        return inline_fragment(
            serialize_node(element, false),
            visual.blocks.clone(),
            defs,
            extensions,
        );
    }

    let mut inline = InlineText::new();
    for chunk in children.chunk_by(|a, b| is_html(a) == is_html(b)) {
        let html = is_html(&chunk[0]);
        let source = chunk
            .iter()
            .map(|node| serialize_node(node, true))
            .collect::<String>();
        // An HTML island remains a unit. Markdown before and after it still
        // participates in normal inline editing, including boundary spaces.
        let trimmed = source.trim_matches([' ', '\t']);
        let blocks = super::math::parse_markdown(trimmed, extensions.clone())
            .expect("the HTML table parser accepted this fragment");
        if html {
            inline.push(raw_fragment(source, blocks, extensions));
        } else {
            let leading = source.len() - source.trim_start_matches([' ', '\t']).len();
            let trailing = (source.len() - source.trim_end_matches([' ', '\t']).len())
                .min(source.len() - leading);
            inline.push(InlineText::plain(&source[..leading], TextMark::default()));
            inline.push(inline_fragment(
                trimmed.to_owned(),
                blocks,
                defs,
                extensions,
            ));
            inline.push(InlineText::plain(
                &source[source.len() - trailing..],
                TextMark::default(),
            ));
        }
    }
    inline
}

fn inline_fragment(
    source: String,
    blocks: Vec<BlockNode>,
    defs: &Defs,
    extensions: &Arc<MarkdownExtensions>,
) -> InlineText {
    match blocks.as_slice() {
        [] => InlineText::new(),
        [BlockNode::Paragraph(paragraph)] => import_inline(paragraph, defs),
        _ => raw_fragment(source, blocks, extensions),
    }
}

struct Fragment {
    blocks: Vec<BlockNode>,
    extensions: Arc<MarkdownExtensions>,
}

fn raw_fragment(
    source: String,
    blocks: Vec<BlockNode>,
    extensions: &Arc<MarkdownExtensions>,
) -> InlineText {
    let node = MarkdownNode::new(
        CellFragment.name(),
        Fragment {
            blocks,
            extensions: extensions.clone(),
        },
    )
    .text(source.clone())
    .markdown(source);
    InlineText::atom(InlineAtom::Custom(node), TextMark::default())
}

pub(crate) struct CellFragment;

impl MarkdownPlugin for CellFragment {
    fn name(&self) -> &str {
        "editor-html-table-cell-fragment"
    }

    fn parse(
        &self,
        _: &markdown::mdast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        // The cell importer creates these from complete HTML subtrees.
        None
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        _: &InlineRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let fragment = node.data::<Fragment>().expect("HTML cell fragment");
        let context =
            RenderContext::from_app(cx).with_markdown_extensions(fragment.extensions.clone());
        let nodes: Vec<_> = fragment.blocks.iter().collect();
        Some(InlineElement::new(div().children(
            nodes.iter().enumerate().map(|(index, block)| {
                block.render(
                    index,
                    FlowPosition::at(&nodes, index, index + 1 == nodes.len()),
                    &context,
                    window,
                    cx,
                )
            }),
        )))
    }
}

pub(crate) fn project(doc: &EditorDocument, table: BlockId, placeholders: bool) -> MarkdownNode {
    let grid = Grid::new(doc, table);
    let cells = grid
        .cells
        .into_iter()
        .map(|placed| {
            let cell = cell_kind(doc, placed.id);
            let content = doc.inline(placed.id).expect("HTML cell is an inline leaf");
            html_table::Cell::projected(
                (placed.rows.start as i16 + 1)..(placed.rows.end as i16 + 1),
                (placed.columns.start as i16 + 1)..(placed.columns.end as i16 + 1),
                super::direct::paragraph(content, placed.id, placeholders),
                cell.align,
                cell.header,
                placed.id,
            )
        })
        .collect();
    MarkdownNode::new(
        html_table::NAME,
        HtmlTable {
            cells,
            columns: grid.columns as u16,
            rows: grid.rows.len() as u16,
            extensions: doc.extensions.clone(),
            table_id: Some(table),
        },
    )
}

fn serialize_node(node: &Handle, include_node: bool) -> String {
    let mut output = Vec::new();
    let options = html5ever::serialize::SerializeOpts {
        traversal_scope: if include_node {
            html5ever::serialize::TraversalScope::IncludeNode
        } else {
            html5ever::serialize::TraversalScope::ChildrenOnly(None)
        },
        ..Default::default()
    };
    serialize(
        &mut output,
        &SerializableHandle::from(node.clone()),
        options,
    )
    .expect("serialization to memory");
    String::from_utf8(output).expect("HTML serializer writes UTF-8")
}

/// Serialize the live rows and cells into the original HTML tree so wrappers,
/// group order, attributes and unrelated children survive document edits.
pub(crate) fn write(doc: &EditorDocument, table: BlockId, source: &Source) -> String {
    let rows = doc.children(table);
    let unchanged = rows.len() == source.rows.len()
        && rows.iter().zip(&source.rows).all(|(&id, original)| {
            let cells = doc.children(id);
            id == original.id
                && cells.len() == original.cells.len()
                && cells
                    .iter()
                    .zip(&original.cells)
                    .all(|(&id, (old_id, kind, value))| {
                        id == *old_id && cell_kind(doc, id) == kind && doc.inline(id) == Some(value)
                    })
        });
    if unchanged {
        return source.html.to_string();
    }
    let root = html_fragment(&source.html);
    let element = table_element(&root).expect("imported HTML table");
    let groups = group_elements(&element);
    let original_rows: Vec<_> = groups.iter().flat_map(group_rows).collect();
    let original_cells: Vec<_> = original_rows.iter().map(row_cells).collect();
    let mut rendered_groups = vec![Vec::new(); groups.len()];
    for &id in rows {
        let row = row_kind(doc, id);
        let row_node = if let Some(original) = row.original {
            original_rows[original].clone()
        } else {
            fresh_element(local_name!("tr"))
        };
        let cells = doc
            .children(id)
            .iter()
            .map(|&id| {
                let cell = cell_kind(doc, id);
                let inline = doc.inline(id).expect("HTML cell content");
                let (cell_node, previous) = if let Some((r, c)) = cell.original {
                    (original_cells[r][c].clone(), Some(&source.rows[r].cells[c]))
                } else {
                    (
                        fresh_element(if cell.header {
                            local_name!("th")
                        } else {
                            local_name!("td")
                        }),
                        None,
                    )
                };
                if previous.is_none_or(|(_, kind, _)| kind.rowspan != cell.rowspan) {
                    set_span(&cell_node, local_name!("rowspan"), cell.rowspan);
                }
                if previous.is_none_or(|(_, kind, _)| kind.colspan != cell.colspan) {
                    set_span(&cell_node, local_name!("colspan"), cell.colspan);
                }
                if previous.is_none_or(|(_, _, value)| value != inline) {
                    cell_node.children.replace(serialize_inline(inline));
                }
                cell_node
            })
            .collect();
        replace_children(&row_node, |node| matches!(tag(node), "td" | "th"), cells);
        rendered_groups[row.group].push(row_node);
    }
    for (group, rows) in groups.iter().zip(rendered_groups) {
        replace_children(group, |node| tag(node) == "tr", rows);
    }
    if source.groups.iter().any(|group| group.implied) {
        let children = element.children.take();
        for child in children {
            if groups
                .iter()
                .zip(&source.groups)
                .any(|(node, group)| group.implied && std::rc::Rc::ptr_eq(node, &child))
            {
                element.children.borrow_mut().extend(child.children.take());
            } else {
                element.children.borrow_mut().push(child);
            }
        }
    }
    serialize_node(&root, false)
}

fn serialize_inline(inline: &InlineText) -> Vec<Handle> {
    let markdown = |range| {
        let source = super::serialize::inline_markdown(&inline.slice(range))
            .replace("\\<", "\\&lt;")
            .replace("\\&", "\\&amp;")
            .replace('\n', "<br>");
        html_fragment(&source).children.take()
    };
    let mut children = Vec::new();
    let mut start = 0;
    for (range, _, atom) in inline.pieces() {
        if let Some(InlineAtom::Custom(node)) = atom
            && node.name() == CellFragment.name()
        {
            children.extend(markdown(start..range.start));
            children.extend(html_fragment(node.as_markdown()).children.take());
            start = range.end;
        }
    }
    children.extend(markdown(start..inline.len()));
    children
}

fn fresh_element(name: html5ever::LocalName) -> Handle {
    markup5ever_rcdom::Node::new(NodeData::Element {
        name: QualName::new(None, ns!(html), name),
        attrs: Default::default(),
        template_contents: Default::default(),
        mathml_annotation_xml_integration_point: false,
    })
}

fn replace_children(node: &Handle, replaces: impl Fn(&Handle) -> bool, children: Vec<Handle>) {
    // Reuse element slots while leaving comments and inter-element whitespace in place.
    let mut next = children.into_iter();
    let old = node.children.take();
    let mut new: Vec<_> = old
        .into_iter()
        .filter_map(|child| {
            if replaces(&child) {
                next.next()
            } else {
                Some(child)
            }
        })
        .collect();
    new.extend(next);
    node.children.replace(new);
}

fn set_span(node: &Handle, name: html5ever::LocalName, value: usize) {
    let NodeData::Element { attrs, .. } = &node.data else {
        unreachable!()
    };
    let mut attrs = attrs.borrow_mut();
    if let Some(index) = attrs.iter().position(|attr| attr.name.local == name) {
        if value == 1 {
            attrs.remove(index);
        } else {
            attrs[index].value = value.to_string().into();
        }
    } else if value != 1 {
        attrs.push(Attribute {
            name: QualName::new(None, ns!(), name),
            value: value.to_string().into(),
        });
    }
}

fn empty_cell(header: bool) -> DetachedBlock {
    DetachedBlock::leaf(
        BlockKindData::HtmlTableCell(Cell {
            original: None,
            rowspan: 1,
            colspan: 1,
            align: if header {
                TextAlign::Center
            } else {
                TextAlign::Left
            },
            header,
        }),
        BlockContent::Inline(InlineText::new()),
    )
}

impl BlockEditorModel {
    /// Adjust only the tracks affected by the command. Reparenting a
    /// surviving rowspan preserves both its contents and its editor identity.
    pub(crate) fn edit_html_table(
        &mut self,
        table: BlockId,
        line: TableLine,
        edit: TableEdit,
    ) -> Result<bool, CommandError> {
        let Some(BlockKindData::HtmlTable(source)) = self.doc.kind(table) else {
            return Ok(false);
        };
        let grid = Grid::new(&self.doc, table);
        let valid = matches!(
            (line, edit),
            (
                TableLine::Row(_),
                TableEdit::AddRowAbove | TableEdit::AddRowBelow | TableEdit::RemoveRow
            ) | (
                TableLine::Column(_),
                TableEdit::AddColumnLeft | TableEdit::AddColumnRight | TableEdit::RemoveColumn
            )
        );
        if !valid {
            return Ok(false);
        }
        if matches!((line, edit), (TableLine::Row(_), TableEdit::RemoveRow) if grid.rows.len() == 1)
            || matches!((line, edit), (TableLine::Column(_), TableEdit::RemoveColumn) if grid.columns == 1)
        {
            self.set_selection(BlockSelection::Blocks {
                anchor: table,
                head: table,
            })?;
            return self.run(super::command::EditorCommand::DeleteSelection);
        }
        let groups = source.groups.clone();
        let origin = EditOrigin::Command("html_table_edit");
        self.transact(origin, |tx| {
            match (line, edit) {
                (TableLine::Row(r), TableEdit::AddRowAbove | TableEdit::AddRowBelow) => {
                    let at = r + usize::from(edit == TableEdit::AddRowBelow);
                    let group = row_kind(tx.doc, grid.rows[r]).group;
                    let mut covered = vec![false; grid.columns];
                    for cell in &grid.cells {
                        if row_kind(tx.doc, grid.rows[cell.rows.start]).group != group
                            || cell.rows.start >= at
                        {
                            continue;
                        }
                        let mut data = cell_kind(tx.doc, cell.id).clone();
                        if cell.rows.end > at || data.rowspan == 0 && cell.rows.end == at {
                            covered[cell.columns.clone()].fill(true);
                            if data.rowspan != 0 {
                                data.rowspan = cell.rows.len() + 1;
                                tx.set_kind(cell.id, BlockKindData::HtmlTableCell(data))?;
                            }
                        } else if cell.rows.end == at && data.rowspan > cell.rows.len() {
                            // A positive span clipped by its group would
                            // otherwise expand into a row appended to the group.
                            data.rowspan = cell.rows.len();
                            tx.set_kind(cell.id, BlockKindData::HtmlTableCell(data))?;
                        }
                    }
                    let cells = covered
                        .iter()
                        .filter(|covered| !**covered)
                        .map(|_| empty_cell(groups[group].header))
                        .collect();
                    let row = DetachedBlock::container(
                        BlockKindData::HtmlTableRow(Row {
                            group,
                            original: None,
                        }),
                        cells,
                    );
                    tx.insert(table, at, vec![row])?;
                }
                (TableLine::Row(r), TableEdit::RemoveRow) => {
                    for cell in grid
                        .cells
                        .iter()
                        .filter(|cell| cell.rows.contains(&r) && cell.rows.len() > 1)
                    {
                        let mut data = cell_kind(tx.doc, cell.id).clone();
                        if data.rowspan != 0 {
                            data.rowspan = cell.rows.len() - 1;
                            tx.set_kind(cell.id, BlockKindData::HtmlTableCell(data))?;
                        }
                        if cell.rows.start == r {
                            let target = grid.rows[r + 1];
                            let at = tx
                                .doc
                                .children(target)
                                .iter()
                                .position(|id| {
                                    grid.cells
                                        .iter()
                                        .find(|other| other.id == *id)
                                        .expect("original cell")
                                        .columns
                                        .start
                                        > cell.columns.start
                                })
                                .unwrap_or(tx.doc.children(target).len());
                            tx.move_block(cell.id, target, at)?;
                        }
                    }
                    tx.remove_block(grid.rows[r])?;
                }
                (TableLine::Column(c), TableEdit::AddColumnLeft | TableEdit::AddColumnRight) => {
                    let at = c + usize::from(edit == TableEdit::AddColumnRight);
                    let mut covered = vec![false; grid.rows.len()];
                    for cell in grid
                        .cells
                        .iter()
                        .filter(|cell| cell.columns.start < at && at < cell.columns.end)
                    {
                        let mut data = cell_kind(tx.doc, cell.id).clone();
                        data.colspan += 1;
                        tx.set_kind(cell.id, BlockKindData::HtmlTableCell(data))?;
                        covered[cell.rows.clone()].fill(true);
                    }
                    for (r, &row) in grid.rows.iter().enumerate().filter(|(r, _)| !covered[*r]) {
                        let group = row_kind(tx.doc, row).group;
                        // Fill skipped coordinates on ragged rows so the new cell
                        // actually occupies the inserted track in the parsed HTML.
                        let mut blank_columns = (0..at)
                            .filter(|column| grid.at(r, *column).is_none())
                            .count();
                        let index = grid
                            .cells
                            .iter()
                            .filter(|cell| cell.rows.start == r && cell.columns.start < at)
                            .count();
                        if index < tx.doc.children(row).len() {
                            blank_columns = 0;
                        }
                        tx.insert(
                            row,
                            index,
                            (0..=blank_columns)
                                .map(|_| empty_cell(groups[group].header))
                                .collect(),
                        )?;
                    }
                }
                (TableLine::Column(c), TableEdit::RemoveColumn) => {
                    for cell in grid.cells.iter().filter(|cell| cell.columns.contains(&c)) {
                        if cell.columns.len() == 1 {
                            tx.remove_block(cell.id)?;
                        } else {
                            let mut data = cell_kind(tx.doc, cell.id).clone();
                            data.colspan -= 1;
                            tx.set_kind(cell.id, BlockKindData::HtmlTableCell(data))?;
                        }
                    }
                }
                _ => unreachable!(),
            }
            let next = Grid::new(tx.doc, table);
            let target = match line {
                TableLine::Row(r) => next
                    .cells
                    .iter()
                    .find(|cell| cell.rows.contains(&r.min(next.rows.len() - 1))),
                TableLine::Column(c) => next.cells.iter().find(|cell| {
                    cell.columns
                        .contains(&c.min(next.columns.saturating_sub(1)))
                }),
            }
            .or_else(|| next.cells.first());
            Ok(target.map(|cell| BlockSelection::caret(BlockPosition::new(cell.id, 0))))
        })?;
        Ok(true)
    }
}
