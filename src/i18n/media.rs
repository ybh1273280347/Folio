use super::{BiText, Locale, locale};
use std::path::Path;

// Translate popover (src/translate.rs)
pub const TRANSLATE_TITLE: BiText = BiText::new("翻译", "Translate");
pub const TRANSLATE_LOADING: BiText = BiText::new("正在翻译…", "Translating…");
pub const TRANSLATE_COPIED: BiText = BiText::new("已复制译文", "Translation copied");
pub const TRANSLATE_COPY: BiText = BiText::new("复制译文", "Copy translation");
pub const TRANSLATE_CLOSE_ESC: BiText = BiText::new("关闭 (Esc)", "Close (Esc)");
pub const PROVIDER_GOOGLE: BiText = BiText::new("Google 翻译", "Google Translate");
pub const PROVIDER_BAIDU: BiText = BiText::new("百度翻译", "Baidu Translate");
pub const ERR_BAIDU_NO_KEYS: BiText = BiText::new(
    "请在 设置 → API 服务 中填写百度翻译 APP ID 与密钥",
    "Please enter Baidu Translate APP ID and API Key in Settings → API Services",
);
pub const ERR_GOOGLE_FORMAT: BiText = BiText::new("Google 翻译返回格式异常", "Unexpected Google Translate response format");
pub const ERR_NO_TRANSLATION: BiText = BiText::new("未获取到有效译文", "No valid translation returned");
pub const ERR_BAIDU_NO_TRANSLATION: BiText = BiText::new("百度翻译未返回有效译文", "Baidu Translate returned no valid translation");
pub const PROVIDER_BAIDU_SHORT: BiText = BiText::new("百度", "Baidu");

pub fn http_init_session(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("初始化 {provider} WinHTTP 会话失败"),
        Locale::En => format!("Failed to initialize {provider} WinHTTP session"),
    }
}
pub fn http_connect(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("连接 {provider} 失败"),
        Locale::En => format!("Failed to connect to {provider}"),
    }
}
pub fn http_open_req(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("创建 {provider} 请求失败"),
        Locale::En => format!("Failed to create {provider} request"),
    }
}
pub fn http_send_req(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("请求 {provider} 失败（网络超时或未通）"),
        Locale::En => format!("Request to {provider} failed (timeout or unreachable)"),
    }
}
pub fn http_recv_timeout(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("接收 {provider} 响应超时"),
        Locale::En => format!("Timed out receiving response from {provider}"),
    }
}
pub fn http_rate_limit(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{provider} 请求过于频繁 (HTTP 429)"),
        Locale::En => format!("{provider} rate limit exceeded (HTTP 429)"),
    }
}
pub fn http_status(provider: &str, status: u32) -> String {
    match locale() {
        Locale::ZhCn => format!("{provider} 返回状态码 HTTP {status}"),
        Locale::En => format!("{provider} returned HTTP {status}"),
    }
}
pub fn http_read_err(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("读取 {provider} 响应出错"),
        Locale::En => format!("Failed to read {provider} response"),
    }
}
pub fn http_too_large(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{provider} 响应体积过大"),
        Locale::En => format!("{provider} response too large"),
    }
}
pub fn http_invalid_utf8(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{provider} 响应不是有效的 UTF-8 文本"),
        Locale::En => format!("{provider} response is not valid UTF-8"),
    }
}
#[cfg(not(windows))]
pub fn curl_invoke_failed(provider: &str, err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("调用 curl 请求 {provider} 失败：{err}"),
        Locale::En => format!("Failed to invoke curl for {provider}: {err}"),
    }
}
#[cfg(not(windows))]
pub fn curl_req_failed(provider: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("请求 {provider} 失败"),
        Locale::En => format!("Request to {provider} failed"),
    }
}

