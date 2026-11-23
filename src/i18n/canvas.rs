use super::{BiText, Locale, locale};

// View modes
pub const VIEW_FREEFORM: BiText = BiText::new("自由画布", "Freeform");
pub const VIEW_TREE_HORIZONTAL: BiText = BiText::new("水平导图", "Horizontal");
pub const VIEW_TREE_VERTICAL: BiText = BiText::new("垂直导图", "Vertical");
pub const TOOLTIP_VIEW_FREEFORM: BiText = BiText::new(
    "自由画布：任意拖拽位置；按住 Alt 拖拽节点可重组层级",
    "Freeform view: drag freely; hold Alt while dragging to reparent/reorder",
);
pub const TOOLTIP_VIEW_TREE_HORIZONTAL: BiText = BiText::new(
    "水平树状导图：从左向右自动排版，拖拽节点到目标上/中/下可调序或挂为子节点",
    "Horizontal tree map: left-to-right auto-layout; drop onto top/middle/bottom to reorder or reparent",
);
pub const TOOLTIP_VIEW_TREE_VERTICAL: BiText = BiText::new(
    "垂直树状导图：从上向下自动排版，拖拽节点到目标左/中/右可调序或挂为子节点",
    "Vertical tree map: top-to-bottom auto-layout; drop onto left/middle/right to reorder or reparent",
);

// Docked toolbar actions & tooltips
pub const TOOLTIP_NEW_NODE: BiText = BiText::new(
    "新建节点 (或双击画布空白处)",
    "New node (or double-click canvas)",
);
pub const TOOLTIP_INSERT_IMAGE: BiText = BiText::new(
    "插入本地图片 (png/jpg/webp/gif/svg)",
    "Insert local image (png/jpg/webp/gif/svg)",
);
pub const TOOLTIP_UNDO: BiText = BiText::new("撤销 (Ctrl+Z)", "Undo (Ctrl+Z)");
pub const TOOLTIP_REDO: BiText = BiText::new("重做 (Ctrl+Y / Ctrl+Shift+Z)", "Redo (Ctrl+Y / Ctrl+Shift+Z)");
pub const TOOLTIP_SELECT_ALL: BiText = BiText::new("全选可见节点 (Ctrl+A)", "Select all visible nodes (Ctrl+A)");

pub const EDGE_KIND_SOLID_SHORT: BiText = BiText::new("实线", "Solid");
pub const EDGE_KIND_DASHED_SHORT: BiText = BiText::new("虚线", "Dashed");
pub const TOOLTIP_EDGE_KIND_TOGGLE: BiText = BiText::new(
    "切换拖拽圆点默认连线类型（实线=父子层级，虚线=关联；按住 Shift 拖拽可直接画父子实线）",
    "Toggle default port-drag edge type (Solid=Hierarchy, Dashed=Reference; hold Shift to draw Solid)",
);

// Contextual node actions
pub const TOOLTIP_ADD_CHILD: BiText = BiText::new("添加子节点 (Tab)", "Add child node (Tab)");
pub const TOOLTIP_ADD_SIBLING: BiText = BiText::new("添加同级兄弟节点 (Enter)", "Add sibling node (Enter)");
pub const TOOLTIP_EDIT_NODE: BiText = BiText::new("编辑内容或图片说明 (F2)", "Edit content or caption (F2)");
pub const TOOLTIP_DETACH_NODE: BiText = BiText::new("脱离父节点", "Detach from parent");
pub const TOOLTIP_COLOR_PICKER: BiText = BiText::new("节点/连线颜色", "Node / Edge color");
pub const COLOR_CLEAR: BiText = BiText::new("默认", "Reset");
pub const TOOLTIP_DELETE_SELECTION: BiText = BiText::new("删除选中项 (Delete / Backspace)", "Delete selected (Delete / Backspace)");

