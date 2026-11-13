//! On-demand WebView2 HTML viewer backed by gpui-webview and wry.
//! Local relative assets (CSS, JS, images) are served via WebView2's native
//! virtual host mapping (`SetVirtualHostNameToFolderMapping`).
use crate::storage;
use crate::theme::color as rgb;
use crate::ui;
use gpui::{EventEmitter, FocusHandle, Focusable, Subscription};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::*;
use gpui_webview::WebView;
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::mpsc,
};

const BORDER: u32 = 0xe8e7e3;
const MUTED: u32 = 0x787774;

pub enum HtmlPaneEvent {
    Close,
    OpenExternal(String),
}

enum HtmlInternalEvent {
    PageLoad(bool),
    OpenExternal(String),
    NavigateInternal(String),
}

pub struct HtmlPane {
    file_path: PathBuf,
    root_dir: PathBuf,
    virtual_host: String,
    entry_url: String,
    fallback_url: String,
    is_reading: bool,
    webview: Option<Entity<WebView>>,
    web_context: Option<wry::WebContext>,
    events_rx: Option<mpsc::Receiver<HtmlInternalEvent>>,
    visible: bool,
    attempted: bool,
    loading: bool,
    using_fallback: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HtmlPaneEvent> for HtmlPane {}

impl Focusable for HtmlPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for HtmlPane {
    fn drop(&mut self) {
        self.events_rx = None;
        self.webview = None;
        self.web_context = None;
    }
}

impl HtmlPane {
    pub fn new(
        file_path: PathBuf,
        root_dir: PathBuf,
        is_reading: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let canonical_root = dunce::canonicalize(&root_dir).unwrap_or(root_dir);
        let canonical_file = dunce::canonicalize(&file_path).unwrap_or(file_path);
        let virtual_host = virtual_host_for_root(&canonical_root);
        let entry_url = build_virtual_url(&virtual_host, &canonical_root, &canonical_file);
        let fallback_url = url::Url::from_file_path(&canonical_file)
            .map(|url| url.into())
            .unwrap_or_else(|_| format!("file:///{}", canonical_file.to_string_lossy().replace('\\', "/")));

        Self {
            file_path: canonical_file,
            root_dir: canonical_root,
            virtual_host,
            entry_url,
            fallback_url,
            is_reading,
            webview: None,
            web_context: None,
            events_rx: None,
            visible: false,
            attempted: false,
            loading: false,
            using_fallback: false,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: Vec::new(),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn set_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.visible = visible;
        if visible && self.webview.is_none() && !self.attempted {
            self.ensure_webview(window, cx);
        }
        if let Some(webview) = &self.webview {
            let currently_visible = webview.read(cx).visible();
            if currently_visible != visible {
                webview.update(cx, |view, _| {
                    if visible {
                        view.show();
                    } else {
                        view.hide();
                    }
                });
            }
        }
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(webview) = &self.webview {
            self.loading = true;
            let _ = webview.read(cx).raw().reload();
            cx.notify();
        } else if self.visible {
            self.attempted = false;
            self.error = None;
            self.ensure_webview(window, cx);
            cx.notify();
        }
    }

    pub fn release(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(webview) = self.webview.take() {
            webview.update(cx, |view, _| {
                view.hide();
            });
        }
        self.events_rx = None;
        self.web_context = None;
        self.loading = false;
        self.attempted = false;
    }

    pub fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut external_urls = Vec::new();
        let mut internal_urls = Vec::new();
        if let Some(rx) = &self.events_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    HtmlInternalEvent::PageLoad(loading) => {
                        if self.loading != loading {
                            self.loading = loading;
                            changed = true;
                        }
                    }
                    HtmlInternalEvent::OpenExternal(url) => external_urls.push(url),
                    HtmlInternalEvent::NavigateInternal(url) => internal_urls.push(url),
                }
            }
        }
        for url in internal_urls {
            if let Some(webview) = &self.webview {
                webview.update(cx, |view, _| view.load_url(&url));
            }
        }
        for url in external_urls {
            cx.emit(HtmlPaneEvent::OpenExternal(url));
        }
        if changed {
            let _ = window;
            cx.notify();
        }
    }

    fn ensure_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.attempted = true;
        self.error = None;
        self.using_fallback = false;

        let data_dir = storage::data_directory().ok().map(|dir| dir.join("webview2"));
        if let Some(dir) = &data_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut context = wry::WebContext::new(data_dir);

        let (tx, rx) = mpsc::channel();
        let allowed_origin = format!("https://{}", self.virtual_host);
        let allowed_prefix = format!("{allowed_origin}/");
        let fallback_prefix = url::Url::from_directory_path(&self.root_dir)
            .map(|u| u.to_string())
            .unwrap_or_default();

        let nav_tx = tx.clone();
        let nav_origin = allowed_origin.clone();
        let nav_prefix = allowed_prefix.clone();
        let nav_fallback = fallback_prefix.clone();

        let new_win_tx = tx.clone();
        let new_win_origin = allowed_origin;
        let new_win_prefix = allowed_prefix;

        let load_tx = tx;

        let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_devtools(cfg!(debug_assertions))
            .with_navigation_handler(move |uri| {
                if uri == "about:blank"
                    || uri == nav_origin
                    || uri.starts_with(&nav_prefix)
                    || uri.starts_with("data:")
                    || uri.starts_with("blob:")
                    || (!nav_fallback.is_empty() && uri.starts_with(&nav_fallback))
                {
                    return true;
                }
                if uri.starts_with("http://") || uri.starts_with("https://") {
                    let _ = nav_tx.send(HtmlInternalEvent::OpenExternal(uri));
                }
                false
            })
            .with_new_window_req_handler(move |uri, _| {
                if uri == new_win_origin || uri.starts_with(&new_win_prefix) {
                    let _ = new_win_tx.send(HtmlInternalEvent::NavigateInternal(uri));
                } else if uri.starts_with("http://") || uri.starts_with("https://") {
                    let _ = new_win_tx.send(HtmlInternalEvent::OpenExternal(uri));
                }
                wry::NewWindowResponse::Deny
            })
            .with_on_page_load_handler(move |event, _| {
                let is_loading = matches!(event, wry::PageLoadEvent::Started);
                let _ = load_tx.send(HtmlInternalEvent::PageLoad(is_loading));
            });

        match WebView::build(builder, window, cx) {
            Ok(webview) => {
                let webview_entity = cx.new(|_| webview);
                let mapping_result = webview_entity.read(cx);
                let map_res = apply_virtual_host_mapping(
                    mapping_result,
                    &self.virtual_host,
                    &self.root_dir,
                );
                let target_url = match map_res {
                    Ok(()) => self.entry_url.clone(),
                    Err(err) => {
                        eprintln!("WebView2 virtual host mapping failed, falling back to file URL: {err}");
                        self.using_fallback = true;
                        self.fallback_url.clone()
                    }
                };
                self.loading = true;
                webview_entity.update(cx, |view, _| {
                    view.load_url(&target_url);
                    if !self.visible {
                        view.hide();
                    }
                });
                self.webview = Some(webview_entity);
                self.web_context = Some(context);
                self.events_rx = Some(rx);
            }
            Err(err) => {
                self.loading = false;
                self.error = Some(format!("无法启动 WebView2：{err}"));
            }
        }
    }

    fn display_name(&self) -> String {
        self.file_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file_path.to_string_lossy().into_owned())
    }

    fn open_in_external_browser(&self, cx: &mut Context<Self>) {
        cx.open_url(&self.fallback_url);
    }
}