pub fn translate_auto_arrow(target: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("自动 → {target}"),
        Locale::En => format!("Auto → {target}"),
    }
}
pub fn lang_name(code: &str) -> String {
    match (locale(), code.to_ascii_lowercase().as_str()) {
        (Locale::ZhCn, "zh-cn" | "zh" | "zh-hans") => "中文".to_owned(),
        (Locale::ZhCn, "zh-tw" | "zh-hant" | "cht") => "繁中".to_owned(),
        (Locale::ZhCn, "en") => "英语".to_owned(),
        (Locale::ZhCn, "ja" | "jp") => "日语".to_owned(),
        (Locale::ZhCn, "ko" | "kor") => "韩语".to_owned(),
        (Locale::ZhCn, "fr" | "fra") => "法语".to_owned(),
        (Locale::ZhCn, "de") => "德语".to_owned(),
        (Locale::ZhCn, "es" | "spa") => "西语".to_owned(),
        (Locale::ZhCn, "ru") => "俄语".to_owned(),
        (Locale::ZhCn, "la") => "拉丁语".to_owned(),

        (Locale::En, "zh-cn" | "zh" | "zh-hans") => "Chinese".to_owned(),
        (Locale::En, "zh-tw" | "zh-hant" | "cht") => "Trad. Chinese".to_owned(),
        (Locale::En, "en") => "English".to_owned(),
        (Locale::En, "ja" | "jp") => "Japanese".to_owned(),
        (Locale::En, "ko" | "kor") => "Korean".to_owned(),
        (Locale::En, "fr" | "fra") => "French".to_owned(),
        (Locale::En, "de") => "German".to_owned(),
        (Locale::En, "es" | "spa") => "Spanish".to_owned(),
        (Locale::En, "ru") => "Russian".to_owned(),
        (Locale::En, "la") => "Latin".to_owned(),
        (_, other) => other.to_owned(),
    }
}
pub fn err_google_and_baidu(google_err: &str, baidu_err: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{google_err}；百度备用也失败：{baidu_err}"),
        Locale::En => format!("{google_err}; Baidu fallback failed: {baidu_err}"),
    }
}
pub fn err_parse_google(e: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("解析 Google 翻译响应失败：{e}"),
        Locale::En => format!("Failed to parse Google Translate response: {e}"),
    }
}
pub fn err_parse_baidu(e: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("解析百度翻译响应失败：{e}"),
        Locale::En => format!("Failed to parse Baidu Translate response: {e}"),
    }
}
pub fn err_baidu_code(code: &str, msg: &str) -> String {
    let detail = match (locale(), code) {
        (Locale::ZhCn, "52001") => "请求超时",
        (Locale::ZhCn, "52002") => "百度服务系统错误",
        (Locale::ZhCn, "52003") => "未授权用户，请检查 APP ID 是否正确及是否已开通通用文本翻译",
        (Locale::ZhCn, "54001") => "签名错误，请检查 APP ID 与密钥是否匹配",
        (Locale::ZhCn, "54003") => "访问频率受限，请稍后再试",
        (Locale::ZhCn, "54004") => "百度翻译账户余额不足",
        (Locale::ZhCn, "54005") => "请求文本过长",
        (Locale::ZhCn, "58000") => "客户端 IP 非法（如设置了 IP 白名单）",
        (Locale::En, "52001") => "Request timed out",
        (Locale::En, "52002") => "Baidu service system error",
        (Locale::En, "52003") => "Unauthorized user; check APP ID and General Text Translation access",
        (Locale::En, "54001") => "Signature error; verify APP ID and API Key match",
        (Locale::En, "54003") => "Rate limit exceeded; try again later",
        (Locale::En, "54004") => "Insufficient Baidu Translate account balance",
        (Locale::En, "54005") => "Query text too long",
        (Locale::En, "58000") => "Invalid client IP (check IP allowlist)",
        _ => msg,
    };
    match locale() {
        Locale::ZhCn => format!("百度翻译错误 ({code})：{detail}"),
        Locale::En => format!("Baidu Translate error ({code}): {detail}"),
    }
}

// PDF annotations (src/pdf.rs)
pub const PDF_ANN_PLACEHOLDER: BiText = BiText::new("输入 PDF 批注内容（回车保存）…", "Enter PDF annotation (Enter to save)…");
pub const PDF_ANN_EMPTY: BiText = BiText::new(
    "暂无批注。直接拖拽选中 PDF 文字，或点击顶部 ⬚（或按住 Alt 拖拽框选）即可高亮与批注区域内的有效内容。",
    "No annotations yet. Drag to select PDF text, or click ⬚ (or Alt+Drag) to highlight and annotate a region.",
);
pub const PDF_ANN_REGION_LABEL: BiText = BiText::new("框选区域", "Selected region");

