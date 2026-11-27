use super::{BiText, Locale, locale};

// Tag colors (display names)
pub const TAG_COLOR_GRAY: BiText = BiText::new("灰色", "Gray");
pub const TAG_COLOR_BLUE: BiText = BiText::new("蓝色", "Blue");
pub const TAG_COLOR_GREEN: BiText = BiText::new("绿色", "Green");
pub const TAG_COLOR_ORANGE: BiText = BiText::new("橙色", "Orange");
pub const TAG_COLOR_RED: BiText = BiText::new("红色", "Red");
pub const TAG_COLOR_PURPLE: BiText = BiText::new("紫色", "Purple");

// Tag store errors (tags.rs)
pub const ERR_INVALID_TAG_NAME: BiText = BiText::new(
    "请输入标签名称，名称不能包含 / 或控制字符。",
    "Please enter a tag name without '/' or control characters.",
);
pub const ERR_NEWER_TAG_DB: BiText = BiText::new("标签数据库来自更新版本的 Folio。", "Tag database is from a newer version of Folio.");
pub const ERR_TAG_DB_NOT_FOUND: BiText = BiText::new("找不到标签数据库。", "Tag database not found.");
pub const ERR_TAG_ALREADY_EXISTS: BiText = BiText::new("已存在同名标签。", "A tag with this name already exists.");
pub const ERR_SELECT_SUPPORTED_FILE: BiText = BiText::new("请选择受支持的文件。", "Please select a supported file.");

// Workspace tags UI (workspace/tags.rs)
pub fn err_load_tags(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法读取标签：{error}"),
        Locale::En => format!("Failed to load tags: {error}"),
    }
}
pub fn err_refresh_tags(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法刷新标签：{error}"),
        Locale::En => format!("Failed to refresh tags: {error}"),
    }
}
pub fn err_moved_tags_not_updated(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("文件已移动，标签暂未更新：{error}"),
        Locale::En => format!("File moved, tags not yet updated: {error}"),
    }
}
pub fn err_watch_loose_dir(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法监听独立文件目录：{error}"),
        Locale::En => format!("Failed to watch loose files directory: {error}"),
    }
}
pub fn err_save_tags(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法保存标签：{error}"),
        Locale::En => format!("Failed to save tags: {error}"),
    }
}
pub const ERR_NO_SUPPORTED_FILES_IN_FOLDER: BiText = BiText::new(
    "此文件夹及其子文件夹中没有受支持的文件。",
    "No supported files found in this folder or its subfolders.",
);
pub fn dialog_delete_tag_title(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("删除标签「{name}」？"),
        Locale::En => format!("Delete tag \"{name}\"?"),
    }
}
pub const DIALOG_DELETE_TAG_BODY: BiText = BiText::new("此标签会从所有关联文件上移除。", "This tag will be removed from all associated files.");
pub const DIALOG_DELETE_TAG_CONFIRM: BiText = BiText::new("删除标签", "Delete Tag");

pub fn dialog_agent_delete_tag_body(name: &str, affected: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("「{name}」会从全部 {affected} 个关联文件上移除，删除后无法撤销。"),
        Locale::En => format!("This permanently removes \"{name}\" from all {affected} associated files."),
    }
}

pub const TAGS_TITLE: BiText = BiText::new("标签", "Tags");
pub const CLEAR_TAG_FILTER: BiText = BiText::new("清除标签筛选", "Clear tag filter");
pub fn mode_edit_files_tags(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("编辑 {count} 个文件的标签"),
        Locale::En => format!("Edit tags for {count} files"),
    }
}
pub const MODE_RENAME_TAG: BiText = BiText::new("重命名标签", "Rename Tag");
pub const MODE_RECOVER_FILE: BiText = BiText::new("选择新的关联文件", "Select New Target File");
pub const BACK_TO_TAG_LIST: BiText = BiText::new("返回标签列表", "Back to tag list");
pub const CLOSE_TAG_EDIT: BiText = BiText::new("关闭标签编辑", "Close tag editor");
pub const COLLAPSE_TAG_PANEL: BiText = BiText::new("收起标签面板", "Collapse tags panel");
pub fn folder_with_subfolders(folder: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{folder}（含子文件夹）"),
        Locale::En => format!("{folder} (incl. subfolders)"),
    }
}
pub const SAVE_TAG_NAME: BiText = BiText::new("保存标签名称", "Save tag name");
pub const OPEN_TARGET_FILE: BiText = BiText::new("打开目标文件…", "Open Target File…");
pub const PENDING_RELINK: BiText = BiText::new("待重新关联", "Missing file");
pub const RELINK_BTN: BiText = BiText::new("重新关联", "Relink");
pub const REMOVE_TAG: BiText = BiText::new("移除标签", "Remove tag");
pub const CHANGE_TAG_COLOR: BiText = BiText::new("更改标签颜色", "Change tag color");
pub const DELETE_TAG: BiText = BiText::new("删除标签", "Delete tag");
pub const EDIT_TAG_PLACEHOLDER: BiText = BiText::new("输入标签名，回车添加或新建…", "Type tag name to add or create…");
pub const NEW_TAG_BTN: BiText = BiText::new("新建标签", "New tag");
pub const NEW_TAG_PLACEHOLDER: BiText = BiText::new("输入标签名称…", "New tag name…");
pub const NO_ASSIGNED_TAGS: BiText = BiText::new("暂无标签，在下方选择或输入以创建", "No tags yet — pick below or type to create");
pub const NO_MATCHING_TAGS: BiText = BiText::new("没有匹配的标签", "No matching tags");
pub const MORE_TAG_ACTIONS: BiText = BiText::new("更多操作", "More actions");
pub const APPLY_TAG_TO_ALL: BiText = BiText::new("应用到全部选中文件", "Apply to all selected files");
pub fn create_and_add_tag(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("创建并添加 “{name}”"),
        Locale::En => format!("Create and add \"{name}\""),
    }
}

