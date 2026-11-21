use super::{BiText, Locale, locale};

// Main AI panel (src/ai/mod.rs)
pub const INPUT_PLACEHOLDER: BiText = BiText::new("随心输入，/ 呼出命令…", "Ask anything, or type / for commands…");
pub const EXT_INPUT_PLACEHOLDER: BiText = BiText::new("输入内容…", "Enter text…");
pub const EXT_TEXTAREA_PLACEHOLDER: BiText = BiText::new("编辑文本…", "Edit text…");
pub const MODEL_SEARCH_PLACEHOLDER: BiText = BiText::new("搜索模型…", "Search models...");
pub const ADD_TO_CHAT: BiText = BiText::new("添加到对话", "Add to chat");
pub const DEFAULT_NOTE_TITLE: BiText = BiText::new("AI回答", "AI Response");
pub const AI_ANSWER_SOURCE: BiText = BiText::new("AI 回答", "AI Response");
pub const YOUR_MESSAGE: BiText = BiText::new("你的消息", "You");
pub const CONTEXT_COMPACTED: BiText = BiText::new("上下文已压缩", "Context compacted");
pub const TOOL_INCOMPLETE: BiText = BiText::new("工具调用未完成", "Tool call incomplete");
pub const REQUEST_FAILED: BiText = BiText::new("请求失败", "Request failed");
pub const EXT_RUNTIME_ERROR: BiText = BiText::new("扩展运行错误", "Extension runtime error");
pub const PROMPT_CONFIRM: BiText = BiText::new("请求确认", "Confirmation required");
pub const PROMPT_SELECT: BiText = BiText::new("请选择", "Please select");
pub const PROMPT_INPUT: BiText = BiText::new("请输入", "Please enter");
pub const PROMPT_EDIT: BiText = BiText::new("编辑内容", "Edit content");
pub const DETACH_CURRENT_CTX_TITLE: BiText = BiText::new("取消当前上下文", "Detach current context");
pub const ATTACH_CURRENT_CTX_TITLE: BiText = BiText::new("附加当前上下文", "Attach current context");
pub const COMPACT_TITLE: BiText = BiText::new("压缩", "Compact");
pub const COMPACT_SUBTITLE_DEFAULT: BiText = BiText::new("压缩此聊天的上下文", "Compact this chat's context");
pub const DIALOG_PICK_ATTACHMENT: BiText = BiText::new("选择附加文件", "Select Attachment");
pub const PREVIEW_FIRST_120_LINES: BiText = BiText::new("仅显示前 120 行预览", "Showing first 120 lines preview");
pub const ERR_RESOLVE_FILE_REF: BiText = BiText::new("无法解析文件引用", "Unable to resolve file reference");
pub const COPY_CODE: BiText = BiText::new("复制代码", "Copy code");
pub const COPY_TABLE: BiText = BiText::new("复制表格", "Copy table");
pub const COPY_ANSWER: BiText = BiText::new("复制回答", "Copy response");
pub const COPY_MESSAGE: BiText = BiText::new("复制消息", "Copy message");
pub const EXPORT_AS_NOTE: BiText = BiText::new("导出为笔记", "Export as note");
pub const THINKING_ELLIPSIS: BiText = BiText::new("思考中…", "Thinking…");
pub const STILL_THINKING: BiText = BiText::new("正在思考…", "Thinking…");
pub const THINKING_PROCESS: BiText = BiText::new("思考过程", "Thinking process");
pub const WORK_PROCESS: BiText = BiText::new("工作过程", "Work process");
pub const THINKING_AND_TOOLS: BiText = BiText::new("思考与工具调用", "Thinking & tool calls");
pub const INPUT_ARGS_LABEL: BiText = BiText::new("输入参数", "Input");
pub const OUTPUT_LABEL: BiText = BiText::new("输出", "Output");
pub const CONTEXT_WINDOW_EMPTY: BiText = BiText::new("上下文窗口：尚无统计", "Context window: no data yet");
pub const NO_MODELS_AVAILABLE: BiText = BiText::new("暂无可用模型", "No models available");
pub const NO_MATCHING_MODELS: BiText = BiText::new("无匹配模型", "No matching models");
pub const MODEL_NO_THINKING_LEVEL: BiText = BiText::new("当前模型不支持调节", "Not adjustable for this model");
pub const HERO_PREFIX: BiText = BiText::new("你想在 ", "What would you like to do in ");
pub const HERO_SUFFIX: BiText = BiText::new(" 里做什么？", "?");
pub const HERO_EMPTY: BiText = BiText::new("今天想做点什么？", "What would you like to work on today?");
pub const MODEL_FALLBACK: BiText = BiText::new("模型", "Model");
pub const TOOLTIP_ATTACH_FILE: BiText = BiText::new("选择文件附加", "Attach file");
pub const NO_OPEN_FILE: BiText = BiText::new("当前无打开文件", "No open file");
pub const TOOLTIP_PERM_MODE: BiText = BiText::new("切换命令与脚本的确认权限", "Toggle approval mode for commands & scripts");
pub const PERM_FULL_ACCESS: BiText = BiText::new("完全访问", "Full Access");
pub const PERM_ASK: BiText = BiText::new("需确认", "Ask");
pub const IMAGE_MESSAGE: BiText = BiText::new("图片消息", "Image message");
pub const SCROLL_TO_BOTTOM: BiText = BiText::new("回到底部", "Scroll to bottom");
pub const ERR_PASTE_SVG: BiText = BiText::new("不支持直接粘贴 SVG 矢量图片", "Pasting SVG images directly is not supported");
pub const ERR_PASTE_IMAGE_FORMAT: BiText = BiText::new("不支持直接粘贴该图片格式", "Pasting this image format directly is not supported");
pub const ERR_UNSUPPORTED_IMAGE_FORMAT: BiText = BiText::new("不支持的图片格式", "Unsupported image format");

