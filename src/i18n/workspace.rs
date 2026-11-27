use super::{BiText, Locale, locale};
use std::path::Path;

// Startup & storage errors
pub const ERR_OPEN_WINDOW: BiText = BiText::new("无法打开 Folio 原生窗口", "Failed to open Folio native window");
pub const ERR_DATA_DIR_NOT_FOUND: BiText = BiText::new("找不到应用数据目录。", "Application data directory not found.");
pub const ERR_LOAD_PROJECTS: BiText = BiText::new("无法读取项目列表", "Failed to load project list");
pub const ERR_LOAD_SESSIONS: BiText = BiText::new("无法读取工作区标签页", "Failed to load workspace sessions");
pub const ERR_LOAD_SETTINGS: BiText = BiText::new("无法读取界面设置", "Failed to load settings");

// File & folder operations (files.rs / trash.rs)
pub const ERR_OUTSIDE_PROJECT: BiText = BiText::new("无法访问项目文件夹之外的文件。", "Cannot access files outside the project folder.");
pub const ERR_SELECT_FOLDER: BiText = BiText::new("请选择一个文件夹。", "Please select a folder.");
pub const ERR_INVALID_PROJECT_FILENAME: BiText = BiText::new(
    "请输入有效的文件名，并使用受支持的可新建扩展名（不支持直接新建 PDF、图片或 HTML）。",
    "Please enter a valid file name with a supported creatable extension (PDF, images, and HTML cannot be created directly).",
);
pub const ERR_INVALID_FILENAME_EXT: BiText = BiText::new(
    "请输入有效的文件名和受支持的扩展名。",
    "Please enter a valid file name with a supported extension.",
);
pub const ERR_INVALID_FOLDER_NAME: BiText = BiText::new("请输入有效的文件夹名称。", "Please enter a valid folder name.");
pub const ERR_NAME_ALREADY_EXISTS: BiText = BiText::new("同名文件夹或文件已存在。", "A file or folder with the same name already exists.");
pub const ERR_SELECT_PROJECT_FOLDER: BiText = BiText::new("请选择项目内的文件夹。", "Please select a folder inside the project.");
pub const ERR_MOVE_FOLDER_INTO_SELF: BiText = BiText::new("不能将文件夹移入自身或子文件夹。", "Cannot move a folder into itself or a subfolder.");
pub const ERR_TARGET_EXISTS_UNMOVED: BiText = BiText::new("目标已存在同名文件或文件夹，未移动。", "Target already exists; item was not moved.");
pub const ERR_BINARY_FILE_UNSUPPORTED: BiText = BiText::new(
    "检测到二进制内容，无法作为纯文本打开。",
    "Binary content detected; cannot open as plain text.",
);
pub const ERR_NON_UTF8_TEXT: BiText = BiText::new(
    "文件编码不是有效的 UTF-8，无法打开编辑。",
    "The file is not valid UTF-8 and cannot be opened for editing.",
);

pub fn err_file_too_large(max_mb: u64) -> String {
    match locale() {
        Locale::ZhCn => format!("文件超过 {max_mb} MB 文本编辑上限。"),
        Locale::En => format!("The file exceeds the {max_mb} MB text editor limit."),
    }
}

pub fn untitled_ext(ext: &str, number: Option<usize>) -> String {
    match (locale(), number) {
        (Locale::ZhCn, None) => format!("未命名.{ext}"),
        (Locale::ZhCn, Some(n)) => format!("未命名{n}.{ext}"),
        (Locale::En, None) => format!("Untitled.{ext}"),
        (Locale::En, Some(n)) => format!("Untitled{n}.{ext}"),
    }
}

pub fn untitled_md(number: Option<usize>) -> String {
    untitled_ext("md", number)
}

pub const ERR_TRASH_INVALID_ID: BiText = BiText::new("无效的废纸篓条目 ID。", "Invalid trash entry ID.");
pub const ERR_TRASH_MISMATCH_DIR: BiText = BiText::new("废纸篓条目 ID 与存储目录不匹配。", "Trash entry ID does not match storage directory.");
pub const ERR_TRASH_FILES_ONLY: BiText = BiText::new("只能将文件移入 Folio 废纸篓。", "Only files can be moved to Folio Trash.");

