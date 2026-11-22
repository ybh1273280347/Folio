use super::{BiText, Locale, locale};

pub const SETTINGS_TITLE: BiText = BiText::new("设置", "Settings");
pub const NAV_APPEARANCE: BiText = BiText::new("外观", "Appearance");
pub const NAV_FONTS: BiText = BiText::new("字体", "Fonts");
pub const NAV_THEME: BiText = BiText::new("主题", "Theme");
pub const NAV_VIEW: BiText = BiText::new("视图", "View");
pub const NAV_SHORTCUTS: BiText = BiText::new("快捷键", "Shortcuts");
pub const NAV_TERMINAL: BiText = BiText::new("终端", "Terminal");
pub const NAV_API: BiText = BiText::new("API 服务", "API Services");

// Theme section (theme names themselves remain untouched)
pub const PAGE_THEME_TITLE: BiText = BiText::new("外观 / 主题", "Appearance / Theme");
pub const THEME_SECTION_TITLE: BiText = BiText::new("主题", "Theme");
pub const THEME_NAME_PLACEHOLDER: BiText = BiText::new("主题名称", "Theme name");
pub const EDIT_COLORS: BiText = BiText::new("编辑配色", "Edit Colors");
pub const COLLAPSE_COLORS: BiText = BiText::new("收起配色", "Hide Colors");
pub const SAVE_THEME: BiText = BiText::new("保存主题", "Save Theme");
pub const DELETE_THEME: BiText = BiText::new("删除主题", "Delete Theme");
pub const RESET_THEME: BiText = BiText::new("恢复默认主题", "Reset Default Theme");
pub const ERR_THEME_NAME_EMPTY: BiText = BiText::new("请为主题填写名称。", "Please enter a theme name.");
pub const ERR_THEME_NAME_RESERVED: BiText = BiText::new("请填写主题名称。", "Please enter a valid theme name.");

pub fn current_theme(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("当前：{name}"),
        Locale::En => format!("Current: {name}"),
    }
}

pub const COLOR_BACKGROUND: BiText = BiText::new("背景", "Background");
pub const COLOR_SURFACE: BiText = BiText::new("画布与弹窗", "Surface & Popover");
pub const COLOR_TEXT: BiText = BiText::new("正文与界面文字", "Body & UI Text");
pub const COLOR_MUTED: BiText = BiText::new("次要文字", "Muted Text");
pub const COLOR_BORDER: BiText = BiText::new("边框与分隔线", "Border & Divider");
pub const COLOR_HOVER: BiText = BiText::new("悬停背景", "Hover Background");
pub const COLOR_PRESSED: BiText = BiText::new("按下背景", "Pressed Background");
pub const COLOR_ACCENT: BiText = BiText::new("强调色", "Accent");
pub const COLOR_ON_ACCENT: BiText = BiText::new("强调色上的文字", "Text on Accent");
pub const COLOR_CODE: BiText = BiText::new("代码背景", "Code Background");
pub const COLOR_SELECTION: BiText = BiText::new("选中背景", "Selection Background");
pub const COLOR_DANGER: BiText = BiText::new("错误与删除", "Danger & Error");

