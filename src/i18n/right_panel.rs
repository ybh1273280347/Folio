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
pub const ACTION_NEW_PAGE: BiText = BiText::new("新建页面", "New Page");
pub const ACTION_NEW_PAGE_DESC: BiText = BiText::new("在右侧分屏中新建 Markdown 页面", "Open a new Markdown page in split view");
pub const ACTION_ZEN_MODE: BiText = BiText::new("Zen 专注模式", "Zen Mode");
pub const ACTION_PREFERENCES: BiText = BiText::new("偏好设置", "Preferences");

pub const NO_MATCHING_FEATURES: BiText = BiText::new("没有匹配的功能", "No matching features");
pub const DEFAULT_BUILD_TARGET: BiText = BiText::new("项目", "Project");
pub const STATUS_RUNNING: BiText = BiText::new("运行中", "Running");
pub const STATUS_WAITING: BiText = BiText::new("等待操作", "Waiting");
pub const STATUS_GENERATING: BiText = BiText::new("生成中", "Generating");
pub const RUNNING_TASKS: BiText = BiText::new("运行中任务", "Running Tasks");
pub const HUB_SECTION_OPEN: BiText = BiText::new("打开", "Open");
pub const HUB_SECTION_CREATE: BiText = BiText::new("新建", "Create");
pub const BADGE_SYNCING: BiText = BiText::new("整理中", "Syncing");

pub const KB_OPEN_PROJECT_HINT: BiText = BiText::new(
    "打开项目目录后可使用知识库",
    "Open a project folder to use Knowledge Base",
);
pub const KB_ENABLE_TITLE: BiText = BiText::new("项目知识库", "Project Knowledge Base");
pub const KB_ENABLE_DESC: BiText = BiText::new("本地检索笔记与 PDF", "Search notes & PDFs locally");
pub const KB_ENABLE_BTN: BiText = BiText::new("开启知识库", "Enable");
pub const KB_SEARCH_PLACEHOLDER: BiText = BiText::new("搜索笔记或 PDF…", "Search notes or PDFs…");
pub const KB_SEARCHING: BiText = BiText::new("检索中…", "Searching…");
pub const KB_NO_RESULTS: BiText = BiText::new("未找到相关内容", "No matching results");
pub const KB_EMPTY_HINT: BiText = BiText::new("回车检索项目文档", "Press Enter to search");
pub const KB_SYNC_TOOLTIP: BiText = BiText::new("增量同步索引", "Incremental sync");
pub const KB_PDF_MANAGER_TOOLTIP: BiText = BiText::new("PDF 管理器", "PDF Manager");
pub const KB_PDF_MANAGER_TITLE: BiText = BiText::new("PDF 管理器", "PDF Manager");
pub const KB_BACK_TO_SEARCH: BiText = BiText::new("返回知识库", "Back to Knowledge Base");
pub const KB_MENU_TOOLTIP: BiText = BiText::new("更多操作", "More actions");
pub const KB_MENU_REBUILD: BiText = BiText::new("重建索引", "Rebuild index");
pub const KB_MENU_DISABLE: BiText = BiText::new("停用知识库", "Disable Knowledge Base");
pub const KB_FILTER_ALL: BiText = BiText::new("全部", "All");
pub const KB_FILTER_NOTES: BiText = BiText::new("笔记", "Notes");
pub const KB_FILTER_PDF: BiText = BiText::new("PDF", "PDF");
pub const KB_PDF_FILTER_NEEDS_OCR: BiText = BiText::new("待补全", "Needs OCR");
pub const KB_PDF_FILTER_COMPLETE: BiText = BiText::new("已完整", "Ready");
pub const KB_PDF_FILTER_ACCURATE: BiText = BiText::new("精确", "Accurate");
pub const KB_SELECT_ALL: BiText = BiText::new("全选", "All");
pub const KB_PDF_EMPTY: BiText = BiText::new("暂无可管理的 PDF", "No PDFs in index");
pub const KB_ONLY_OCR_BTN: BiText = BiText::new("补全缺页", "Fill missing");
pub const KB_EXPORT_MD_BTN: BiText = BiText::new("导出 .md", "Export .md");
pub const KB_EXPORT_UNAVAILABLE_TIP: BiText = BiText::new(
    "所选文档尚有缺页未补全，暂不可导出",
    "Selected PDFs still have missing pages",
);
pub const KB_CANCEL_BTN: BiText = BiText::new("取消", "Cancel");
pub const KB_RETRY_BTN: BiText = BiText::new("重试", "Retry");
pub const KB_COLLAPSE_HITS: BiText = BiText::new("收起", "Collapse");
pub const KB_DRAFT_BADGE: BiText = BiText::new("草稿", "Draft");
pub const KB_ANNOT_BADGE: BiText = BiText::new("批注", "Note");
pub const KB_ACCURATE_BADGE: BiText = BiText::new("精确", "Accurate");
pub const KB_HYBRID_BADGE: BiText = BiText::new("混合", "Hybrid");
pub const KB_FAST_BADGE: BiText = BiText::new("快速", "Fast");
pub const KB_NEEDS_OCR_BADGE: BiText = BiText::new("待补全", "Needs OCR");
pub const KB_FAILED_BADGE: BiText = BiText::new("失败", "Failed");
pub const KB_QUEUED_OCR: BiText = BiText::new("排队等待 OCR…", "Queued for OCR…");
pub const KB_OPEN_SETTINGS: BiText = BiText::new("打开设置", "Open settings");

pub fn kb_search_summary(hits: usize, docs: usize, truncated: bool) -> String {
    let suffix = if truncated { "+" } else { "" };
    match locale() {
        Locale::ZhCn => format!("{hits}{suffix} 条 · {docs} 篇"),
        Locale::En => format!("{hits}{suffix} hits · {docs} docs"),
    }
}

pub fn kb_more_hits(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("展开其余 {count} 条命中"),
        Locale::En => format!("Show {count} more"),
    }
}

pub fn kb_hits_pill(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{count} 处"),
        Locale::En => {
            if count == 1 {
                "1 hit".to_owned()
            } else {
                format!("{count} hits")
            }
        }
    }
}

pub fn kb_pages_count(pages: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{pages} 页"),
        Locale::En => format!("{pages}p"),
    }
}

pub fn kb_missing_pages_badge(missing: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("缺 {missing} 页"),
        Locale::En => format!("{missing}p OCR"),
    }
}

pub fn kb_ocr_running_label(label: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("OCR 识别中 · {label}"),
        Locale::En => format!("OCR running · {label}"),
    }
}

pub fn kb_export_selected_label(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("导出 ({count})"),
        Locale::En => format!("Export ({count})"),
    }
}

pub fn kb_fill_missing_label(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("补全缺页 ({count})"),
        Locale::En => format!("Fill ({count})"),
    }
}

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

pub fn kb_selected_count(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已选 {count}"),
        Locale::En => format!("{count} selected"),
    }
}

pub fn kb_refine_selected(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("精确解析 ({count})"),
        Locale::En => format!("Accurate ({count})"),
    }
}

pub fn kb_exported_md(rel_path: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已导出：{rel_path}"),
        Locale::En => format!("Exported: {rel_path}"),
    }
}

pub fn kb_exported_count(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已导出 {count} 篇 Markdown"),
        Locale::En => format!("Exported {count} Markdown file(s)"),
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