// Workspace UI & dialogs
pub const DEFAULT_SPACE: BiText = BiText::new("默认空间", "Default Workspace");
pub const LOOSE_FILES: BiText = BiText::new("独立文件", "Loose Files");
pub const ADD_LOCAL_PROJECT: BiText = BiText::new("添加本地项目", "Add Local Project");
pub const FILES_HEADER: BiText = BiText::new("文件", "Files");
pub const OPEN_FILE: BiText = BiText::new("打开文件", "Open File");
pub const REFRESH_FILES_TOOLTIP: BiText = BiText::new("刷新文件列表", "Refresh file list");
pub const FILES_REFRESHED: BiText = BiText::new("已刷新文件列表。", "File list refreshed.");
pub const SEARCH_HUB_PLACEHOLDER: BiText = BiText::new("搜索功能、命令或文件…", "Search features, commands, or files…");
pub const BATCH_EDIT_TAGS_TOOLTIP: BiText = BiText::new("批量编辑标签（Alt+T）", "Batch edit tags (Alt+T)");
pub const TRASH_LABEL: BiText = BiText::new("废纸篓", "Trash");
pub const SETTINGS_LABEL: BiText = BiText::new("设置", "Settings");
pub const TOOLTIP_PROJECT_SWITCHER: BiText = BiText::new("切换或管理项目", "Switch or manage projects");
pub const SEARCH_PROJECTS_PLACEHOLDER: BiText = BiText::new("搜索项目…", "Search projects…");
pub const NO_MATCHING_PROJECTS: BiText = BiText::new("没有匹配的项目", "No matching projects");
pub const NO_PROJECTS_YET: BiText = BiText::new("暂无本地项目", "No local projects yet");
pub const TOOLTIP_TOGGLE_SIDEBAR_OPEN: BiText = BiText::new("展开文件树", "Show file tree");
pub const TOOLTIP_TOGGLE_SIDEBAR_CLOSE: BiText = BiText::new("收起文件树", "Hide file tree");
pub const MENU_NEW_DEFAULT_MD: BiText = BiText::new("新建 Markdown", "New Markdown");
pub const EXPORT_BTN: BiText = BiText::new("导出", "Export");
pub const NO_MATCHING_FILES: BiText = BiText::new("没有匹配的文件", "No matching files");

pub fn selected_count(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已选 {count} 项"),
        Locale::En => format!("{count} selected"),
    }
}

pub const TRASH_SUBTITLE: BiText = BiText::new(
    "删除的文件保留 7 天，到期自动清除",
    "Deleted files are kept for 7 days before automatic removal",
);
pub const TRASH_SCOPE_CURRENT: BiText = BiText::new("当前项目", "Current Project");
pub const TRASH_SCOPE_ALL: BiText = BiText::new("全部项目", "All Projects");
pub const TRASH_SEARCH_PLACEHOLDER: BiText = BiText::new("筛选文件名或路径…", "Filter by file name or path…");
pub const TRASH_RESTORE: BiText = BiText::new("恢复", "Restore");
pub const TRASH_RESTORE_ALL: BiText = BiText::new("全部恢复", "Restore All");
pub const TRASH_RESTORE_SELECTED: BiText = BiText::new("恢复选中", "Restore Selected");
pub const TRASH_PURGE: BiText = BiText::new("彻底删除", "Delete Permanently");
pub const TRASH_CLEAR_PROJECT: BiText = BiText::new("清空当前项目", "Empty Project");
pub const TRASH_CLEAR_ALL: BiText = BiText::new("清空全部", "Empty All");
pub const TRASH_CLEAR_GROUP: BiText = BiText::new("清空", "Empty");
pub const TRASH_EMPTY: BiText = BiText::new("废纸篓为空", "Trash is empty");
pub const TRASH_EMPTY_PROJECT: BiText = BiText::new("当前项目废纸篓为空", "Current project Trash is empty");
pub const TRASH_EMPTY_SUBTITLE: BiText = BiText::new(
    "删除的文件会在此保留 7 天，可随时恢复",
    "Deleted files stay here for 7 days and can be restored anytime",
);
pub const TRASH_NO_MATCHES: BiText = BiText::new("没有匹配的已删除文件", "No matching deleted files");
pub const TRASH_CLEAR_FILTER: BiText = BiText::new("清除筛选", "Clear Filter");
pub const TRASH_VIEW_ALL_PROJECTS: BiText = BiText::new("查看全部项目", "View All Projects");
pub const TRASH_PROJECT_REMOVED: BiText = BiText::new("已移除", "Removed");
pub const TRASH_DIR_MISSING: BiText = BiText::new("目录缺失", "Folder missing");
pub const TRASH_MAIN_FILE_BADGE: BiText = BiText::new("主文件", "Main");

