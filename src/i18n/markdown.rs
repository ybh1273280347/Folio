use super::{BiText, Locale, locale};

// Slash command titles
pub const SLASH_TEXT: BiText = BiText::new("正文段落", "Paragraph");
pub const SLASH_H1: BiText = BiText::new("一级标题", "Heading 1");
pub const SLASH_H2: BiText = BiText::new("二级标题", "Heading 2");
pub const SLASH_H3: BiText = BiText::new("三级标题", "Heading 3");
pub const SLASH_H4: BiText = BiText::new("四级标题", "Heading 4");
pub const SLASH_BULLET: BiText = BiText::new("无序列表", "Bulleted List");
pub const SLASH_ORDERED: BiText = BiText::new("有序列表", "Numbered List");
pub const SLASH_TODO: BiText = BiText::new("任务待办", "To-do List");
pub const SLASH_QUOTE: BiText = BiText::new("引用块", "Quote");
pub const SLASH_DETAILS: BiText = BiText::new("可折叠区域", "Collapsible Section");
pub const SLASH_CODE: BiText = BiText::new("代码块", "Code Block");
pub const SLASH_TABLE: BiText = BiText::new("表格 (3×3)", "Table (3×3)");
pub const SLASH_MATH: BiText = BiText::new("数学公式块", "Math Block");
pub const SLASH_DIVIDER: BiText = BiText::new("分割线", "Divider");
pub const SLASH_IMAGE: BiText = BiText::new("插入本地图片", "Insert Local Image");
pub const SLASH_EMOJI: BiText = BiText::new("表情符号", "Emoji");

pub const DETAILS_TEMPLATE: BiText = BiText::new(
    "<details open>\n<summary>标题</summary>\n\n正文\n\n</details>",
    "<details open>\n<summary>Title</summary>\n\nContent\n\n</details>",
);

// Placeholders
pub const LINK_INPUT_PLACEHOLDER: BiText = BiText::new("输入链接 URL (回车确认)", "Enter link URL (Enter to apply)");
pub const SEARCH_DOC_PLACEHOLDER: BiText = BiText::new("在文档中查找…", "Find in document…");
pub const REPLACE_WITH_PLACEHOLDER: BiText = BiText::new("替换为…", "Replace with…");
pub const ANNOTATION_PLACEHOLDER: BiText = BiText::new("输入批注内容 (回车保存)…", "Add annotation (Enter to save)…");

// Image insertion
pub const DIALOG_INSERT_IMAGE: BiText = BiText::new("插入本地图片", "Insert Local Image");
pub const IMAGE_FILTER_LABEL: BiText = BiText::new("图片", "Images");
pub fn image_markdown_insertion(filename: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("![图片](<{filename}>)"),
        Locale::En => format!("![Image](<{filename}>)"),
    }
}

// Toolbar & menus
pub const TRANSLATE_SHORT_ICON: BiText = BiText::new("译", "Tr");
pub const BLOCK_TEXT: BiText = BiText::new("正文", "Text");
pub const BLOCK_H1: BiText = BiText::new("一级标题 H1", "Heading 1");
pub const BLOCK_H2: BiText = BiText::new("二级标题 H2", "Heading 2");
pub const BLOCK_H3: BiText = BiText::new("三级标题 H3", "Heading 3");
pub const BLOCK_H4: BiText = BiText::new("四级标题 H4", "Heading 4");
pub const BLOCK_BULLET: BiText = BiText::new("无序列表", "Bulleted list");
pub const BLOCK_ORDERED: BiText = BiText::new("有序列表", "Numbered list");
pub const BLOCK_TODO: BiText = BiText::new("任务列表", "To-do list");
pub const BLOCK_QUOTE: BiText = BiText::new("引用块", "Quote");
pub const BLOCK_CODE: BiText = BiText::new("代码块", "Code block");
pub const HIGHLIGHTER_LABEL: BiText = BiText::new("荧光笔", "Highlight");
pub const TEXT_COLOR_LABEL: BiText = BiText::new("字体颜色", "Text color");
pub const COLOR_PRESETS_EDIT: BiText = BiText::new("编辑", "Edit");
pub const COLOR_PRESETS_DONE: BiText = BiText::new("完成", "Done");
pub const COLOR_PRESET_ADD: BiText = BiText::new("添加色号", "Add color");
pub const COLOR_PRESET_RESET: BiText = BiText::new("恢复默认", "Reset");
pub const COLOR_HEX_PLACEHOLDER: BiText = BiText::new("#rrggbb 如 #dc2626", "#rrggbb e.g. #dc2626");
pub const COLOR_HEX_INVALID: BiText = BiText::new("请输入 3 或 6 位十六进制色号", "Enter 3 or 6 hex digits");

pub const EMOJI_HEADER_EMPTY: BiText = BiText::new("表情符号 · 输入中文或英文搜索", "Emoji · Type to search");
pub fn emoji_header_query(query: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("表情: {query} (↑↓选择 · Enter确认)"),
        Locale::En => format!("Emoji: {query} (↑↓ select · Enter)"),
    }
}
pub const SLASH_HEADER_EMPTY: BiText = BiText::new("基础与排版块 (↑↓选择 · Enter确认)", "Blocks (↑↓ select · Enter)");
pub fn slash_header_query(query: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("过滤: /{query}"),
        Locale::En => format!("Filter: /{query}"),
    }
}