// Fonts section
pub const PAGE_FONTS_TITLE: BiText = BiText::new("外观 / 字体", "Appearance / Fonts");
pub const SYSTEM_DEFAULT: BiText = BiText::new("系统默认", "System Default");
pub const DEFAULT_MONOSPACE: BiText = BiText::new("默认等宽字体", "Default Monospace");
pub const SECTION_SURFACE_FONTS: BiText = BiText::new("各区域字体", "Surface Fonts");
pub const FONT_SIZE: BiText = BiText::new("字号", "Size");
pub const FONT_INTERFACE: BiText = BiText::new("界面 · 工具栏、侧栏与设置", "Interface · Chrome, Sidebar & Settings");
pub const FONT_MARKDOWN: BiText = BiText::new("Markdown 预览", "Markdown Preview");
pub const FONT_AI: BiText = BiText::new("AI 面板", "AI Panel");
pub const FONT_TERMINAL: BiText = BiText::new("终端", "Terminal");
pub const FONT_EDITOR: BiText = BiText::new("源码编辑器", "Source Editor");
pub const EDITOR_FONT_SAMPLE: BiText = BiText::new(
    "\\section{Introduction} % 中文注释",
    "\\section{Introduction} % Sample comment",
);
pub const SECTION_SHARED_TYPOGRAPHY: BiText = BiText::new("Markdown 与 AI 共享排版", "Shared Markdown & AI Typography");
pub const SHARED_TYPOGRAPHY_HINT: BiText = BiText::new(
    "代码和公式按各自的正文字号缩放。",
    "Code and math scale relative to each surface's body font size.",
);
pub const INLINE_CODE_FONT: BiText = BiText::new("行内代码字体", "Inline Code Font");
pub const BLOCK_CODE_FONT: BiText = BiText::new("代码块字体", "Code Block Font");
pub const FONT_SCALE: BiText = BiText::new("字号比例", "Scale");
pub const MARKDOWN_FONT_SAMPLE: BiText = BiText::new(
    "调用 `merge(39, 61)` 合并两个结点。\n\n```rust\nlet root = merge(39, 61);\n```",
    "Call `merge(39, 61)` to join two nodes.\n\n```rust\nlet root = merge(39, 61);\n```",
);
pub const MATH_FONT_LABEL: BiText = BiText::new("数学字体", "Math Font");
pub const INLINE_MATH_SCALE: BiText = BiText::new("行内公式比例", "Inline Math Scale");
pub const BLOCK_MATH_SCALE: BiText = BiText::new("块级公式比例", "Display Math Scale");
pub const SECTION_FONT_FALLBACK: BiText = BiText::new("字体回退", "Font Fallback");
pub const FONT_FALLBACK_HINT: BiText = BiText::new(
    "界面、终端、Markdown 预览和 AI 面板的主字体缺少字形时，优先使用此字体。",
    "Preferred fallback font when primary UI, terminal, Markdown, or AI fonts lack glyphs.",
);
pub const CJK_FONT_SCALE: BiText = BiText::new("中西文字体比例", "CJK Font Scale");
pub const CJK_FONT_SCALE_HINT: BiText = BiText::new(
    "按比例缩放中文及全角标点字号，用于平衡中文字体相对英文偏小的问题。",
    "Scale CJK characters and fullwidth punctuation relative to Latin text.",
);
pub const FALLBACK_FONT_SAMPLE: BiText = BiText::new("Aa 中文字体预览", "Aa CJK & Latin Font Preview");
pub const RESET_FONTS: BiText = BiText::new("恢复默认字体", "Reset Default Fonts");

// View section
pub const PAGE_VIEW_TITLE: BiText = BiText::new("外观 / 视图", "Appearance / View");
pub const LANGUAGE_LABEL: BiText = BiText::new("界面语言", "Interface Language");
pub const BACKGROUND_IMAGE: BiText = BiText::new("背景图", "Background Image");
pub const CHOOSE_IMAGE: BiText = BiText::new("选择图片", "Choose Image");
pub const CHOOSE_BACKGROUND_TITLE: BiText = BiText::new("选择背景图", "Choose Background Image");
pub const IMAGE_FILTER: BiText = BiText::new("图片", "Images");
pub const REMOVE_IMAGE: BiText = BiText::new("移除图片", "Remove Image");
pub const IMAGE_OPACITY: BiText = BiText::new("图片不透明度", "Image Opacity");
pub const TOOL_EXECUTION_VIEW: BiText = BiText::new("工具执行视图", "Tool Execution View");
pub const TOOL_VIEW_MINIMAL: BiText = BiText::new("极简模式", "Minimal");
pub const TOOL_VIEW_STANDARD: BiText = BiText::new("标准模式", "Standard");
pub const TOOL_VIEW_DETAILED: BiText = BiText::new("详细模式", "Detailed");
pub const SHOW_LINE_NUMBERS: BiText = BiText::new("显示行号（TeX / Markdown 源码）", "Show Line Numbers (TeX / Markdown Source)");
pub const LIMIT_LINE_WIDTH: BiText = BiText::new("编辑器限制行宽", "Limit Editor Line Width");
pub const INTERNAL_LINK_TARGET: BiText = BiText::new("内部文件链接跳转位置", "Internal File Link Target");
pub const TARGET_MAIN_EDITOR: BiText = BiText::new("主编辑区", "Main Editor");
pub const TARGET_SPLIT_PANE: BiText = BiText::new("右侧分屏", "Right Split Pane");
pub const INTERNAL_LINK_HINT: BiText = BiText::new(
    "Ctrl+左键点击 Markdown 内部文件链接时的打开方式（非分屏类型仍在主编辑区打开）",
    "Where Ctrl+Click opens internal Markdown file links (unsupported split types still open in the main editor)",
);