pub fn export_note_ok(filename: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已导出为笔记：{filename}"),
        Locale::En => format!("Exported as note: {filename}"),
    }
}
pub fn export_note_err(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("导出笔记失败：{error}"),
        Locale::En => format!("Failed to export note: {error}"),
    }
}
pub fn command_failed(command: &str, err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("{command} 失败：{err}"),
        Locale::En => format!("{command} failed: {err}"),
    }
}
pub fn host_exited(detail: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("AI 宿主已退出{detail}"),
        Locale::En => format!("AI host exited{detail}"),
    }
}
pub fn title_gen_failed(err: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("标题生成失败，可手动重命名：{err}"),
        Locale::En => format!("Title generation failed (you can rename manually): {err}"),
    }
}
pub fn compact_failed(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("压缩失败：{err}"),
        Locale::En => format!("Compaction failed: {err}"),
    }
}
pub fn ext_error(err: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("扩展异常：{err}"),
        Locale::En => format!("Extension error: {err}"),
    }
}
pub fn edited_diff(path: &str, added: usize, removed: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已编辑 {path} +{added} -{removed}"),
        Locale::En => format!("Edited {path} +{added} -{removed}"),
    }
}
pub fn compact_subtitle_used(pct: u64) -> String {
    match locale() {
        Locale::ZhCn => format!("压缩此聊天的上下文（已使用 {pct}%）"),
        Locale::En => format!("Compact this chat's context ({pct}% used)"),
    }
}
pub fn send_failed(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("发送失败：{err}"),
        Locale::En => format!("Send failed: {err}"),
    }
}
pub fn worked_for(duration: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("工作了 {duration}"),
        Locale::En => format!("Worked for {duration}"),
    }
}
pub fn elapsed_for(duration: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("用时 {duration}"),
        Locale::En => format!("Took {duration}"),
    }
}
pub fn completed_steps(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已完成 {count} 个步骤"),
        Locale::En => {
            if count == 1 {
                "Completed 1 step".to_owned()
            } else {
                format!("Completed {count} steps")
            }
        }
    }
}
pub fn thought_for(duration: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("思考了 {duration}"),
        Locale::En => format!("Thought for {duration}"),
    }
}
pub fn tool_status_label(name: &str, finished: bool, failed: bool) -> String {
    match (locale(), name, finished, failed) {
        (Locale::ZhCn, "edit" | "write", _, true) => "编辑文件失败".into(),
        (Locale::ZhCn, "edit" | "write", true, false) => "编辑了文件".into(),
        (Locale::ZhCn, "edit" | "write", false, false) => "正在编辑文件".into(),
        (Locale::ZhCn, "run_python", _, true) => "Python 脚本执行失败".into(),
        (Locale::ZhCn, "run_python", true, false) => "运行了 Python 脚本".into(),
        (Locale::ZhCn, "run_python", false, false) => "正在运行 Python 脚本".into(),
        (Locale::ZhCn, "bash" | "powershell", _, true) => "命令执行失败".into(),
        (Locale::ZhCn, "bash" | "powershell", true, false) => "执行了命令".into(),
        (Locale::ZhCn, "bash" | "powershell", false, false) => "正在执行命令".into(),
        (Locale::ZhCn, _, _, true) => format!("{name} 执行失败"),
        (Locale::ZhCn, _, true, false) => format!("已执行 {name}"),
        (Locale::ZhCn, _, false, false) => format!("正在执行 {name}"),

        (Locale::En, "edit" | "write", _, true) => "Failed to edit file".into(),
        (Locale::En, "edit" | "write", true, false) => "Edited file".into(),
        (Locale::En, "edit" | "write", false, false) => "Editing file".into(),
        (Locale::En, "run_python", _, true) => "Python script failed".into(),
        (Locale::En, "run_python", true, false) => "Ran Python script".into(),
        (Locale::En, "run_python", false, false) => "Running Python script".into(),
        (Locale::En, "bash" | "powershell", _, true) => "Command failed".into(),
        (Locale::En, "bash" | "powershell", true, false) => "Ran command".into(),
        (Locale::En, "bash" | "powershell", false, false) => "Running command".into(),
        (Locale::En, _, _, true) => format!("{name} failed"),
        (Locale::En, _, true, false) => format!("Ran {name}"),
        (Locale::En, _, false, false) => format!("Running {name}"),
    }
}
pub fn context_window_stats(pct: u64, rem: u64, used_k: usize, total_k: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("上下文窗口：{pct}% 已用（剩余 {rem}%）· {used_k}k / {total_k}k"),
        Locale::En => format!("Context window: {pct}% used ({rem}% left) · {used_k}k / {total_k}k"),
    }
}
pub fn quote_prefix(source: &str, text: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("引用 {source}: {text}"),
        Locale::En => format!("Quote {source}: {text}"),
    }
}
pub fn attached_file_tooltip(display: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已附加 @{display}（点击取消）"),
        Locale::En => format!("Attached @{display} (click to remove)"),
    }
}
pub fn attach_current_file_tooltip(display: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("附加当前文件 @{display}"),
        Locale::En => format!("Attach current file @{display}"),
    }
}
pub fn detach_ctx_subtitle(display: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("移除 @{display}"),
        Locale::En => format!("Remove @{display}"),
    }
}
pub fn attach_ctx_subtitle(display: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("附加 @{display}"),
        Locale::En => format!("Attach @{display}"),
    }
}
pub fn format_duration_zh_en(total_secs: u64) -> String {
    match locale() {
        Locale::ZhCn => {
            if total_secs < 60 {
                format!("{total_secs} 秒")
            } else {
                format!("{} 分 {} 秒", total_secs / 60, total_secs % 60)
            }
        }
        Locale::En => {
            if total_secs < 60 {
                format!("{total_secs}s")
            } else {
                format!("{}m {}s", total_secs / 60, total_secs % 60)
            }
        }
    }
}
pub const VERB_EDITING_FILE: BiText = BiText::new("正在编辑文件", "Editing file");
pub const VERB_RUNNING_CMD: BiText = BiText::new("正在运行命令", "Running command");
pub const VERB_READING_FILE: BiText = BiText::new("正在查阅文件", "Reading file");
pub const VERB_CALLING_TOOL: BiText = BiText::new("正在调用工具", "Calling tool");
pub const VERB_EDITED_FILE: BiText = BiText::new("编辑了文件", "Edited file");
pub const VERB_RAN_CMD: BiText = BiText::new("运行了命令", "Ran command");
pub const VERB_READ_FILE: BiText = BiText::new("查阅了文件", "Read file");
pub const VERB_CALLED_TOOL: BiText = BiText::new("调用了工具", "Called tool");
pub const VERB_CREATED: BiText = BiText::new("已创建", "Created");
pub const VERB_EDITED: BiText = BiText::new("已编辑", "Edited");