pub const TURN_INTO_TEXT: BiText = BiText::new("转为正文", "Turn into Text");
pub const TURN_INTO_H1: BiText = BiText::new("转为一级标题 H1", "Turn into Heading 1");
pub const TURN_INTO_H2: BiText = BiText::new("转为二级标题 H2", "Turn into Heading 2");
pub const TURN_INTO_H3: BiText = BiText::new("转为三级标题 H3", "Turn into Heading 3");
pub const TURN_INTO_BULLET: BiText = BiText::new("转为无序列表", "Turn into Bulleted list");
pub const TURN_INTO_TODO: BiText = BiText::new("转为任务列表", "Turn into To-do list");
pub const TURN_INTO_QUOTE: BiText = BiText::new("转为引用块", "Turn into Quote");
pub const TURN_INTO_CODE: BiText = BiText::new("转为代码块", "Turn into Code block");
pub const DELETE_BLOCK: BiText = BiText::new("删除该块", "Delete block");

pub const SAVE_ANNOTATION: BiText = BiText::new("保存批注", "Save annotation");
pub const LOADING_MORE_CONTENT: BiText = BiText::new("正在加载后续内容…", "Loading remaining content…");

pub fn link_tooltip_click(is_mac: bool, is_anchor: bool) -> String {
    match (locale(), is_mac, is_anchor) {
        (Locale::ZhCn, true, true) => "Cmd+左键 跳转".into(),
        (Locale::ZhCn, true, false) => "Cmd+左键 打开".into(),
        (Locale::ZhCn, false, true) => "Ctrl+左键 跳转".into(),
        (Locale::ZhCn, false, false) => "Ctrl+左键 打开".into(),
        (Locale::En, true, true) => "Cmd+Click to jump".into(),
        (Locale::En, true, false) => "Cmd+Click to open".into(),
        (Locale::En, false, true) => "Ctrl+Click to jump".into(),
        (Locale::En, false, false) => "Ctrl+Click to open".into(),
    }
}

pub const FIND_CASE_SENSITIVE: BiText = BiText::new("区分大小写", "Match Case");
pub const FIND_WHOLE_WORD: BiText = BiText::new("全字匹配", "Match Whole Word");
pub const FIND_TOGGLE_REPLACE: BiText = BiText::new("替换 (Ctrl+H)", "Replace (Ctrl+H)");
pub const FIND_PREV: BiText = BiText::new("上一个 (Shift+Enter)", "Previous (Shift+Enter)");
pub const FIND_NEXT: BiText = BiText::new("下一个 (Enter)", "Next (Enter)");
pub const FIND_CLOSE: BiText = BiText::new("关闭 (Esc)", "Close (Esc)");
pub const REPLACE_ONE: BiText = BiText::new("替换当前 (Enter)", "Replace (Enter)");
pub const REPLACE_ALL: BiText = BiText::new("全部替换", "Replace All");
pub const DOC_ANNOTATIONS: BiText = BiText::new("文档批注", "Document Annotations");

// Context menu
pub const CTX_BOLD: BiText = BiText::new("加粗", "Bold");
pub const CTX_ITALIC: BiText = BiText::new("斜体", "Italic");
pub const CTX_UNDERLINE: BiText = BiText::new("下划线", "Underline");
pub const CTX_STRIKETHROUGH: BiText = BiText::new("删除线", "Strikethrough");
pub const CTX_HIGHLIGHT: BiText = BiText::new("荧光高亮", "Highlight");
pub const CTX_HEADING: BiText = BiText::new("标题", "Heading");
pub const CTX_BULLET_LIST: BiText = BiText::new("无序列表", "Bulleted List");
pub const CTX_TASK_LIST: BiText = BiText::new("任务列表", "Task List");
pub const CTX_INSERT_IMAGE: BiText = BiText::new("插入图片…", "Insert Image…");
pub const CTX_INSERT_TABLE: BiText = BiText::new("插入表格", "Insert Table");

// Modals
pub const EDIT_FORMULA_TITLE: BiText = BiText::new("编辑公式", "Edit Formula");
pub const EDIT_FORMULA_HINT: BiText = BiText::new("输入 LaTeX 正文，无需添加分隔符。", "Enter LaTeX body without delimiters.");
pub const OVERWRITE_IMAGE_TITLE: BiText = BiText::new("替换同名图片？", "Replace Existing Image?");
pub const IMAGE_IMPORT_FAILED: BiText = BiText::new("图片导入失败", "Image Import Failed");

// Outline (src/markdown/outline.rs)
pub const OUTLINE_TITLE: BiText = BiText::new("文档大纲", "Outline");
pub const OUTLINE_EMPTY: BiText = BiText::new("暂无标题", "No headings");
pub const OUTLINE_COLLAPSE: BiText = BiText::new("收起文档大纲", "Collapse outline");
pub const OUTLINE_EXPAND: BiText = BiText::new("展开文档大纲", "Expand outline");