pub fn pdf_ann_count(count: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("PDF 批注 ({count})"),
        Locale::En => format!("PDF Annotations ({count})"),
    }
}
pub fn pdf_ann_page_region(page_1: usize) -> String {
    match locale() {
        Locale::ZhCn => format!("第 {page_1} 页 · 框选区域批注"),
        Locale::En => format!("Page {page_1} · Region annotation"),
    }
}
pub fn pdf_ann_page_quote(page_1: usize, quote: &str, truncated: bool) -> String {
    match (locale(), truncated) {
        (Locale::ZhCn, true) => format!("第 {page_1} 页 · “{quote}…”"),
        (Locale::ZhCn, false) => format!("第 {page_1} 页 · “{quote}”"),
        (Locale::En, true) => format!("Page {page_1} · \"{quote}…\""),
        (Locale::En, false) => format!("Page {page_1} · \"{quote}\""),
    }
}

// HTML WebView2 preview (src/html.rs)
pub const HTML_ERR_NO_HWND: BiText = BiText::new("无法获取父窗口句柄", "Failed to get parent window handle");
pub const HTML_STATUS_FAILED: BiText = BiText::new("加载失败", "Failed to load");
pub const HTML_STATUS_LOADING: BiText = BiText::new("正在加载…", "Loading…");
pub const HTML_BACK: BiText = BiText::new("后退", "Back");
pub const HTML_FORWARD: BiText = BiText::new("前进", "Forward");
pub const HTML_REFRESH: BiText = BiText::new("刷新页面", "Refresh");
pub const HTML_OPEN_BROWSER: BiText = BiText::new("在默认浏览器中打开", "Open in default browser");
pub const HTML_CLOSE_TAB: BiText = BiText::new("关闭标签页", "Close tab");
pub const HTML_INIT_FAILED: BiText = BiText::new("WebView2 初始化失败", "WebView2 initialization failed");
pub const HTML_PAUSED: BiText = BiText::new("网页视图已暂停显示", "Web preview paused");
pub const HTML_INITIALIZING: BiText = BiText::new("正在初始化 WebView2…", "Initializing WebView2…");
pub const HTML_ERR_ENV_NULL: BiText = BiText::new("WebView2 环境回调返回空指针", "WebView2 environment callback returned null");
#[cfg(not(windows))]
pub const HTML_ERR_NON_WINDOWS: BiText = BiText::new("非 Windows 平台暂不支持 WebView2 虚拟主机映射", "WebView2 virtual host mapping is only supported on Windows");

pub fn html_err_start(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法启动 WebView2：{err}"),
        Locale::En => format!("Failed to start WebView2: {err}"),
    }
}
pub fn html_err_env_create(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("WebView2 环境创建失败：{err}"),
        Locale::En => format!("Failed to create WebView2 environment: {err}"),
    }
}
pub fn html_err_env_options(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("CreateCoreWebView2EnvironmentWithOptions 调用失败：{err}"),
        Locale::En => format!("CreateCoreWebView2EnvironmentWithOptions failed: {err}"),
    }
}
pub fn html_err_isolated_retry(first: &str, second: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("{first}；隔离目录重试仍失败：{second}"),
        Locale::En => format!("{first}; retry with isolated directory failed: {second}"),
    }
}
pub fn html_err_webview2_3(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法获取 ICoreWebView2_3 接口：{err}"),
        Locale::En => format!("Failed to acquire ICoreWebView2_3 interface: {err}"),
    }
}
pub fn html_err_host_mapping(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("SetVirtualHostNameToFolderMapping 失败：{err}"),
        Locale::En => format!("SetVirtualHostNameToFolderMapping failed: {err}"),
    }
}

// Mermaid, Math, Compiler, Build Log, Markdown PDF
pub const MERMAID_PENDING: BiText = BiText::new("图表排版中…", "Rendering diagram…");
pub const MERMAID_EMPTY: BiText = BiText::new("Mermaid 图表为空", "Mermaid diagram is empty");
pub const MERMAID_NO_SVG: BiText = BiText::new("未生成 SVG 输出", "No SVG output generated");
pub const MERMAID_BITMAP_ALLOC: BiText = BiText::new("无法分配图表位图", "Failed to allocate diagram bitmap");
pub const MERMAID_BITMAP_INVALID: BiText = BiText::new("图表位图数据无效", "Invalid diagram bitmap data");