pub fn err_decode_image(e: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法解析图片：{e}"),
        Locale::En => format!("Failed to decode image: {e}"),
    }
}
pub fn err_encode_png(e: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("PNG 转码失败：{e}"),
        Locale::En => format!("Failed to encode PNG: {e}"),
    }
}
pub fn err_read_image(e: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("读取图片失败：{e}"),
        Locale::En => format!("Failed to read image: {e}"),
    }
}
pub fn image_label(id: u64) -> String {
    match locale() {
        Locale::ZhCn => format!("图片 #{id}"),
        Locale::En => format!("Image #{id}"),
    }
}
pub const TOOL_TAGS_LIST: BiText = BiText::new("查看 Folio 标签", "List Folio tags");
pub const TOOL_FILES_QUERY: BiText = BiText::new("查找 Folio 文件及标签", "Query Folio files & tags");
pub const TOOL_RUN_PYTHON: BiText = BiText::new("运行 Python 脚本", "Run Python script");
pub fn tool_edit_folder_tags(folder: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("编辑目录「{folder}」下的文件标签"),
        Locale::En => format!("Edit file tags under \"{folder}\""),
    }
}
pub fn tool_edit_files_tags(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("编辑 {count} 个文件的标签"),
        Locale::En => format!("Edit tags for {count} files"),
    }
}
pub fn tool_manage_tag(tag: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("管理标签：{tag}"),
        Locale::En => format!("Manage tag: {tag}"),
    }
}