impl Render for HtmlPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filename = self.display_name();
        let status_label = if self.error.is_some() {
            "加载失败"
        } else if self.loading {
            "正在加载…"
        } else if self.using_fallback {
            "本地文件回退模式"
        } else {
            "WebView2 · 虚拟主机映射"
        };

        let mut root = div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff));

        let mut toolbar = div()
            .h(px(34.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .bg(rgb(0xfafaf9))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                ui::control("html-back", IconName::ChevronLeft)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("后退").build(window, cx))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        if let Some(wv) = &pane.webview {
                            wv.update(cx, |view, _| {
                                let _ = view.back();
                            });
                        }
                    })),
            )
            .child(
                ui::control("html-forward", IconName::ChevronRight)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("前进").build(window, cx))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        if let Some(wv) = &pane.webview {
                            wv.update(cx, |view, _| {
                                let _ = view.forward();
                            });
                        }
                    })),
            )
            .child(
                ui::control("html-reload", IconName::RotateCcw)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("刷新页面").build(window, cx))
                    .on_click(cx.listener(|pane, _, window, cx| {
                        pane.reload(window, cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::icon(IconName::Globe).text_color(rgb(0xd97706)))
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .text_xs()
                            .font_medium()
                            .child(filename),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("· {status_label}")),
                    ),
            )
            .child(
                ui::control("html-external", IconName::ExternalLink)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("在默认浏览器中打开").build(window, cx))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.open_in_external_browser(cx);
                    })),
            );

        if !self.is_reading {
            toolbar = toolbar.child(
                ui::control("html-close", IconName::Close)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("关闭标签页").build(window, cx))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(HtmlPaneEvent::Close);
                    })),
            );
        }

        root = root.child(toolbar);

        if let Some(err) = &self.error {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .p_6()
                    .text_color(rgb(MUTED))
                    .child(div().text_sm().font_semibold().text_color(rgb(0x37352f)).child("WebView2 初始化失败"))
                    .child(div().text_xs().max_w(px(460.)).child(err.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(ui::button_secondary("html-retry", "重试").on_click(cx.listener(
                                |pane, _, window, cx| {
                                    pane.attempted = false;
                                    pane.error = None;
                                    pane.ensure_webview(window, cx);
                                    cx.notify();
                                },
                            )))
                            .child(ui::button_primary("html-open-browser", "在默认浏览器中打开").on_click(
                                cx.listener(|pane, _, _, cx| {
                                    pane.open_in_external_browser(cx);
                                }),
                            )),
                    ),
            );
        } else if !self.visible {
            // Keep GPUI layout clean while hidden (e.g. when a modal overlay is open).
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("网页视图已暂停显示"),
            );
        } else if let Some(webview) = &self.webview {
            root = root.child(div().flex_1().min_h_0().w_full().child(webview.clone()));
        } else {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("正在初始化 WebView2…"),
            );
        }

        root
    }
}

