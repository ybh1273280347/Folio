use super::{BiText, Locale, locale};

pub const TAB_TITLE: BiText = BiText::new("知识图谱", "Knowledge Graph");
pub const TOOLTIP_OPEN_GRAPH: BiText = BiText::new("打开知识图谱", "Open Knowledge Graph");
pub const ACTION_OPEN_GRAPH: BiText = BiText::new("知识图谱", "Knowledge Graph");

pub const SEARCH_PLACEHOLDER: BiText = BiText::new("搜索节点或标签…", "Search nodes or tags…");
pub const FILTER_CANVAS: BiText = BiText::new("画布", "Canvas");
pub const FILTER_UNRESOLVED: BiText = BiText::new("悬空", "Unresolved");
pub const FILTER_ATTACHMENTS: BiText = BiText::new("附件", "Assets");
pub const FILTER_ORPHANS: BiText = BiText::new("仅孤立", "Orphans");

pub const COLOR_BY_FOLDER: BiText = BiText::new("按目录", "Folder");
pub const COLOR_BY_TAG: BiText = BiText::new("按标签", "Tag");
pub const COLOR_BY_KIND: BiText = BiText::new("按类型", "Type");

pub const TOOLTIP_REFRESH: BiText = BiText::new("刷新图谱索引", "Refresh graph index");
pub const TOOLTIP_REHEAT: BiText = BiText::new("重新整理布局", "Re-layout graph");
pub const TOOLTIP_FIT_VIEW: BiText = BiText::new("自适应全图", "Fit graph to view");
pub const TOOLTIP_ZOOM_IN: BiText = BiText::new("放大", "Zoom in");
pub const TOOLTIP_ZOOM_OUT: BiText = BiText::new("缩小", "Zoom out");
pub const TOOLTIP_ZOOM_100: BiText = BiText::new("重置 100%", "Reset 100%");

pub const KIND_MARKDOWN: BiText = BiText::new("Markdown 笔记", "Markdown Note");
pub const KIND_CANVAS: BiText = BiText::new("知识画布", "Knowledge Canvas");
pub const KIND_UNRESOLVED: BiText = BiText::new("待创建双链", "Unresolved Link");
pub const KIND_ATTACHMENT: BiText = BiText::new("附件资源", "Attachment");

pub const BTN_OPEN: BiText = BiText::new("打开", "Open");
pub const BTN_OPEN_SPLIT: BiText = BiText::new("分栏打开", "Open Split");
pub const BTN_CREATE_NOTE: BiText = BiText::new("创建笔记", "Create Note");
pub const BTN_FOCUS_NODE: BiText = BiText::new("居中", "Center");

pub const EMPTY_TITLE: BiText = BiText::new("当前空间暂无可展示的图谱节点", "No graph nodes in this workspace");
pub const EMPTY_HINT: BiText = BiText::new(
    "在 Markdown 中使用 [[双链]] 或在知识画布中添加文档锚点后，节点关系会自动出现在这里",
    "Add [[wikilinks]] in Markdown or document anchors in a Canvas to build your knowledge graph",
);

pub fn stats_summary(nodes: usize, edges: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{nodes} 节点 · {edges} 连线"),
        Locale::En => format!("{nodes} nodes · {edges} links"),
    }
}

pub fn search_matches_summary(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{count} 匹配"),
        Locale::En => format!("{count} matched"),
    }
}

pub fn degree_summary(inbound: usize, outbound: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("入链 {inbound} · 出链 {outbound}"),
        Locale::En => format!("{inbound} in · {outbound} out"),
    }
}
