use super::{BiText, Locale, locale};

pub const TAB_TITLE: BiText = BiText::new("知识图谱", "Knowledge Graph");
pub const LOCAL_PANEL_TITLE: BiText = BiText::new("局部图谱", "Local Graph");
pub const TOOLTIP_OPEN_GRAPH: BiText = BiText::new("打开知识图谱", "Open Knowledge Graph");
pub const TOOLTIP_OPEN_LOCAL_GRAPH: BiText = BiText::new("在右侧栏打开局部图谱", "Open Local Graph in Side Panel");
pub const TOOLTIP_EXPAND_GLOBAL_GRAPH: BiText = BiText::new("在全局图谱中打开", "Open in Global Graph");
pub const ACTION_OPEN_GRAPH: BiText = BiText::new("知识图谱", "Knowledge Graph");
pub const ACTION_OPEN_LOCAL_GRAPH: BiText = BiText::new("局部图谱", "Local Graph");

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

pub const BTN_OPEN: BiText = BiText::new("打开", "Open");
pub const BTN_OPEN_SPLIT: BiText = BiText::new("分栏", "Split");
pub const BTN_CREATE_NOTE: BiText = BiText::new("创建笔记", "Create Note");
pub const BTN_EXIT_LOCAL: BiText = BiText::new("退出聚焦", "Exit Focus");
pub const HOP_1: BiText = BiText::new("1跳", "1-hop");
pub const HOP_2: BiText = BiText::new("2跳", "2-hop");

pub const MENU_FOCUS_1HOP: BiText = BiText::new("聚焦邻域 (1跳)", "Focus Local (1-hop)");
pub const MENU_FOCUS_2HOP: BiText = BiText::new("聚焦邻域 (2跳)", "Focus Local (2-hop)");
pub const MENU_COPY_LINK: BiText = BiText::new("复制链接", "Copy Link");
pub const MENU_CONNECT_TO: BiText = BiText::new("连接到…（文末补链）", "Link to… (Append)");
pub const MENU_REVEAL_IN_TREE: BiText = BiText::new("在文件树中定位", "Reveal in File Tree");
pub const MENU_EDGE_JUMP: BiText = BiText::new("跳转到引用位置", "Jump to Reference");
pub const LOCAL_EMPTY_HINT: BiText = BiText::new(
    "打开一篇 Markdown 或 Canvas 文档即可在此查看其 1~2 跳关联网络",
    "Open a Markdown note or Canvas to view its 1–2 hop neighborhood here",
);
pub const ERR_APPEND_CONFLICT: BiText = BiText::new(
    "文件刚在外部被修改，请刷新后再试",
    "File was modified externally, please refresh and retry",
);

pub const EDGE_CANVAS_ANCHOR: BiText = BiText::new("画布锚点", "Canvas anchor");

pub const EMPTY_TITLE: BiText = BiText::new("当前空间暂无可展示的图谱节点", "No graph nodes in this workspace");
pub const EMPTY_HINT: BiText = BiText::new(
    "在 Markdown 中使用 [[双链]] 或在知识画布中添加文档锚点后，节点关系会自动出现在这里",
    "Add [[wikilinks]] in Markdown or document anchors in a Canvas to build your knowledge graph",
);

pub fn search_matches_summary(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{count} 匹配"),
        Locale::En => format!("{count} matched"),
    }
}

pub fn local_focus_label(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("局部 · {name}"),
        Locale::En => format!("Local · {name}"),
    }
}

pub fn edge_line_badge(line: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("第 {line} 行"),
        Locale::En => format!("Line {line}"),
    }
}

pub fn copied_link_notice(title: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已复制链接：{title}"),
        Locale::En => format!("Copied link: {title}"),
    }
}

pub fn appended_link_notice(source: &str, target: &str, saved: bool) -> String {
    match (locale(), saved) {
        (Locale::ZhCn, true) => format!("已追加双链：{source} → {target}"),
        (Locale::ZhCn, false) => format!("已追加到草稿（保存后更新图谱）：{source} → {target}"),
        (Locale::En, true) => format!("Appended link: {source} → {target}"),
        (Locale::En, false) => format!("Appended to draft (save to update graph): {source} → {target}"),
    }
}
