use super::{BiText, Locale, locale};

pub const BADGE_OPENED: BiText = BiText::new("已开启", "Open");
pub const BADGE_EXPANDED: BiText = BiText::new("已展开", "Expanded");
pub const BADGE_BUILD_FAILED: BiText = BiText::new("编译失败", "Failed");

pub const ACTION_AI: BiText = BiText::new("AI 助手", "AI Assistant");
pub const ACTION_KNOWLEDGE_BASE: BiText = BiText::new("知识库", "Knowledge Base");
pub const ACTION_TERMINAL: BiText = BiText::new("终端", "Terminal");
pub const ACTION_PDF_PREVIEW: BiText = BiText::new("PDF 预览", "PDF Preview");
pub const ACTION_BUILD_LOGS: BiText = BiText::new("编译日志", "Build Logs");
pub const ACTION_OPEN_FILE: BiText = BiText::new("打开文件", "Open File");
pub const ACTION_ZEN_MODE: BiText = BiText::new("Zen 专注模式", "Zen Mode");
pub const ACTION_PREFERENCES: BiText = BiText::new("偏好设置", "Preferences");

pub const NO_MATCHING_FEATURES: BiText = BiText::new("没有匹配的功能", "No matching features");
pub const DEFAULT_BUILD_TARGET: BiText = BiText::new("项目", "Project");
pub const STATUS_RUNNING: BiText = BiText::new("运行中", "Running");
pub const STATUS_WAITING: BiText = BiText::new("等待操作", "Waiting");
pub const STATUS_GENERATING: BiText = BiText::new("生成中", "Generating");
pub const RUNNING_TASKS: BiText = BiText::new("运行中任务", "Running Tasks");
pub const BADGE_SYNCING: BiText = BiText::new("整理中", "Syncing");

pub const KB_OPEN_PROJECT_HINT: BiText = BiText::new(
    "打开项目目录后可使用知识库",
    "Open a project folder to use Knowledge Base",
);
pub const KB_ENABLE_TITLE: BiText = BiText::new("项目知识库", "Project Knowledge Base");
pub const KB_ENABLE_DESC: BiText = BiText::new("本地检索笔记与 PDF", "Search notes & PDFs locally");
pub const KB_ENABLE_BTN: BiText = BiText::new("开启知识库", "Enable");
pub const KB_SEARCH_PLACEHOLDER: BiText = BiText::new("搜索笔记、公式或 PDF…", "Search notes, formulas, PDFs…");
pub const KB_SEARCHING: BiText = BiText::new("检索中…", "Searching…");
pub const KB_NO_RESULTS: BiText = BiText::new("未找到相关内容", "No matching results");
pub const KB_EMPTY_HINT: BiText = BiText::new("回车检索项目文档", "Press Enter to search");
pub const KB_SYNC_TOOLTIP: BiText = BiText::new("立即同步索引", "Sync index");
pub const KB_MENU_TOOLTIP: BiText = BiText::new("更多操作", "More actions");
pub const KB_MENU_REBUILD: BiText = BiText::new("重新整理索引", "Rebuild index");
pub const KB_MENU_ACCURATE_OCR: BiText = BiText::new("深度解析：仅扫描件", "Accurate: Scan only");
pub const KB_MENU_ACCURATE_ALL: BiText = BiText::new("深度解析：全部 PDF", "Accurate: All PDFs");
pub const KB_MENU_DISABLE: BiText = BiText::new("停用知识库", "Disable Knowledge Base");
pub const KB_KEYWORD_ONLY: BiText = BiText::new("仅关键词", "Keyword only");
pub const KB_KEYWORD_ONLY_TIP: BiText = BiText::new(
    "语义服务未连接，点击前往设置检查",
    "Semantic service unavailable, click to check settings",
);
pub const KB_DRAFT_BADGE: BiText = BiText::new("草稿", "Draft");
pub const KB_ANNOT_BADGE: BiText = BiText::new("批注", "Note");
pub const KB_ACCURATE_BADGE: BiText = BiText::new("精修", "Accurate");
pub const KB_OPEN_SETTINGS: BiText = BiText::new("打开设置", "Open settings");

pub fn kb_doc_count(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{count} 篇"),
        Locale::En => {
            if count == 1 {
                "1 doc".to_owned()
            } else {
                format!("{count} docs")
            }
        }
    }
}

pub fn kb_ocr_badge(count: usize) -> String {
    format!("{count} OCR")
}

pub fn kb_ocr_heading(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{count} 篇扫描件待识别"),
        Locale::En => {
            if count == 1 {
                "1 PDF needs OCR".to_owned()
            } else {
                format!("{count} PDFs need OCR")
            }
        }
    }
}

pub fn kb_refine_selected(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("深度解析 ({count})"),
        Locale::En => format!("Accurate ({count})"),
    }
}

pub fn compiling_target(target: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("正在编译 {target}"),
        Locale::En => format!("Compiling {target}"),
    }
}

pub fn terminal_active_sessions(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("终端 · {count} 个活跃会话"),
        Locale::En => {
            if count == 1 {
                "Terminal · 1 active session".to_owned()
            } else {
                format!("Terminal · {count} active sessions")
            }
        }
    }
}