// Agent tags & annotations
pub const ERR_AGENT_TAG_MAX_QUERY: BiText = BiText::new("每次最多查询 20 个标签。", "At most 20 tags can be queried at once.");
pub const ERR_AGENT_TAG_FILTER_REQUIRED: BiText = BiText::new(
    "请至少指定 paths、under_directory、tags、name_contains 或 extension。",
    "Specify at least one of paths, under_directory, tags, name_contains, or extension.",
);
pub const ERR_AGENT_TAG_ADD_REMOVE_REQUIRED: BiText = BiText::new(
    "请指定要添加或移除的标签，每组最多 20 个。",
    "Specify tags to add or remove (at most 20 per group).",
);
pub fn err_agent_tag_both_add_remove(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("不能同时添加和移除「{name}」。"),
        Locale::En => format!("Cannot both add and remove \"{name}\"."),
    }
}
pub const ERR_AGENT_TAG_MAX_COLORS: BiText = BiText::new("一次最多指定 20 个标签颜色。", "At most 20 tag colors can be specified at once.");
pub fn err_agent_tag_color_must_be_in_add(tag: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("tag_colors 中的「{tag}」也需要出现在 add 中。"),
        Locale::En => format!("\"{tag}\" in tag_colors must also appear in add."),
    }
}
pub fn err_agent_tag_duplicate_color(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("重复指定了「{name}」的颜色。"),
        Locale::En => format!("Duplicate color specified for \"{name}\"."),
    }
}
pub const ERR_AGENT_TAG_PATHS_OR_DIR: BiText = BiText::new(
    "请在 paths 和 under_directory 中二选一。",
    "Specify either paths or under_directory, not both.",
);
pub fn err_agent_tag_path_changed(path: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("文件路径已变化：{path}"),
        Locale::En => format!("File path changed: {path}"),
    }
}
pub fn err_agent_tag_exists(new_name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已存在标签「{new_name}」。"),
        Locale::En => format!("Tag \"{new_name}\" already exists."),
    }
}
pub fn err_agent_open_dir(input: &str, error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法打开目录 {input}：{error}"),
        Locale::En => format!("Failed to open directory {input}: {error}"),
    }
}
pub fn err_agent_dir_not_in_workspace(input: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("工作区中没有此可访问目录：{input}"),
        Locale::En => format!("Directory not accessible in workspace: {input}"),
    }
}
pub const ERR_AGENT_FILE_COUNT: BiText = BiText::new("每次请选择 1 到 100 个具体文件。", "Please select 1 to 100 specific files.");
pub fn err_agent_paths_files_only(path: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("paths 仅接受文件。查询或处理目录 {path} 时请使用 under_directory。"),
        Locale::En => format!("paths only accepts files. Use under_directory for directory {path}."),
    }
}
pub fn err_agent_unsupported_file(path: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("项目文件树中没有此受支持文件：{path}"),
        Locale::En => format!("Supported file not found in project tree: {path}"),
    }
}
pub fn err_agent_tag_not_found(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("找不到标签「{name}」，请查看当前空间的标签。"),
        Locale::En => format!("Tag \"{name}\" not found; check tags in the current workspace."),
    }
}
pub const ERR_AGENT_LIMIT_RANGE: BiText = BiText::new("每页数量必须在 1 到 100 之间。", "Page limit must be between 1 and 100.");
pub const ERR_AGENT_STALE_PREVIEW: BiText = BiText::new(
    "确认期间目标文件或标签发生了变化，请重新检索并预览。",
    "Target files or tags changed during confirmation; please query and preview again.",
);
pub const ERR_AGENT_TAGS_PROJECT_OR_DEFAULT: BiText = BiText::new(
    "标签工具可用于项目和默认空间。",
    "Tag tools are available in projects and the default workspace.",
);
pub const ERR_AGENT_SWITCH_PROJECT_RETRY: BiText = BiText::new(
    "请切回发起会话所属的项目后重试。",
    "Please switch back to the project where this session started and retry.",
);
pub const ERR_AGENT_STALE_TAG_CHANGES: BiText = BiText::new(
    "这批标签变更已失效，请重新预览。",
    "These tag changes are stale; please preview again.",
);