pub fn trash_other_projects_hint(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("其他项目中还有 {count} 个已删除文件"),
        Locale::En => {
            if count == 1 {
                "1 deleted item in other projects".to_owned()
            } else {
                format!("{count} deleted items in other projects")
            }
        }
    }
}

pub fn trash_item_count(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{count} 项"),
        Locale::En => {
            if count == 1 {
                "1 item".to_owned()
            } else {
                format!("{count} items")
            }
        }
    }
}

pub fn trash_time_tooltip(deleted: &str, expires: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("删除于 {deleted} · 将于 {expires} 自动清除"),
        Locale::En => format!("Deleted {deleted} · Auto-purges {expires}"),
    }
}

// File dialogs & pickers
pub const DIALOG_EXPORT_PDF: BiText = BiText::new("导出 PDF", "Export PDF");
pub const DIALOG_SELECT_PROJECT_FOLDER: BiText = BiText::new("选择本地项目文件夹", "Select Local Project Folder");
pub const DIALOG_OPEN_DOCUMENT: BiText = BiText::new("打开文档", "Open Document");
pub const DIALOG_SUPPORTED_FILES: BiText = BiText::new("Folio 支持的文件", "Folio Supported Files");

// Messages & reports
pub const ERR_SINGLE_PROJECT_OR_DOCS: BiText = BiText::new(
    "请一次打开一个项目文件夹，或选择多个文档。",
    "Please open one project folder at a time, or select multiple documents.",
);
pub const ERR_SELECT_SUPPORTED_LOCAL_FILE: BiText = BiText::new("请选择受支持的本地文件。", "Please select a supported local file.");
pub const ERR_EMPTY_TITLE: BiText = BiText::new("标题不能为空。", "Title cannot be empty.");
pub const ERR_FILE_DELETED_RESTORE_FIRST: BiText = BiText::new(
    "文件已被删除，请在原位置恢复文件后保存草稿。",
    "File was deleted; restore it at its original path before saving.",
);
pub const ERR_SELECT_COMPILE_FILE: BiText = BiText::new(
    "请选择要编译的 .tex 或 .md 文件。",
    "Please select a .tex or .md file to compile.",
);
pub const ERR_SAME_PDF_OUTPUT: BiText = BiText::new(
    "所选文件会生成同名 PDF，请分开编译。",
    "Selected files produce the same PDF name; please compile them separately.",
);
pub const ERR_COMPILE_TARGET_MISSING: BiText = BiText::new("待编译文件不存在", "Target file to compile does not exist");
pub const ERR_SELECT_MAIN_TEX: BiText = BiText::new(
    "请选择一个 .tex 文件作为编译主文件。",
    "Please select a .tex file as the main compile file.",
);
pub const ERR_SELECT_SOURCE_PROJECT: BiText = BiText::new("请选择来源项目。", "Please select a source project.");
pub const ERR_MOVE_SAME_PROJECT_FILE_ONLY: BiText = BiText::new(
    "仅支持移动同项目文件，独立文件只能复制路径。",
    "Only files within the same project can be moved; loose files only support copying paths.",
);
pub const ERR_WAIT_BUILD_MOVE_FILE: BiText = BiText::new("请等待编译结束后移动文件。", "Please wait for compilation to finish before moving files.");
pub const ERR_WAIT_BUILD_SWITCH_WORKSPACE: BiText = BiText::new("请等待编译结束后切换工作区。", "Please wait for compilation to finish before switching workspaces.");
pub const ERR_SOURCE_NOT_IN_PROJECT: BiText = BiText::new("来源文件不属于当前项目。", "Source file does not belong to the current project.");
pub const ERR_MOVE_SAME_PROJECT_FOLDER_ONLY: BiText = BiText::new("仅支持移动同一项目内的文件夹。", "Only folders within the same project can be moved.");
pub const ERR_WAIT_BUILD_MOVE_FOLDER: BiText = BiText::new("请等待编译结束后移动文件夹。", "Please wait for compilation to finish before moving folders.");
pub const ERR_PROJECT_NAME_EMPTY: BiText = BiText::new("项目名称不能为空。", "Project name cannot be empty.");
pub const ERR_LOOSE_FILE_NO_DELETE: BiText = BiText::new("独立文件不支持删除。", "Loose files cannot be deleted.");
pub const ERR_SOURCE_PROJECT_CHANGED: BiText = BiText::new("来源项目已切换，请重新拖动。", "Source project changed; please drag again.");