// Terminal section
pub const PAGE_TERMINAL_TITLE: BiText = BiText::new("终端设置", "Terminal Settings");
pub const TERMINAL_SHELL: BiText = BiText::new("Shell 命令", "Shell Command");
pub const TERMINAL_SHELL_PLACEHOLDER: BiText = BiText::new("自动使用 PowerShell", "Auto-detect PowerShell");
pub const TERMINAL_SHELL_HINT: BiText = BiText::new(
    "留空时自动使用 PowerShell。默认参数不含 -NoProfile，因此会加载 shell 的用户 profile。",
    "Leave empty to auto-select PowerShell. Default args omit -NoProfile so your user shell profile is loaded.",
);
pub const TERMINAL_ARGS: BiText = BiText::new("启动参数", "Launch Arguments");
pub const TERMINAL_ARGS_HINT: BiText = BiText::new(
    "每行作为一个独立参数传给 shell；不会按空格、引号或 shell 语法拆分。",
    "One argument per line passed directly to the shell without space, quote, or shell splitting.",
);
pub const TERMINAL_PANEL_WIDTH: BiText = BiText::new("终端面板宽度", "Terminal Panel Width");

// API section
pub const PAGE_API_TITLE: BiText = BiText::new("API 服务", "API Services");
pub const AI_SERVICE_TITLE: BiText = BiText::new("AI 服务（Magpie 网关）", "AI Service (Magpie Gateway)");
pub const AI_SERVICE_DESC: BiText = BiText::new(
    "原生 AI 界面经由 pi SDK 宿主调用本地 Magpie 统一网关（免密钥）",
    "Native AI UI connects through the pi SDK host to the local Magpie gateway (no key required)",
);
pub const CONFIGURED: BiText = BiText::new("已配置", "Configured");
pub const ENTER_ADDRESS: BiText = BiText::new("请填写地址", "Enter URL");
pub const DEFAULT_BTN: BiText = BiText::new("默认", "Default");
pub const TITLE_MODEL_ID: BiText = BiText::new("标题模型 ID", "Title Model ID");
pub const QIANFAN_API_KEY: BiText = BiText::new("千帆 API Key", "Qianfan API Key");
pub const QIANFAN_PLACEHOLDER: BiText = BiText::new("百度千帆 API Key", "Baidu Qianfan API Key");
pub const TRANSLATE_SERVICE_TITLE: BiText = BiText::new("翻译服务", "Translation Services");
pub const TRANSLATE_SERVICE_DESC: BiText = BiText::new(
    "支持中英互译 · 优先 Google，失败时自动切换百度",
    "Chinese/English translation · Prefers Google, falls back to Baidu",
);
pub const GOOGLE_TRANSLATE_TITLE: BiText = BiText::new("Google 翻译", "Google Translate");
pub const GOOGLE_TRANSLATE_DESC: BiText = BiText::new(
    "内置网页翻译接口，无需填写 API Key（依赖当前网络或系统代理）",
    "Built-in web endpoint, no API Key needed (uses current network or system proxy)",
);
pub const ZERO_CONFIG: BiText = BiText::new("免配置", "Ready");
pub const BAIDU_TRANSLATE_TITLE: BiText = BiText::new("百度翻译", "Baidu Translate");
pub const BAIDU_TRANSLATE_DESC: BiText = BiText::new(
    "通用文本翻译 API（fanyi-api.baidu.com），需配置 APP ID 与密钥",
    "General text translation API (fanyi-api.baidu.com), requires APP ID and API Key",
);
pub const KEY_REQUIRED: BiText = BiText::new("需配置密钥", "Key Required");
pub const BAIDU_APP_ID_PLACEHOLDER: BiText = BiText::new("填写百度翻译 APP ID", "Enter Baidu Translate APP ID");
pub const BAIDU_API_KEY_PLACEHOLDER: BiText = BiText::new("填写百度翻译密钥（API Key）", "Enter Baidu Translate API Key");
pub const API_KEY_SECRET_LABEL: BiText = BiText::new("API Key（密钥）", "API Key");
pub const RAG_SERVICE_TITLE: BiText = BiText::new("本地知识库（RAG）与 PaddleOCR", "Local Knowledge Base (RAG) & PaddleOCR");
pub const RAG_SERVICE_DESC: BiText = BiText::new(
    "本地 llama-server 向量/重排 + PaddleOCR-VL 扫描 PDF 精修",
    "Local llama-server embedding/rerank + PaddleOCR-VL scanned PDF parsing",
);
pub const RAG_OCR_PLACEHOLDER: BiText = BiText::new(
    "可选：留空时自动复用 PaddleOCR MCP Token",
    "Optional: auto-reuses PaddleOCR MCP Token when blank",
);
pub const RAG_AUTO_MANAGE: BiText = BiText::new("按需拉起 / 空闲释放 llama-server", "Auto-start & idle-reap llama-server");