// Annotations (annotations.rs & workspace/annotations.rs)
pub const ERR_ANN_OUTSIDE_PROJECT: BiText = BiText::new(
    "批注工具只能访问当前项目内的文件。",
    "Annotation tools can only access files inside the current project.",
);
pub const ERR_ANN_UNSUPPORTED_FORMAT: BiText = BiText::new(
    "首版只支持 Markdown 和有文字层的 PDF 文件。",
    "Only Markdown and text-layer PDF files are supported.",
);
pub const ERR_ANN_QUOTE_NOT_FOUND: BiText = BiText::new(
    "未找到原文。请使用单个文字块中的实际原文；不支持跨块、公式或图片定位。",
    "Quote not found. Use exact text within a single block; cross-block, math, or image anchors are unsupported.",
);
pub const ERR_ANN_AMBIGUOUS_MATCH: BiText = BiText::new(
    "原文匹配多处，请增加原文上下文或指定 occurrence。",
    "Quote matched multiple locations; add more context or specify occurrence.",
);
pub fn err_ann_invalid_occurrence(max: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("occurrence 应在 1 到 {max} 之间。"),
        Locale::En => format!("occurrence must be between 1 and {max}."),
    }
}
pub const ERR_ANN_EMPTY_QUOTE: BiText = BiText::new("quote 必须是非空原文。", "quote must be non-empty.");
pub const ERR_ANN_EMPTY_COMMENT: BiText = BiText::new("comment 必须是非空批注正文。", "comment must be non-empty.");
pub const ERR_ANN_MD_NO_PAGE: BiText = BiText::new("Markdown 不使用 page 参数。", "Markdown does not use the page parameter.");
pub const ERR_ANN_UNSUPPORTED_TARGET: BiText = BiText::new(
    "此处不是可高亮的普通文字块，请选择段落、标题或表格单元格中的原文。",
    "Target is not a highlightable text block; select text in a paragraph, heading, or table cell.",
);
pub const ERR_ANN_PDF_PAGE_REQUIRED: BiText = BiText::new(
    "PDF 必须指定从 1 开始的实际文件页码。",
    "PDF requires a 1-based page number.",
);
pub fn err_ann_pdf_parse(error: &dyn std::fmt::Debug) -> String {
    match locale() {
        Locale::ZhCn => format!("无法解析 PDF：{error:?}"),
        Locale::En => format!("Failed to parse PDF: {error:?}"),
    }
}
pub fn err_ann_pdf_page_count(pages: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("PDF 只有 {pages} 页。"),
        Locale::En => format!("PDF only has {pages} pages."),
    }
}
pub const ERR_ANN_PDF_NO_TEXT_LAYER: BiText = BiText::new(
    "该页没有可用原生文字层，首版不支持扫描页或 OCR 坐标定位。",
    "This page has no native text layer; scanned pages and OCR coordinates are unsupported.",
);
pub const ERR_ANN_ALREADY_ANNOTATED: BiText = BiText::new(
    "此位置已有不同的评论，未覆盖原批注。",
    "This location already has a different comment and was not overwritten.",
);
pub const ERR_ANN_UNKNOWN_ACTION: BiText = BiText::new("未知批注操作。", "Unknown annotation action.");
pub const ERR_ANN_ID_NOT_FOUND: BiText = BiText::new(
    "此文件中没有该 ID 的批注，请重新列出批注。",
    "This file has no annotation with that ID. List its annotations again.",
);
pub const ERR_ANN_REMOVE_HIGHLIGHT_ONLY: BiText = BiText::new(
    "Markdown 高亮已移除，但批注记录未能删除。",
    "The Markdown highlight was removed, but the annotation record could not be deleted.",
);
pub const ERR_ANN_DOC_CONFLICT: BiText = BiText::new(
    "文档有磁盘冲突或已被删除，请先处理后重试。",
    "Document has a disk conflict or was deleted; resolve it and retry.",
);
pub const ERR_ANN_PATH_CHANGED: BiText = BiText::new("文件位置已变化，请重新读取后重试。", "File path changed; reload and retry.");
pub const ERR_ANN_DOC_CHANGED: BiText = BiText::new(
    "批注准备期间文档已变化，请重新读取后重试。",
    "Document changed while preparing annotation; reload and retry.",
);
pub const ERR_ANN_PDF_CHANGED: BiText = BiText::new("PDF 已变化，请重新读取后重试。", "PDF changed; reload and retry.");
pub const ERR_ANN_DISK_CHANGED: BiText = BiText::new(
    "磁盘内容已变化，未覆盖外部修改。",
    "Disk content changed; external edits were not overwritten.",
);
pub const ERR_ANN_SAVE_HIGHLIGHT_ONLY: BiText = BiText::new(
    "正文高亮已保存，但批注记录保存失败",
    "Highlight saved in text, but failed to save annotation record",
);
pub const ERR_ANN_SAVE_RECORD: BiText = BiText::new("批注记录保存失败", "Failed to save annotation record");