pub fn err_read_background_image(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法读取背景图：{error}"),
        Locale::En => format!("Failed to read background image: {error}"),
    }
}

pub fn err_file_not_found(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("找不到文件：{}", path.display()),
        Locale::En => format!("File not found: {}", path.display()),
    }
}

pub fn err_unsupported_open(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("Folio 不支持打开此文件：{}", path.display()),
        Locale::En => format!("Folio cannot open this file type: {}", path.display()),
    }
}

pub fn err_unsupported_file_type(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("此文件类型不能在 Folio 中打开：{}", path.display()),
        Locale::En => format!("This file type cannot be opened in Folio: {}", path.display()),
    }
}

pub fn err_link_target_not_found(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("找不到链接目标：{}", path.display()),
        Locale::En => format!("Link target not found: {}", path.display()),
    }
}

pub fn err_open_dir(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("无法打开目录：{}", path.display()),
        Locale::En => format!("Failed to open directory: {}", path.display()),
    }
}

pub fn err_open_file(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("无法打开文件：{}", path.display()),
        Locale::En => format!("Failed to open file: {}", path.display()),
    }
}

pub fn err_target_exists_unmoved(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("目标存在同名文件，未移动：{}", path.display()),
        Locale::En => format!("Target file already exists; not moved: {}", path.display()),
    }
}

// Confirmation dialogs
pub const DIALOG_UNSAVED_ALL_TITLE: BiText = BiText::new("处理未保存的修改", "Unsaved Changes");
pub const DIALOG_UNSAVED_ALL_BODY: BiText = BiText::new(
    "保存全部修改、放弃修改，或返回继续编辑。",
    "Save all changes, discard them, or return to editing.",
);
pub const DIALOG_UNSAVED_FILE_BODY: BiText = BiText::new(
    "保存文件、放弃修改，或返回继续编辑。",
    "Save changes, discard them, or return to editing.",
);
pub fn dialog_close_unsaved_title(path: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("关闭 {path} 前保存修改？"),
        Locale::En => format!("Save changes before closing {path}?"),
    }
}

