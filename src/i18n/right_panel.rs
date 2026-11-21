use super::{BiText, Locale, locale};

pub const BADGE_OPENED: BiText = BiText::new("已开启", "Open");
pub const BADGE_EXPANDED: BiText = BiText::new("已展开", "Expanded");
pub const BADGE_BUILD_FAILED: BiText = BiText::new("编译失败", "Failed");

pub const ACTION_AI: BiText = BiText::new("AI 助手", "AI Assistant");
pub const ACTION_TERMINAL: BiText = BiText::new("终端", "Terminal");
pub const ACTION_PDF_PREVIEW: BiText = BiText::new("PDF 预览", "PDF Preview");
pub const ACTION_BUILD_LOGS: BiText = BiText::new("编译日志", "Build Logs");
pub const ACTION_OPEN_FILE: BiText = BiText::new("打开文件", "Open File");
pub const ACTION_PREFERENCES: BiText = BiText::new("偏好设置", "Preferences");

pub const NO_MATCHING_FEATURES: BiText = BiText::new("没有匹配的功能", "No matching features");
pub const DEFAULT_BUILD_TARGET: BiText = BiText::new("项目", "Project");
pub const STATUS_RUNNING: BiText = BiText::new("运行中", "Running");
pub const STATUS_WAITING: BiText = BiText::new("等待操作", "Waiting");
pub const STATUS_GENERATING: BiText = BiText::new("生成中", "Generating");
pub const RUNNING_TASKS: BiText = BiText::new("运行中任务", "Running Tasks");

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