// Session manager (src/ai/manager.rs)
pub const NEW_SESSION: BiText = BiText::new("新会话", "New Session");
pub const SEARCH_SESSIONS_PLACEHOLDER: BiText = BiText::new("搜索会话标题…", "Search session titles…");
pub const SESSION_NAME_PLACEHOLDER: BiText = BiText::new("会话名称", "Session name");
pub fn err_save_tags(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("保存标签失败：{error}"),
        Locale::En => format!("Failed to save tags: {error}"),
    }
}
pub fn err_sessions_json(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("会话列表格式错误：{error}"),
        Locale::En => format!("Invalid session list format: {error}"),
    }
}
pub const HOST_EXITED_REOPEN_BANNER: BiText = BiText::new(
    "AI 宿主已退出；重新打开 AI 面板以恢复历史，任务不会自动重发。",
    "AI host exited; reopen the AI panel to restore history. Running tasks will not auto-resend.",
);
pub const SESSION_STATUS_WAITING: BiText = BiText::new("等待操作", "Waiting");
pub const SESSION_STATUS_RUNNING: BiText = BiText::new("运行中", "Running");
pub const SESSION_STATUS_FAILED: BiText = BiText::new("失败", "Failed");
pub const PIN: BiText = BiText::new("置顶", "Pin");
pub const UNPIN: BiText = BiText::new("取消置顶", "Unpin");
pub const RENAME: BiText = BiText::new("重命名", "Rename");
pub const DELETE_SESSION: BiText = BiText::new("删除会话", "Delete session");
pub fn session_meta_line(pinned: bool, count: usize, status: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{}{} 条 {}", if pinned { "置顶 · " } else { "" }, count, status),
        Locale::En => {
            let pin_prefix = if pinned { "Pinned · " } else { "" };
            let msg_word = if count == 1 { "msg" } else { "msgs" };
            if status.is_empty() {
                format!("{pin_prefix}{count} {msg_word}")
            } else {
                format!("{pin_prefix}{count} {msg_word} {status}")
            }
        }
    }
}
pub const SESSION_HISTORY: BiText = BiText::new("会话历史", "Session history");
pub const CLOSE_TAB_KEEP_RUNNING: BiText = BiText::new("关闭标签，任务继续运行", "Close tab (keep task running)");
pub const MAXIMIZE_RESTORE: BiText = BiText::new("最大化 / 还原", "Maximize / Restore");
pub const DELETE_ACTIVE_SESSION_MSG: BiText = BiText::new("停止任务并删除该会话？", "Stop the running task and delete this session?");
pub const DELETE_SESSION_MSG: BiText = BiText::new("确认删除该会话？此操作不可恢复。", "Delete this session? This cannot be undone.");
pub const STOP_AND_DELETE: BiText = BiText::new("停止并删除", "Stop & Delete");