// Shortcuts section
pub const PAGE_SHORTCUTS_TITLE: BiText = BiText::new("快捷键设置", "Keyboard Shortcuts");
pub const RESET_ALL_SHORTCUTS: BiText = BiText::new("全部恢复默认", "Reset All");
pub const SHORTCUTS_HINT: BiText = BiText::new(
    "点击右侧按键框后直接按下新的组合键即可修改；在 Markdown 所见即所得模式下还可随时输入 / 呼出快捷块菜单。",
    "Click a shortcut box and press a new key combination to rebind; in Markdown WYSIWYG mode, type / anytime for the block menu.",
);
pub const PRESS_SHORTCUT_PROMPT: BiText = BiText::new("请按下快捷键… (Esc取消)", "Press keys… (Esc to cancel)");

pub fn shortcut_conflict(other: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("与「{other}」冲突"),
        Locale::En => format!("Conflicts with \"{other}\""),
    }
}

pub const CAT_WORKSPACE: BiText = BiText::new("工作区与全局", "Workspace & Global");
pub const CAT_MARKDOWN: BiText = BiText::new("Markdown 编辑", "Markdown Editing");

pub const SC_SAVE: BiText = BiText::new("保存当前文件", "Save current file");
pub const SC_SAVE_ALL: BiText = BiText::new("保存全部文件", "Save all files");
pub const SC_OPEN_PROJECT: BiText = BiText::new("打开本地项目", "Open local project");
pub const SC_OPEN_FILE: BiText = BiText::new("打开本地文件", "Open local file");
pub const SC_COMPILE: BiText = BiText::new("编译文档", "Compile document");
pub const SC_TOGGLE_SIDEBAR: BiText = BiText::new("显示 / 隐藏左侧栏", "Toggle left sidebar");
pub const SC_FIND_FILE: BiText = BiText::new("查找项目文件", "Find project file");
pub const SC_TOGGLE_AI: BiText = BiText::new("打开 / 收起 AI 助手", "Toggle AI Assistant");
pub const SC_TOGGLE_TERMINAL: BiText = BiText::new("打开 / 收起终端", "Toggle Terminal");
pub const SC_TOGGLE_ZEN: BiText = BiText::new("进入 / 退出 Zen 模式", "Toggle Zen Mode");
pub const SC_CLOSE_TAB: BiText = BiText::new("关闭当前标签页", "Close current tab");
pub const SC_TOGGLE_SOURCE: BiText = BiText::new("切换源码 / 所见即所得", "Toggle Source / WYSIWYG");
pub const SC_MD_BOLD: BiText = BiText::new("加粗", "Bold");
pub const SC_MD_ITALIC: BiText = BiText::new("斜体", "Italic");
pub const SC_MD_UNDERLINE: BiText = BiText::new("下划线", "Underline");
pub const SC_MD_STRIKETHROUGH: BiText = BiText::new("删除线", "Strikethrough");
pub const SC_MD_CODE: BiText = BiText::new("行内代码", "Inline code");
pub const SC_MD_HIGHLIGHT: BiText = BiText::new("荧光笔高亮", "Highlight");
pub const SC_MD_LINK: BiText = BiText::new("插入 / 编辑链接", "Insert / edit link");
pub const SC_MD_FIND: BiText = BiText::new("文档内查找", "Find in document");