pub fn mermaid_render_failed(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("Mermaid 渲染失败：{err}"),
        Locale::En => format!("Mermaid render failed: {err}"),
    }
}
pub fn mermaid_svg_parse_failed(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("SVG 解析失败：{err}"),
        Locale::En => format!("SVG parse failed: {err}"),
    }
}

pub const MATH_PENDING: BiText = BiText::new("公式排版中…", "Typesetting formula…");
pub fn math_layout_failed(latex: &str, err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("{latex}  ·  公式排版失败：{err}"),
        Locale::En => format!("{latex}  ·  Formula layout failed: {err}"),
    }
}
pub fn cjk_font_load_failed(msg: &str) -> String {
    match locale() {
        Locale::ZhCn => format!("中文字体加载失败：{msg}"),
        Locale::En => format!("Failed to load CJK font: {msg}"),
    }
}

pub const COMPILER_MAIN_TEX_ONLY: BiText = BiText::new("主文件必须是 .tex 文件。", "Main file must be a .tex file.");
pub const BUILD_LOG_TITLE: BiText = BiText::new("编译日志", "Build Logs");
pub const BUILD_LOG_COPY_ALL: BiText = BiText::new("复制全部", "Copy All");

pub const MD_PDF_CANCELLED: BiText = BiText::new("PDF 导出已取消。", "PDF export cancelled.");
pub const MD_PDF_FONT_EMBED: BiText = BiText::new("无法将当前字体嵌入 PDF", "Unable to embed current font into PDF");
pub const MD_PDF_GRADIENT_BG: BiText = BiText::new("PDF 不支持当前渐变背景", "PDF export does not support this gradient background");
pub const MD_PDF_GRADIENT_PATH: BiText = BiText::new("PDF 不支持当前渐变路径", "PDF export does not support this gradient path");
pub const MD_PDF_MERMAID_SIZE: BiText = BiText::new("Mermaid 图表尺寸无效", "Invalid Mermaid diagram size");
pub const MD_PDF_MERMAID_DRAW: BiText = BiText::new("Mermaid 图表绘制失败", "Failed to draw Mermaid diagram");

pub fn md_pdf_mermaid_svg(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("Mermaid SVG 解析失败：{err}"),
        Locale::En => format!("Failed to parse Mermaid SVG: {err}"),
    }
}
pub fn md_pdf_write_failed(err: &dyn std::fmt::Debug) -> String {
    match locale() {
        Locale::ZhCn => format!("PDF 写入失败：{err:?}"),
        Locale::En => format!("Failed to write PDF: {err:?}"),
    }
}
pub fn md_pdf_exported(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("已按当前 Markdown 字体、公式、换行导出矢量 PDF（文字可选择和复制）：{}", path.display()),
        Locale::En => format!("Exported vector PDF with current Markdown fonts, math, and wrapping (selectable text): {}", path.display()),
    }
}

// Terminal (src/terminal/)
pub const DRAG_LOOSE_FILE: BiText = BiText::new("独立文件不可删除", "Loose file cannot be deleted");
pub const DRAG_INVALID_FOLDER: BiText = BiText::new("不能移入这里", "Cannot move here");
pub const DRAG_TRASH: BiText = BiText::new("移入废纸篓", "Move to Trash");
pub const DRAG_PROJECT_ROOT: BiText = BiText::new("项目根目录", "Project root");
pub const DRAG_OPEN_MAIN: BiText = BiText::new("主标签页打开", "Open in tab");
pub const DRAG_OPEN_SPLIT: BiText = BiText::new("右侧分屏打开", "Open to right");
pub const TERM_PROCESS_ENDED: BiText = BiText::new("终端进程已结束", "Terminal process ended");
pub const TERM_STARTING: BiText = BiText::new("正在启动…", "Starting…");
pub const TERM_NEW_OR_AGENT: BiText = BiText::new("新建终端或启动 Agent", "New terminal or launch Agent");
pub const TERM_MENU_SHELL: BiText = BiText::new("终端", "Terminal");
pub const TERM_MENU_START_AGENT: BiText = BiText::new("启动 Agent", "Start an agent");
pub const TERM_CLICK_PLUS_HINT: BiText = BiText::new("点击“+”新建终端", "Click \"+\" to start a terminal");