// Selected edge floating bar
pub const EDGE_TO_SOLID: BiText = BiText::new("父子实线", "Hierarchy");
pub const EDGE_TO_DASHED: BiText = BiText::new("关联虚线", "Reference");
pub const TOOLTIP_EDGE_TOGGLE_KIND: BiText = BiText::new(
    "切换实线（父子层级）与虚线（交叉关联）",
    "Switch between solid (hierarchy) and dashed (reference)",
);
pub const TOOLTIP_EDGE_REVERSE: BiText = BiText::new("反转连线方向", "Reverse direction");
pub const TOOLTIP_EDGE_DELETE: BiText = BiText::new("删除连线 (Delete)", "Delete edge (Delete)");

// Zoom controls
pub const TOOLTIP_ZOOM_OUT: BiText = BiText::new("缩小 (Ctrl+-)", "Zoom out (Ctrl+-)");
pub const TOOLTIP_ZOOM_FIT: BiText = BiText::new("适配内容 (Ctrl+0)", "Fit content (Ctrl+0)");
pub const TOOLTIP_ZOOM_IN: BiText = BiText::new("放大 (Ctrl+=)", "Zoom in (Ctrl+=)");
pub const TOOLTIP_ZOOM_100: BiText = BiText::new("一键恢复 100% 缩放", "Reset zoom to 100%");

// Canvas hints & inline placeholders
pub const NODE_DOUBLE_CLICK_EDIT: BiText = BiText::new("双击编辑内容…", "Double-click to edit…");
pub const EMPTY_CANVAS_HINT: BiText = BiText::new(
    "双击空白处或点击左上「+」新建 · 右键拖动画布 · 左键框选 · 拖拽圆点连线 · Ctrl+V 粘贴图片",
    "Double-click or click '+' to add · Right-drag to pan · Left-drag to select · Drag ports to connect · Ctrl+V pastes images",
);

// Image dialog
pub const DIALOG_INSERT_IMAGE: BiText = BiText::new("插入图片到画布", "Insert Image to Canvas");
pub const IMAGE_FILTER_LABEL: BiText = BiText::new("图片文件", "Image Files");

pub fn anchor_kind_label(kind: &str) -> &'static str {
    match (locale(), kind) {
        (Locale::ZhCn, "doc") => "文档",
        (Locale::ZhCn, "section") => "章节",
        (Locale::ZhCn, "quote") => "摘录",
        (Locale::ZhCn, "pdf-page") => "PDF页",
        (Locale::ZhCn, "pdf-annot") => "批注",
        (Locale::En, "doc") => "Doc",
        (Locale::En, "section") => "Section",
        (Locale::En, "quote") => "Quote",
        (Locale::En, "pdf-page") => "PDF Page",
        (Locale::En, "pdf-annot") => "Annotation",
        (_, "file") => BiText::new("文件", "File").get(),
        _ => BiText::new("引用", "Ref").get(),
    }
}

// Errors
pub const ERR_PARSE_LOCKED: BiText = BiText::new(
    "当前画布 JSON 解析异常，请先修复或重新加载后再操作。",
    "Canvas JSON failed to parse; fix or reload before editing.",
);
pub const ERR_SELF_LOOP: BiText = BiText::new(
    "节点不能连接到自身。",
    "Cannot connect a node to itself.",
);
pub const ERR_NODE_MISSING: BiText = BiText::new("节点不存在。", "Node does not exist.");
pub const ERR_EDGE_MISSING: BiText = BiText::new("连线不存在。", "Edge does not exist.");
pub const ERR_CYCLE_DESCENDANT: BiText = BiText::new(
    "不能将父节点连接到其后代节点（会形成环路）。",
    "Cannot connect a parent to its descendant (would create a cycle).",
);
pub const ERR_SELF_LOOP_HIERARCHY: BiText = BiText::new(
    "自环不能作为父子层级边。",
    "Self-loop cannot be a hierarchy edge.",
);
pub const ERR_CYCLE_CONVERT_HIERARCHY: BiText = BiText::new(
    "转为父子实线会形成环路，请先反转方向或调整结构。",
    "Converting to a hierarchy edge would create a cycle; reverse direction or adjust first.",
);
pub const ERR_REORDER_TARGET_MISSING: BiText = BiText::new("目标节点不存在。", "Target node does not exist.");
pub const ERR_CYCLE_REPARENT: BiText = BiText::new(
    "不能将节点拖入自身或后代子树。",
    "Cannot move a node into itself or its own subtree.",
);
pub const ERR_SELF_PARENT: BiText = BiText::new(
    "节点不能成为自身的父节点。",
    "A node cannot be its own parent.",
);