fn virtual_host_for_root(root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let short: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("folio-{short}.localhost")
}

fn build_virtual_url(host: &str, root: &Path, file: &Path) -> String {
    let mut url = url::Url::parse(&format!("https://{host}/")).expect("valid virtual host url");
    if let Ok(relative) = file.strip_prefix(root) {
        if let Ok(mut segments) = url.path_segments_mut() {
            for component in relative.components() {
                if let Component::Normal(part) = component {
                    segments.push(&part.to_string_lossy());
                }
            }
        }
    } else if let Some(name) = file.file_name() {
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.push(&name.to_string_lossy());
        }
    }
    url.into()
}

#[cfg(target_os = "windows")]
fn apply_virtual_host_mapping(
    webview: &WebView,
    host: &str,
    folder: &Path,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW, ICoreWebView2_3,
    };
    use windows_core::{HSTRING, Interface};
    use wry::WebViewExtWindows;

    let core = webview.raw().webview();
    let core3: ICoreWebView2_3 = core
        .cast()
        .map_err(|e| format!("无法获取 ICoreWebView2_3 接口：{e}"))?;
    let host_h = HSTRING::from(host);
    let folder_h = HSTRING::from(folder.to_string_lossy().as_ref());
    unsafe {
        core3.SetVirtualHostNameToFolderMapping(
            &host_h,
            &folder_h,
            COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
        )
    }
    .map_err(|e| format!("SetVirtualHostNameToFolderMapping 失败：{e}"))
}

#[cfg(not(target_os = "windows"))]
fn apply_virtual_host_mapping(
    _webview: &WebView,
    _host: &str,
    _folder: &Path,
) -> Result<(), String> {
    Err("非 Windows 平台暂不支持 WebView2 虚拟主机映射".into())
}