// Confirmation cards (src/ai/confirmation.rs)
pub const APPROVE: BiText = BiText::new("允许", "Allow");
pub const RUN_COMMAND: BiText = BiText::new("运行命令", "Run command");
pub const DECLINE: BiText = BiText::new("拒绝", "Decline");
pub const ALLOW_FOR_SESSION: BiText = BiText::new("此会话始终允许", "Always allow in this session");
pub const UNDO: BiText = BiText::new("撤销", "Undo");
pub const UNDONE: BiText = BiText::new("已撤销", "Undone");
pub const STATE_DECLINED: BiText = BiText::new("已拒绝", "Declined");
pub const STATE_EXPIRED: BiText = BiText::new("已过期", "Expired");
pub const STATE_CANCELLED: BiText = BiText::new("已取消", "Cancelled");
pub const STATE_FAILED: BiText = BiText::new("未应用", "Failed");
pub const NEW_BADGE_DOT: BiText = BiText::new("· 新建", "· New");
pub const NEW_BADGE: BiText = BiText::new("新建", "New");
pub const TAG_RENAME_TITLE: BiText = BiText::new("重命名标签", "Rename tag");
pub const TAG_COLOR_TITLE: BiText = BiText::new("更改标签颜色", "Change tag color");

pub fn batch_undone_files(total_files: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已撤销 {total_files} 个文件的标签"),
        Locale::En => format!("Undid tags for {total_files} files"),
    }
}
pub fn batch_undone_changes(blocks: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已撤销 {blocks} 项标签变更"),
        Locale::En => format!("Undid {blocks} tag changes"),
    }
}
pub fn batch_updated_files(total_files: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已更新 {total_files} 个文件的标签"),
        Locale::En => format!("Updated tags for {total_files} files"),
    }
}
pub fn batch_completed_changes(blocks: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("已完成 {blocks} 项标签变更"),
        Locale::En => format!("Completed {blocks} tag changes"),
    }
}
pub fn review_color_changes(count: usize, applied: bool, undone: bool) -> String {
    match locale() {
        Locale::ZhCn => {
            let verb = if undone { "已撤销更改" } else if applied { "已更改" } else { "更改" };
            format!("{verb} {count} 个标签的颜色")
        }
        Locale::En => {
            let verb = if undone { "Undid color changes for" } else if applied { "Changed colors for" } else { "Change colors for" };
            format!("{verb} {count} tags")
        }
    }
}
pub fn review_add_tags(tags: &[String], count: usize, applied: bool, undone: bool) -> String {
    match locale() {
        Locale::ZhCn => {
            if undone {
                format!("已撤销「{}」的 {count} 个文件标签", tags.join("、"))
            } else if applied {
                format!("已为 {count} 个文件添加")
            } else {
                format!("为 {count} 个文件添加标签")
            }
        }
        Locale::En => {
            if undone {
                format!("Undid adding \"{}\" on {count} files", tags.join(", "))
            } else if applied {
                format!("Added to {count} files")
            } else {
                format!("Add tags to {count} files")
            }
        }
    }
}
pub fn review_remove_tags(tags: &[String], count: usize, applied: bool, undone: bool) -> String {
    match locale() {
        Locale::ZhCn => {
            if undone {
                format!("已撤销「{}」的 {count} 个文件标签", tags.join("、"))
            } else if applied {
                format!("已从 {count} 个文件移除")
            } else {
                format!("从 {count} 个文件移除标签")
            }
        }
        Locale::En => {
            if undone {
                format!("Undid removing \"{}\" on {count} files", tags.join(", "))
            } else if applied {
                format!("Removed from {count} files")
            } else {
                format!("Remove tags from {count} files")
            }
        }
    }
}
pub fn review_update_tags(count: usize, applied: bool, undone: bool) -> String {
    match locale() {
        Locale::ZhCn => {
            let verb = if undone { "已撤销" } else if applied { "已更新" } else { "调整" };
            format!("{verb} {count} 个文件的标签")
        }
        Locale::En => {
            let verb = if undone { "Undid" } else if applied { "Updated" } else { "Update" };
            format!("{verb} tags for {count} files")
        }
    }
}
pub fn review_rename_undone(from: &str, to: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已撤销将「{from}」重命名为「{to}」"),
        Locale::En => format!("Undid renaming \"{from}\" to \"{to}\""),
    }
}
pub fn review_rename_applied(from: &str, to: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已将「{from}」重命名为「{to}」"),
        Locale::En => format!("Renamed \"{from}\" to \"{to}\""),
    }
}
pub fn review_color_undone(tag: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已撤销更改「{tag}」的颜色"),
        Locale::En => format!("Undid color change for \"{tag}\""),
    }
}
pub fn review_color_applied(tag: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("已更改「{tag}」的颜色"),
        Locale::En => format!("Changed color for \"{tag}\""),
    }
}
pub fn tag_recolor_affected(tag: &str, affected: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("「{tag}」换色，影响 {affected} 个关联文件"),
        Locale::En => format!("Recolor \"{tag}\", affecting {affected} files"),
    }
}
pub fn dir_file_count(dir: &str, count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("{dir} · {count} 个文件"),
        Locale::En => format!("{dir} · {count} files"),
    }
}
pub fn affects_files(affected: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("影响 {affected} 个关联文件"),
        Locale::En => format!("Affects {affected} associated files"),
    }
}
pub fn tag_affects_files(tag: &str, affected: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("「{tag}」，影响 {affected} 个关联文件"),
        Locale::En => format!("\"{tag}\", affects {affected} associated files"),
    }
}

// RPC errors (src/ai/rpc.rs)
pub const ERR_RPC_DEPS_MISSING: BiText = BiText::new(
    "SDK 宿主依赖未安装，请运行 scripts/setup-ai.ps1",
    "SDK host dependencies are not installed; please run scripts/setup-ai.ps1",
);
pub fn err_rpc_spawn(e: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("启动 SDK 宿主失败：{e}"),
        Locale::En => format!("Failed to start SDK host: {e}"),
    }
}
pub fn err_rpc_protocol(error: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("宿主协议错误：{error}"),
        Locale::En => format!("Host protocol error: {error}"),
    }
}
pub const ERR_RPC_HOST_EXITED: BiText = BiText::new(
    "SDK 宿主已退出，重新打开会话以恢复历史",
    "SDK host exited; reopen the session to restore history",
);
pub const ERR_RPC_STDIN_CLOSED: BiText = BiText::new("SDK 宿主输入通道已关闭", "SDK host input channel closed");