pub fn term_exited_code(code: impl std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("已退出 · {code}"),
        Locale::En => format!("Exited · {code}"),
    }
}
pub fn term_exit_status_err(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("读取退出状态失败：{err}"),
        Locale::En => format!("Failed to read exit status: {err}"),
    }
}
pub fn term_default_title(id: u64) -> String {
    match locale() {
        Locale::ZhCn => format!("终端 {id}"),
        Locale::En => format!("Terminal {id}"),
    }
}
pub fn term_err_conpty(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法创建 Windows ConPTY：{err}"),
        Locale::En => format!("Failed to create Windows ConPTY: {err}"),
    }
}
pub fn term_err_spawn(shell: &Path, err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法启动 {}：{err}", shell.display()),
        Locale::En => format!("Failed to launch {}: {err}", shell.display()),
    }
}
pub fn term_err_job(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法管理终端进程：{err}"),
        Locale::En => format!("Failed to manage terminal process: {err}"),
    }
}

// Draw.io WebView2 editor (src/web_view/drawio.rs)
pub const DRAWIO_INITIALIZING: BiText = BiText::new("正在初始化 draw.io…", "Initializing draw.io…");
pub const DRAWIO_PAUSED: BiText = BiText::new("绘图视图已暂停显示", "Diagram editor paused");
pub const DRAWIO_INIT_FAILED: BiText = BiText::new("draw.io 初始化失败", "draw.io initialization failed");
pub const DRAWIO_STATUS_SAVED: BiText = BiText::new("已保存", "Saved");
pub const DRAWIO_STATUS_SAVING: BiText = BiText::new("保存中…", "Saving…");
pub const DRAWIO_STATUS_UNSAVED: BiText = BiText::new("未保存", "Unsaved");
pub const DRAWIO_SAVE: BiText = BiText::new("保存", "Save");
pub const DRAWIO_EXPORT_PDF: BiText = BiText::new("导出 PDF", "Export PDF");
pub const DRAWIO_EXPORT_SVG: BiText = BiText::new("导出 SVG", "Export SVG");
pub const DRAWIO_EXPORT_PNG: BiText = BiText::new("导出 PNG", "Export PNG");
pub const DRAWIO_ERR_MISSING_ASSETS: BiText = BiText::new(
    "未找到 draw.io 离线资源目录（resources/drawio），请先运行 scripts/install-drawio.ps1",
    "draw.io offline assets (resources/drawio) not found; run scripts/install-drawio.ps1 first",
);
pub fn drawio_exported(path: &Path) -> String {
    match locale() {
        Locale::ZhCn => format!("已导出：{}", path.display()),
        Locale::En => format!("Exported: {}", path.display()),
    }
}
pub fn drawio_err_io(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("draw.io 文件读写失败：{err}"),
        Locale::En => format!("draw.io file I/O failed: {err}"),
    }
}
pub fn drawio_err_export(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("draw.io 导出失败：{err}"),
        Locale::En => format!("draw.io export failed: {err}"),
    }
}

// Image viewer (src/image/viewer.rs)
pub const IMG_ACTUAL_SIZE: BiText = BiText::new("100%", "100%");
pub const IMG_TOOLTIP_ZOOM_OUT: BiText = BiText::new("缩小 (Ctrl+-)", "Zoom Out (Ctrl+-)");
pub const IMG_TOOLTIP_ZOOM_IN: BiText = BiText::new("放大 (Ctrl+=)", "Zoom In (Ctrl+=)");
pub const IMG_TOOLTIP_FIT: BiText = BiText::new("适应窗口 (Ctrl+0)", "Fit to Window (Ctrl+0)");
pub const IMG_TOOLTIP_ACTUAL: BiText = BiText::new("实际大小 100% (Ctrl+1)", "Actual Size 100% (Ctrl+1)");
pub const IMG_SVG_COMPLEX_NOTICE: BiText = BiText::new(
    "检测到公式或表格等复杂富文本，静态预览可能不完整；建议在 draw.io 导出时取消勾选「格式化文本」或改用内置 .drawio 编辑器打开。",
    "Complex rich text (math/table) detected; static SVG preview may be incomplete. Uncheck 'Formatted Text' in draw.io or open as .drawio.",
);
pub fn img_load_failed(err: &dyn std::fmt::Display) -> String {
    match locale() {
        Locale::ZhCn => format!("无法加载图像：{err}"),
        Locale::En => format!("Failed to load image: {err}"),
    }
}