pub fn dialog_trash_title(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("将 {count} 个文件移入废纸篓？"),
        Locale::En => {
            if count == 1 {
                "Move 1 file to Trash?".to_owned()
            } else {
                format!("Move {count} files to Trash?")
            }
        }
    }
}
pub const DIALOG_TRASH_BODY: BiText = BiText::new(
    "文件保留 7 天，可在废纸篓恢复。",
    "Files are kept for 7 days and can be restored from Trash.",
);
pub const DIALOG_TRASH_FOLDER_TITLE: BiText = BiText::new("删除此文件夹？", "Delete this folder?");
pub const DIALOG_TRASH_FOLDER_BODY: BiText = BiText::new(
    "受支持的项目文件会移入废纸篓并保留 7 天。文件夹仅在磁盘上完全为空时移除。",
    "Supported project files move to Trash for 7 days. The folder is removed only if it is completely empty on disk.",
);
pub const DIALOG_TRASH_CONFIRM: BiText = BiText::new("移入废纸篓", "Move to Trash");
pub const REPORT_MOVED_TO_TRASH: BiText = BiText::new("已移入废纸篓，按 Ctrl+Z 撤销。", "Moved to Trash. Press Ctrl+Z to undo.");
pub const REPORT_REMOVED_EMPTY_FOLDER: BiText = BiText::new("已移除空文件夹。", "Empty folder removed.");
pub const REPORT_COPIED_PATH: BiText = BiText::new("已复制完整路径。", "Copied full path.");
pub const REPORT_COPIED_FOLDER_PATH: BiText = BiText::new("已复制文件夹完整路径。", "Copied folder full path.");
pub fn report_copied_paths(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已复制 {count} 个文件的完整路径。"),
        Locale::En => format!("Copied full paths of {count} files."),
    }
}

pub const REPORT_TRASH_PARENT_MISSING: BiText = BiText::new(
    "原项目目录不存在，文件仍保留在废纸篓。",
    "Original project directory no longer exists; file remains in Trash.",
);
pub const DIALOG_TRASH_CONFLICT_TITLE: BiText = BiText::new("原位置已有同名文件", "File Already Exists at Destination");

pub fn dialog_purge_title(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("彻底删除 {count} 个文件？"),
        Locale::En => {
            if count == 1 {
                "Permanently delete 1 file?".to_owned()
            } else {
                format!("Permanently delete {count} files?")
            }
        }
    }
}
pub const DIALOG_PURGE_BODY: BiText = BiText::new("删除后无法从 Folio 废纸篓恢复。", "Deleted files cannot be recovered from Folio Trash.");
pub const DIALOG_PURGE_CONFIRM: BiText = BiText::new("彻底删除", "Delete Permanently");

pub const DIALOG_REMOVE_PROJECT_TITLE: BiText = BiText::new("从工作台移除项目？", "Remove Project from Workspace?");
pub const DIALOG_REMOVE_PROJECT_BODY: BiText = BiText::new("文件夹和文件保留在磁盘上。", "The folder and its files will remain on disk.");
pub const DIALOG_REMOVE_PROJECT_CONFIRM: BiText = BiText::new("移除项目", "Remove Project");

pub const BTN_KEEP_EDITING: BiText = BiText::new("继续编辑", "Keep Editing");
pub const BTN_DISCARD_CHANGES: BiText = BiText::new("放弃修改", "Discard Changes");