pub fn err_json_parse(detail: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("画布 JSON 解析失败：{detail}"),
        Locale::En => format!("Canvas JSON parse failed: {detail}"),
    }
}

pub fn err_create_image_dir(detail: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("创建图片目录失败：{detail}"),
        Locale::En => format!("Failed to create image directory: {detail}"),
    }
}

pub fn err_copy_image(detail: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("复制图片失败：{detail}"),
        Locale::En => format!("Failed to copy image: {detail}"),
    }
}

pub fn err_duplicate_node(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("节点 ID 已存在：{id}"),
        Locale::En => format!("Node ID already exists: {id}"),
    }
}

pub fn err_missing_parent(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("父节点不存在：{id}"),
        Locale::En => format!("Parent node does not exist: {id}"),
    }
}

pub fn err_missing_node(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("节点不存在：{id}"),
        Locale::En => format!("Node does not exist: {id}"),
    }
}

pub fn err_missing_target_parent(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("目标父节点不存在：{id}"),
        Locale::En => format!("Target parent node does not exist: {id}"),
    }
}

pub fn err_missing_source_node(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("起点节点不存在：{id}"),
        Locale::En => format!("Source node does not exist: {id}"),
    }
}

pub fn err_missing_target_node(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("终点节点不存在：{id}"),
        Locale::En => format!("Target node does not exist: {id}"),
    }
}

pub fn err_duplicate_edge(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("连线 ID 已存在：{id}"),
        Locale::En => format!("Edge ID already exists: {id}"),
    }
}

pub fn err_missing_edge(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("连线不存在：{id}"),
        Locale::En => format!("Edge does not exist: {id}"),
    }
}

pub fn err_cycle_detected(id: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("检测到树形父子环路，涉及节点：{id}"),
        Locale::En => format!("Hierarchy cycle detected involving node: {id}"),
    }
}

// Target canvas picker (from Markdown / PDF)
pub const PICKER_TITLE: BiText = BiText::new("发送到画布", "Send to Canvas");
pub const PICKER_SUBTITLE: BiText = BiText::new(
    "选择已有画布，或在当前文档目录新建画布",
    "Choose an existing canvas or create one alongside this file",
);
pub const PICKER_OPEN_TAB_BADGE: BiText = BiText::new("已打开", "Open");
pub const PICKER_EMPTY_LIST: BiText = BiText::new(
    "当前项目中暂无 .canvas 画布文件，可在下方新建。",
    "No .canvas files in this project yet. Create one below.",
);
pub const PICKER_NEW_PLACEHOLDER: BiText = BiText::new("输入新画布文件名…", "New canvas filename…");
pub const PICKER_BTN_CREATE_AND_ADD: BiText = BiText::new("新建并添加", "Create & Add");

pub fn toast_added_to_canvas(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已添加到画布：{name}"),
        Locale::En => format!("Added to canvas: {name}"),
    }
}

pub fn pdf_page_annotation_preview(page: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("PDF 第 {page} 页批注"),
        Locale::En => format!("PDF page {page} annotation"),
    }
}

// Export & backlinks
pub const TOOLTIP_EXPORT: BiText = BiText::new(
    "导出为 PDF / SVG / PNG（未选中时默认导出全部）",
    "Export to PDF / SVG / PNG (exports all when nothing is selected)",
);
pub const ERR_EXPORT_EMPTY: BiText = BiText::new(
    "当前画布中没有可导出的内容。",
    "Nothing to export on the canvas.",
);
pub const TOOLTIP_BACKLINKS: BiText = BiText::new("查看引用此文档的知识画布", "Canvases referencing this document");
pub const BACKLINKS_MENU_HEADER: BiText = BiText::new("画布反向引用", "Canvas Backlinks");
