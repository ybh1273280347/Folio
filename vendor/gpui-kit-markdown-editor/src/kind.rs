use gpui::SharedString;

use gpui_kit::base::text::editor_host::ColumnAlign;

/// Public, payload-light projection of a block's kind, for matching in
/// toolbar code. Payloads (code language, table alignments, definition
/// targets) stay in the crate-internal `BlockKindData`.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    // containers
    Root,
    Blockquote,
    List {
        ordered: bool,
    },
    ListItem {
        checked: Option<bool>,
    },
    Table,
    TableRow,
    TableCell,
    // text-bearing leaves
    Paragraph,
    Heading {
        level: u8,
    },
    CodeBlock,
    FrontMatter,
    // atomic leaves
    /// Reserved: on main, images are inline nodes inside a paragraph, so the
    /// spike stores them as inline atoms and never creates this kind.
    Image,
    HorizontalRule,
    Break,
    Custom,
    Definition,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FoldSyntax {
    Details { opening: SharedString, summary_opening: SharedString },
    Alert { name: SharedString },
}

/// Internal kind with payload.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BlockKindData {
    Root,
    Blockquote,
    Alert { name: SharedString },
    Fold { syntax: FoldSyntax, initially_open: bool },
    List {
        ordered: bool,
        start: Option<u32>,
    },
    ListItem {
        checked: Option<bool>,
        spread: bool,
    },
    Table {
        aligns: Vec<ColumnAlign>,
    },
    HtmlTable(std::sync::Arc<super::html_table_model::Source>),
    HtmlTableRow(super::html_table_model::Row),
    HtmlTableCell(super::html_table_model::Cell),
    TableRow,
    TableCell,
    Paragraph,
    Heading {
        level: u8,
    },
    CodeBlock {
        lang: Option<SharedString>,
        meta: Option<SharedString>,
    },
    FrontMatter {
        opening: SharedString,
        // Source separator before the closing fence, including a single blank YAML line.
        yaml_ending: SharedString,
        closing: SharedString,
    },
    HorizontalRule,
    Break {
        html: bool,
    },
    Custom,
    Definition {
        identifier: SharedString,
        url: SharedString,
        title: Option<SharedString>,
    },
    Unknown,
}

impl BlockKindData {
    pub(crate) fn public(&self) -> BlockKind {
        match self {
            Self::Root => BlockKind::Root,
            Self::Blockquote | Self::Alert { .. } | Self::Fold { .. } => BlockKind::Blockquote,
            Self::List { ordered, .. } => BlockKind::List { ordered: *ordered },
            Self::ListItem { checked, .. } => BlockKind::ListItem { checked: *checked },
            Self::Table { .. } | Self::HtmlTable(_) => BlockKind::Table,
            Self::TableRow | Self::HtmlTableRow(_) => BlockKind::TableRow,
            Self::TableCell | Self::HtmlTableCell(_) => BlockKind::TableCell,
            Self::Paragraph => BlockKind::Paragraph,
            Self::Heading { level } => BlockKind::Heading { level: *level },
            Self::CodeBlock { .. } => BlockKind::CodeBlock,
            Self::FrontMatter { .. } => BlockKind::FrontMatter,
            Self::HorizontalRule => BlockKind::HorizontalRule,
            Self::Break { .. } => BlockKind::Break,
            Self::Custom => BlockKind::Custom,
            Self::Definition { .. } => BlockKind::Definition,
            Self::Unknown => BlockKind::Unknown,
        }
    }

    /// Whether this kind holds child blocks.
    pub(crate) fn is_container(&self) -> bool {
        matches!(
            self,
            Self::Root
                | Self::Blockquote
                | Self::Alert { .. }
                | Self::Fold { .. }
                | Self::List { .. }
                | Self::ListItem { .. }
                | Self::Table { .. }
                | Self::TableRow
                | Self::HtmlTable(_)
                | Self::HtmlTableRow(_)
        )
    }

    /// Whether this kind holds editable inline text.
    pub(crate) fn is_textblock(&self) -> bool {
        matches!(
            self,
            Self::Paragraph | Self::Heading { .. } | Self::TableCell | Self::HtmlTableCell(_)
        )
    }

    pub(crate) fn is_table_cell(&self) -> bool {
        matches!(self, Self::TableCell | Self::HtmlTableCell(_))
    }

    pub(crate) fn is_table(&self) -> bool {
        matches!(self, Self::Table { .. } | Self::HtmlTable(_))
    }
}