// Context menus
pub const MENU_REVEAL_IN_EXPLORER: BiText = BiText::new("在资源管理器中显示", "Reveal in Explorer");
pub const MENU_RENAME_PROJECT: BiText = BiText::new("重命名项目…", "Rename Project…");
pub const MENU_REMOVE_PROJECT: BiText = BiText::new("移除项目…", "Remove Project…");
pub const MENU_COPY_FULL_PATH: BiText = BiText::new("复制完整路径", "Copy Full Path");
pub const MENU_EDIT_TAGS: BiText = BiText::new("编辑标签…", "Edit Tags…");
pub const MENU_RENAME: BiText = BiText::new("重命名…", "Rename…");
pub const MENU_NEW_MARKDOWN: BiText = BiText::new("新建 .md", "New .md");
pub const MENU_NEW_CANVAS: BiText = BiText::new("新建 .canvas", "New .canvas");
pub const MENU_NEW_TEX: BiText = BiText::new("新建 .tex", "New .tex");
pub const MENU_NEW_DRAWIO: BiText = BiText::new("新建 .drawio", "New .drawio");
pub const MENU_NEW_BIB: BiText = BiText::new("新建 .bib", "New .bib");
pub const MENU_NEW_TXT: BiText = BiText::new("新建 .txt", "New .txt");
pub const MENU_NEW_OTHER: BiText = BiText::new("其他…", "Other…");
pub const MENU_NEW_FILE: BiText = BiText::new("其他文件…", "Other File…");
pub const MENU_NEW_FOLDER: BiText = BiText::new("新建文件夹…", "New Folder…");
pub const TOOLTIP_NEW_FILE_MENU: BiText = BiText::new("新建其他类型文件", "New other file type");
pub const TOOLTIP_BATCH_COMPILE: BiText = BiText::new("编译所选文件", "Compile selected");
pub const TOOLTIP_CLEAR_SELECTION: BiText = BiText::new("取消选择", "Clear selection");
pub const CREATE_FILE_HINT: BiText = BiText::new(
    "输入带受支持扩展名的文件名（如 styles.sty、config.yaml、data.csv）；不支持直接新建 PDF、图片或 HTML。",
    "Enter a filename with a supported extension (e.g. styles.sty, config.yaml, data.csv). PDF, images, and HTML cannot be created directly.",
);
pub const MENU_BATCH_FOLDER_TAGS: BiText = BiText::new("批量编辑文件夹标签…", "Batch Edit Folder Tags…");
pub const MENU_DELETE_FOLDER: BiText = BiText::new("删除文件夹…", "Delete Folder…");
pub const MENU_BATCH_TAGS: BiText = BiText::new("批量编辑标签…", "Batch Edit Tags…");
pub const MENU_EDIT_FILE_TAGS: BiText = BiText::new("编辑此文件的标签…", "Edit File Tags…");
pub const MENU_OPEN_IN_SPLIT: BiText = BiText::new("在分屏中打开", "Open in Split Pane");
pub const MENU_SET_MAIN_FILE: BiText = BiText::new("设为编译主文件", "Set as Main File");
pub const MENU_DELETE: BiText = BiText::new("删除…", "Delete…");
pub const MENU_ASK_AI: BiText = BiText::new("问 AI", "Ask AI");
pub const MENU_CUT: BiText = BiText::new("剪切", "Cut");
pub const MENU_PASTE: BiText = BiText::new("粘贴", "Paste");
pub const MENU_SELECT_ALL: BiText = BiText::new("全选", "Select All");

// Sidebar & editor area
pub const TOOLTIP_COLLAPSE_OUTLINE: BiText = BiText::new("收起文档大纲", "Collapse outline");
pub const TOOLTIP_EXPAND_OUTLINE: BiText = BiText::new("展开文档大纲", "Expand outline");
pub const TOOLTIP_CLOSE_TAB: BiText = BiText::new("关闭标签", "Close tab");
pub const TOOLTIP_SWITCH_WYSIWYG: BiText = BiText::new("切换到可视化视图", "Switch to WYSIWYG view");
pub const TOOLTIP_SWITCH_SOURCE: BiText = BiText::new("切换到源码视图", "Switch to source view");
pub const SOURCE_LABEL: BiText = BiText::new("源码", "Source");
pub const BANNER_FILE_DELETED: BiText = BiText::new("文件已从磁盘删除，草稿已保留。", "File was deleted from disk; draft preserved.");
pub const BANNER_DISK_MODIFIED: BiText = BiText::new("磁盘文件已被外部修改。", "File on disk was modified externally.");
pub const BTN_LOAD_DISK: BiText = BiText::new("使用磁盘版本", "Load Disk Version");
pub const BTN_OVERWRITE_DISK: BiText = BiText::new("保留草稿并覆盖", "Keep Draft & Overwrite");

pub const EMPTY_LOOSE_HINT: BiText = BiText::new("打开文件，或点击上方新建按钮开始写作", "Open a file, or click the New button above to start writing");
pub const EMPTY_PROJECT_HINT: BiText = BiText::new("选择文件，或点击上方新建按钮开始写作", "Select a file, or click the New button above to start writing");
pub const SHORTCUTS_BAR_LOOSE: BiText = BiText::new(
    "Ctrl+Shift+O 打开文件  ·  Ctrl+O 打开项目  ·  Ctrl+S 保存",
    "Ctrl+Shift+O Open File  ·  Ctrl+O Open Project  ·  Ctrl+S Save",
);
pub const SHORTCUTS_BAR_PROJECT: BiText = BiText::new(
    "Ctrl+O 打开项目  ·  Ctrl+S 保存",
    "Ctrl+O Open Project  ·  Ctrl+S Save",
);

// Preview & compile toolbar
pub const COMPILE_PREVIEW_TITLE: BiText = BiText::new("编译预览", "Build Preview");
pub const PREVIEW_TITLE: BiText = BiText::new("预览", "Preview");
pub const STATUS_COMPILING: BiText = BiText::new("正在编译…", "Compiling…");
pub const STATUS_COMPILE_IDLE: BiText = BiText::new("编译后在此查看文档", "Compile to preview document here");
pub const SELECT_MAIN_FILE: BiText = BiText::new("选择主文件", "Select main file");
pub const BTN_STOP_BUILD: BiText = BiText::new("■  停止", "■  Stop");
pub const BTN_COMPILE_BUILD: BiText = BiText::new("▶  编译", "▶  Compile");
pub const STATUS_STOPPING: BiText = BiText::new("正在停止", "Stopping");
pub const STATUS_BUILDING: BiText = BiText::new("正在编译", "Compiling");
pub const STATUS_NEEDS_REBUILD: BiText = BiText::new(" · 需重新编译", " · Rebuild needed");
pub fn status_failed_secs(secs: f32) -> String {
    match locale() {
        Locale::ZhCn => format!("失败 · {secs:.1}s"),
        Locale::En => format!("Failed · {secs:.1}s"),
    }
}
pub fn status_cancelled_secs(secs: f32) -> String {
    match locale() {
        Locale::ZhCn => format!("已取消 · {secs:.1}s"),
        Locale::En => format!("Cancelled · {secs:.1}s"),
    }
}
pub const MAIN_FILE_FALLBACK: BiText = BiText::new("主文件", "main file");
pub fn tooltip_compile_target(target: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("编译 {target} · Ctrl+Enter"),
        Locale::En => format!("Compile {target} · Ctrl+Enter"),
    }
}
pub const TOOLTIP_BUILD_SETTINGS: BiText = BiText::new("编译设置", "Build settings");
pub const LABEL_COMPILE_ENGINE: BiText = BiText::new("编译引擎", "Compile Engine");
pub const LABEL_MAIN_FILE: BiText = BiText::new("编译主文件", "Main File");
pub const TOOLTIP_COLLAPSE_RIGHT_HUB: BiText = BiText::new("收起右侧功能栏", "Collapse right panel");
pub const TOOLTIP_EXPAND_RIGHT_HUB: BiText = BiText::new("展开右侧功能栏", "Expand right panel");

pub fn exit_zen_label(shortcut: &str) -> String {
    match (locale(), shortcut.is_empty()) {
        (Locale::ZhCn, true) => "退出 Zen".to_owned(),
        (Locale::ZhCn, false) => format!("退出 Zen · {shortcut}"),
        (Locale::En, true) => "Exit Zen".to_owned(),
        (Locale::En, false) => format!("Exit Zen · {shortcut}"),
    }
}

// Name modal titles
pub const MODAL_CREATE_FILE: BiText = BiText::new("新建文件", "New File");
pub const MODAL_CREATE_FOLDER: BiText = BiText::new("新建文件夹", "New Folder");
pub const MODAL_RENAME_FILE: BiText = BiText::new("重命名文件", "Rename File");
pub const MODAL_RENAME_FOLDER: BiText = BiText::new("重命名文件夹", "Rename Folder");
pub const MODAL_RENAME_PROJECT: BiText = BiText::new("重命名项目", "Rename Project");

// Link validation errors
pub const ERR_EMPTY_LINK: BiText = BiText::new("空链接。", "Empty link.");
pub fn err_resolve_local_link(raw: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("无法解析本地链接：{raw}"),
        Locale::En => format!("Cannot resolve local link: {raw}"),
    }
}
pub fn err_invalid_link(raw: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("无效链接：{raw}"),
        Locale::En => format!("Invalid link: {raw}"),
    }
}

pub fn untitled_drawio(number: Option<usize>) -> String {
    match (locale(), number) {
        (Locale::ZhCn, None) => "未命名绘图.drawio".to_owned(),
        (Locale::ZhCn, Some(n)) => format!("未命名绘图 {n}.drawio"),
        (Locale::En, None) => "Untitled.drawio".to_owned(),
        (Locale::En, Some(n)) => format!("Untitled {n}.drawio"),
    }
}

pub fn untitled_canvas(number: Option<usize>) -> String {
    match (locale(), number) {
        (Locale::ZhCn, None) => "未命名画布.canvas".to_owned(),
        (Locale::ZhCn, Some(n)) => format!("未命名画布 {n}.canvas"),
        (Locale::En, None) => "Untitled.canvas".to_owned(),
        (Locale::En, Some(n)) => format!("Untitled {n}.canvas"),
    }
}

pub const MENU_PASTE_IMAGE: BiText = BiText::new("粘贴图像", "Paste Image");
pub const ERR_IMPORT_FILES_ONLY: BiText = BiText::new(
    "请选择要复制到项目的文件。",
    "Select files to copy into the project.",
);
pub fn report_imported_files(count: usize) -> String {
    match (locale(), count) {
        (Locale::ZhCn, _) => format!("已复制 {count} 个文件到项目"),
        (Locale::En, 1) => "Copied 1 file into the project".to_owned(),
        (Locale::En, _) => format!("Copied {count} files into the project"),
    }
}
pub const REPORT_NO_CLIPBOARD_IMAGE: BiText = BiText::new(
    "剪贴板中没有可粘贴的图像或 SVG。",
    "No pasteable image or SVG found in clipboard.",
);
pub fn report_pasted_image(name: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已粘贴图像：{name}"),
        Locale::En => format!("Pasted image: {name}"),
    }
}
pub fn report_pasted_images(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已粘贴 {count} 个图像文件"),
        Locale::En => format!("Pasted {count} image files"),
    }
}

pub fn report_relocated_markdown_links(files: usize, links: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已自动更新 {files} 个文档中的 {links} 处引用"),
        Locale::En => {
            let link_word = if links == 1 { "link" } else { "links" };
            let file_word = if files == 1 { "file" } else { "files" };
            format!("Updated {links} {link_word} across {files} {file_word}")
        }
    }
}

pub fn report_relocated_markdown_links_with_conflicts(
    files: usize,
    links: usize,
    conflicts: usize,
) -> String {
    match locale() {
        Locale::ZhCn => {
            format!("已自动更新 {files} 个文档中的 {links} 处引用（{conflicts} 个文档因外部修改已跳过）")
        }
        Locale::En => {
            let link_word = if links == 1 { "link" } else { "links" };
            let file_word = if files == 1 { "file" } else { "files" };
            format!(
                "Updated {links} {link_word} across {files} {file_word} ({conflicts} skipped due to external changes)"
            )
        }
    }
}

pub fn report_relocated_markdown_links_conflict(path: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("文档已被外部修改，未更新引用：{path}"),
        Locale::En => format!("File was modified externally; links not updated: {path}"),
    }
}

pub const TOOLTIP_BACKLINKS_ALL: BiText = BiText::new(
    "查看引用此文件的文档与画布",
    "Documents and canvases referencing this file",
);
pub const MARKDOWN_BACKLINKS_MENU_HEADER: BiText = BiText::new("文档反向引用", "Document Backlinks");
