//! On-demand offline draw.io editor backed by WebView2 virtual host mapping and
//! draw.io's native `embed=1&proto=json` postMessage protocol.

use super::{apply_virtual_host_mapping, finish_child_webview, new_base_builder, parent_window_handle};
use crate::theme::color as rgb;
use crate::ui;
use gpui::{EventEmitter, FocusHandle, Focusable};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use gpui_webview::WebView;
use krilla::{Document, geom::Size as KrillaSize, page::PageSettings};
use krilla_svg::{SurfaceExt, SvgSettings};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
};

const BORDER: u32 = 0xe8e7e3;
const MUTED: u32 = 0x787774;
const DRAWIO_HOST: &str = "folio-drawio.localhost";

const DRAWIO_BRIDGE_SCRIPT: &str = r#"
(() => {
    if (window.__folioDrawioBridgeInstalled) return;
    window.__folioDrawioBridgeInstalled = true;
    const channel = new MessageChannel();
    const host = channel.port1;
    host.postMessage = (msg) => {
        const text = typeof msg === 'string' ? msg : JSON.stringify(msg);
        if (window.ipc && typeof window.ipc.postMessage === 'function') {
            window.ipc.postMessage(text);
        }
    };
    Object.defineProperty(window, 'opener', {
        value: host,
        configurable: true,
    });
    window.__folioSendToDrawio = (payload) => {
        const data = typeof payload === 'string' ? payload : JSON.stringify(payload);
        window.dispatchEvent(new MessageEvent('message', {
            data,
            origin: window.location.origin,
            source: host,
        }));
    };
})();
"#;

pub enum DrawioPaneEvent {
    Close,
    OpenExternal(String),
    Notice(String),
    Error(String),
}

enum DrawioInternalEvent {
    PageLoad(bool),
    Ipc(String),
    OpenExternal(String),
}

#[derive(Debug, Deserialize)]
struct DrawioIncoming {
    event: Option<String>,
    xml: Option<String>,
    data: Option<String>,
    format: Option<String>,
}

enum PendingExport {
    SaveDrawioSvg,
    SaveDrawioXml,
    SidecarSvg(PathBuf),
    SidecarPng(PathBuf),
    SidecarPdfFromSvg(PathBuf),
    SidecarPdfFromPng(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveState {
    Clean,
    Dirty,
    Saving,
}

pub struct DrawioPane {
    file_path: PathBuf,
    is_reading: bool,
    webview: Option<Entity<WebView>>,
    events_rx: Option<mpsc::Receiver<DrawioInternalEvent>>,
    init_task: Option<Task<()>>,
    visible: bool,
    attempted: bool,
    released: bool,
    loading: bool,
    ready: bool,
    save_state: SaveState,
    error: Option<String>,
    pending_exports: VecDeque<PendingExport>,
    last_written_hash: Option<[u8; 32]>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DrawioPaneEvent> for DrawioPane {}

impl Focusable for DrawioPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for DrawioPane {
    fn drop(&mut self) {
        self.released = true;
        self.init_task = None;
        self.events_rx = None;
        self.webview = None;
    }
}

impl DrawioPane {
    pub fn new(file_path: PathBuf, is_reading: bool, cx: &mut Context<Self>) -> Self {
        let canonical_file = dunce::canonicalize(&file_path).unwrap_or(file_path);
        Self {
            file_path: canonical_file,
            is_reading,
            webview: None,
            events_rx: None,
            init_task: None,
            visible: false,
            attempted: false,
            released: false,
            loading: false,
            ready: false,
            save_state: SaveState::Clean,
            error: None,
            pending_exports: VecDeque::new(),
            last_written_hash: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible && self.webview.is_none() && !self.attempted && !self.released {
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

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !self.ready {
            return;
        }
        self.save_state = SaveState::Saving;
        if is_drawio_svg(&self.file_path) {
            self.pending_exports.push_back(PendingExport::SaveDrawioSvg);
            self.send_action(json!({ "action": "export", "format": "xmlsvg" }), cx);
        } else {
            self.pending_exports.push_back(PendingExport::SaveDrawioXml);
            self.send_action(json!({ "action": "export", "format": "xml" }), cx);
        }
        cx.notify();
    }

    pub fn reload_if_external(&mut self, cx: &mut Context<Self>) {
        let Ok(bytes) = fs::read(&self.file_path) else {
            return;
        };
        let hash = sha256_bytes(&bytes);
        if self.last_written_hash == Some(hash) {
            return;
        }
        self.last_written_hash = Some(hash);
        if self.ready {
            let content = String::from_utf8_lossy(&bytes).into_owned();
            self.send_load(&content, cx);
            self.save_state = SaveState::Clean;
            cx.notify();
        }
    }

    pub fn release(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.released = true;
        self.init_task = None;
        if let Some(webview) = self.webview.take() {
            webview.update(cx, |view, _| view.hide());
        }
        self.events_rx = None;
        self.loading = false;
        self.ready = false;
        self.attempted = false;
        self.pending_exports.clear();
    }

    pub fn poll(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut messages = Vec::new();
        if let Some(rx) = &self.events_rx {
            while let Ok(event) = rx.try_recv() {
                messages.push(event);
            }
        }

        for event in messages {
            match event {
                DrawioInternalEvent::PageLoad(loading) => {
                    if self.loading != loading {
                        self.loading = loading;
                        changed = true;
                    }
                }
                DrawioInternalEvent::OpenExternal(url) => {
                    cx.emit(DrawioPaneEvent::OpenExternal(url));
                }
                DrawioInternalEvent::Ipc(raw) => {
                    self.handle_ipc(&raw, cx);
                    changed = true;
                }
            }
        }

        if changed {
            cx.notify();
        }
    }

    fn handle_ipc(&mut self, raw: &str, cx: &mut Context<Self>) {
        let Ok(msg) = serde_json::from_str::<DrawioIncoming>(raw) else {
            return;
        };
        let Some(event) = msg.event.as_deref() else {
            return;
        };
        match event {
            "init" => {
                self.ready = true;
                self.loading = false;
                self.save_state = SaveState::Clean;
                match fs::read(&self.file_path) {
                    Ok(bytes) => {
                        self.last_written_hash = Some(sha256_bytes(&bytes));
                        let xml = String::from_utf8_lossy(&bytes).into_owned();
                        self.send_load(&xml, cx);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        self.send_load("", cx);
                    }
                    Err(err) => {
                        cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_io(&err)));
                    }
                }
            }
            "autosave" => {
                self.save_state = SaveState::Dirty;
                if is_drawio_svg(&self.file_path) {
                    self.pending_exports.push_back(PendingExport::SaveDrawioSvg);
                    self.send_action(json!({ "action": "export", "format": "xmlsvg" }), cx);
                } else if let Some(xml) = msg.xml.as_deref() {
                    self.write_source_file(xml.as_bytes(), false, cx);
                }
            }
            "save" => {
                self.save_state = SaveState::Saving;
                if is_drawio_svg(&self.file_path) {
                    self.pending_exports.push_back(PendingExport::SaveDrawioSvg);
                    self.send_action(json!({ "action": "export", "format": "xmlsvg" }), cx);
                } else if let Some(xml) = msg.xml.as_deref() {
                    self.write_source_file(xml.as_bytes(), true, cx);
                }
            }
            "export" => {
                let Some(pending) = self.pending_exports.pop_front() else {
                    return;
                };
                self.handle_export(pending, msg, cx);
            }
            "exit" => {
                cx.emit(DrawioPaneEvent::Close);
            }
            _ => {}
        }
    }

    fn handle_export(
        &mut self,
        pending: PendingExport,
        msg: DrawioIncoming,
        cx: &mut Context<Self>,
    ) {
        match pending {
            PendingExport::SaveDrawioSvg => {
                let Some(data) = msg.data.as_deref() else {
                    return;
                };
                match decode_data_url(data) {
                    Ok(bytes) => self.write_source_file(&bytes, true, cx),
                    Err(err) => {
                        self.save_state = SaveState::Dirty;
                        cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_io(&err)));
                    }
                }
            }
            PendingExport::SaveDrawioXml => {
                let xml = msg.xml.as_deref().or(msg.data.as_deref()).unwrap_or_default();
                self.write_source_file(xml.as_bytes(), true, cx);
            }
            PendingExport::SidecarSvg(dest) => {
                let Some(data) = msg.data.as_deref() else {
                    return;
                };
                match decode_data_url(data).and_then(|bytes| write_bytes(&dest, &bytes)) {
                    Ok(()) => cx.emit(DrawioPaneEvent::Notice(crate::i18n::media::drawio_exported(&dest))),
                    Err(err) => cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_export(&err))),
                }
            }
            PendingExport::SidecarPng(dest) => {
                let Some(data) = msg.data.as_deref() else {
                    return;
                };
                match decode_data_url(data).and_then(|bytes| write_bytes(&dest, &bytes)) {
                    Ok(()) => cx.emit(DrawioPaneEvent::Notice(crate::i18n::media::drawio_exported(&dest))),
                    Err(err) => cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_export(&err))),
                }
            }
            PendingExport::SidecarPdfFromSvg(dest) => {
                let Some(data) = msg.data.as_deref() else {
                    return;
                };
                let bytes = match decode_data_url(data) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_export(&err)));
                        return;
                    }
                };
                let svg_text = String::from_utf8_lossy(&bytes);
                if svg_text.contains("<foreignObject") {
                    // `usvg` ignores HTML inside `<foreignObject>`; fall back to high-DPI PNG in PDF.
                    self.pending_exports.push_back(PendingExport::SidecarPdfFromPng(dest));
                    self.send_action(json!({ "action": "export", "format": "png", "scale": 3 }), cx);
                    return;
                }
                match svg_to_pdf(&svg_text).and_then(|pdf| write_bytes(&dest, &pdf)) {
                    Ok(()) => cx.emit(DrawioPaneEvent::Notice(crate::i18n::media::drawio_exported(&dest))),
                    Err(err) => cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_export(&err))),
                }
            }
            PendingExport::SidecarPdfFromPng(dest) => {
                let Some(data) = msg.data.as_deref() else {
                    return;
                };
                let result = decode_data_url(data)
                    .and_then(|png| png_to_pdf(&png))
                    .and_then(|pdf| write_bytes(&dest, &pdf));
                match result {
                    Ok(()) => cx.emit(DrawioPaneEvent::Notice(crate::i18n::media::drawio_exported(&dest))),
                    Err(err) => cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_export(&err))),
                }
            }
        }
        let _ = msg.format;
    }

    fn write_source_file(&mut self, bytes: &[u8], clear_modified: bool, cx: &mut Context<Self>) {
        match write_bytes(&self.file_path, bytes) {
            Ok(()) => {
                self.last_written_hash = Some(sha256_bytes(bytes));
                self.save_state = SaveState::Clean;
                if clear_modified {
                    self.send_action(
                        json!({
                            "action": "status",
                            "messageKey": "allChangesSaved",
                            "modified": false
                        }),
                        cx,
                    );
                }
            }
            Err(err) => {
                self.save_state = SaveState::Dirty;
                cx.emit(DrawioPaneEvent::Error(crate::i18n::media::drawio_err_io(&err)));
            }
        }
    }

    fn export_sidecar(&mut self, kind: &str, cx: &mut Context<Self>) {
        if !self.ready {
            return;
        }
        let dest = sidecar_output_path(&self.file_path, kind);
        match kind {
            "pdf" => {
                self.pending_exports
                    .push_back(PendingExport::SidecarPdfFromSvg(dest));
                self.send_action(json!({ "action": "export", "format": "svg" }), cx);
            }
            "svg" => {
                self.pending_exports.push_back(PendingExport::SidecarSvg(dest));
                self.send_action(json!({ "action": "export", "format": "svg" }), cx);
            }
            "png" => {
                self.pending_exports.push_back(PendingExport::SidecarPng(dest));
                self.send_action(
                    json!({ "action": "export", "format": "png", "scale": 2 }),
                    cx,
                );
            }
            _ => {}
        }
    }

    fn send_load(&self, xml: &str, cx: &App) {
        self.send_action(
            json!({
                "action": "load",
                "xml": xml,
                "autosave": 1,
                "title": self.display_name(),
            }),
            cx,
        );
    }

    fn send_action(&self, payload: serde_json::Value, cx: &App) {
        let Some(webview) = &self.webview else {
            return;
        };
        let Ok(encoded) = serde_json::to_string(&payload.to_string()) else {
            return;
        };
        let script = format!("window.__folioSendToDrawio && window.__folioSendToDrawio({encoded});");
        let _ = webview.read(cx).raw().evaluate_script(&script);
    }

    fn ensure_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.attempted = true;
        self.error = None;
        self.loading = true;

        let drawio_dir = match resolve_drawio_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.loading = false;
                self.error = Some(err);
                return;
            }
        };

        let parent = match parent_window_handle(window) {
            Ok(handle) => handle,
            Err(err) => {
                self.loading = false;
                self.error = Some(err);
                return;
            }
        };

        // Always load draw.io UI in Simplified Chinese while keeping outer Folio controls synced with `locale()`.
        let entry_url = format!(
            "https://{DRAWIO_HOST}/index.html?embed=1&proto=json&spin=1&offline=1&libraries=1&noExitBtn=1&saveAndExit=0&lang=zh"
        );

        let task = cx.spawn_in(window, async move |this, cx| {
            let (tx, rx) = mpsc::channel();
            let allowed_origin = format!("https://{DRAWIO_HOST}");
            let allowed_prefix = format!("{allowed_origin}/");

            let nav_tx = tx.clone();
            let nav_origin = allowed_origin.clone();
            let nav_prefix = allowed_prefix.clone();

            let new_win_tx = tx.clone();
            let load_tx = tx.clone();
            let ipc_tx = tx;

            let builder = new_base_builder()
                .with_initialization_script(DRAWIO_BRIDGE_SCRIPT)
                .with_ipc_handler(move |req| {
                    let _ = ipc_tx.send(DrawioInternalEvent::Ipc(req.into_body()));
                })
                .with_navigation_handler(move |uri| {
                    if uri == "about:blank"
                        || uri == nav_origin
                        || uri.starts_with(&nav_prefix)
                        || uri.starts_with("data:")
                        || uri.starts_with("blob:")
                    {
                        return true;
                    }
                    if uri.starts_with("http://") || uri.starts_with("https://") {
                        let _ = nav_tx.send(DrawioInternalEvent::OpenExternal(uri));
                    }
                    false
                })
                .with_new_window_req_handler(move |uri, _| {
                    if uri.starts_with("http://") || uri.starts_with("https://") {
                        let _ = new_win_tx.send(DrawioInternalEvent::OpenExternal(uri));
                    }
                    wry::NewWindowResponse::Deny
                })
                .with_on_page_load_handler(move |event, _| {
                    let is_loading = matches!(event, wry::PageLoadEvent::Started);
                    let _ = load_tx.send(DrawioInternalEvent::PageLoad(is_loading));
                });

            let result = finish_child_webview(builder, &parent).await;

            let _ = this.update_in(cx, |pane, window, cx| {
                pane.init_task = None;
                if pane.released {
                    return;
                }
                match result {
                    Ok(wry_webview) => {
                        let webview = WebView::new(wry_webview, window, cx);
                        let webview_entity = cx.new(|_| webview);
                        if let Err(err) = apply_virtual_host_mapping(
                            webview_entity.read(cx),
                            DRAWIO_HOST,
                            &drawio_dir,
                        ) {
                            pane.loading = false;
                            pane.error = Some(err);
                            cx.notify();
                            return;
                        }
                        webview_entity.update(cx, |view, _| {
                            view.load_url(&entry_url);
                            if !pane.visible {
                                view.hide();
                            }
                        });
                        pane.webview = Some(webview_entity);
                        pane.events_rx = Some(rx);
                    }
                    Err(err) => {
                        pane.loading = false;
                        pane.error = Some(crate::i18n::media::html_err_start(&err));
                    }
                }
                cx.notify();
            });
        });

        self.init_task = Some(task);
    }

    fn display_name(&self) -> String {
        self.file_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file_path.to_string_lossy().into_owned())
    }
}

impl Render for DrawioPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filename = self.display_name();
        let status_label = if self.error.is_some() {
            Some(crate::i18n::media::HTML_STATUS_FAILED.get())
        } else if self.loading || !self.ready {
            Some(crate::i18n::media::HTML_STATUS_LOADING.get())
        } else {
            match self.save_state {
                SaveState::Clean => Some(crate::i18n::media::DRAWIO_STATUS_SAVED.get()),
                SaveState::Dirty => Some(crate::i18n::media::DRAWIO_STATUS_UNSAVED.get()),
                SaveState::Saving => Some(crate::i18n::media::DRAWIO_STATUS_SAVING.get()),
            }
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
                div()
                    .flex_1()
                    .min_w_0()
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::icon(IconName::LayoutGrid).text_color(rgb(0xd97706)))
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .text_xs()
                            .font_medium()
                            .child(filename),
                    )
                    .children(status_label.map(|status| {
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("· {status}"))
                    })),
            )
            .child(
                Button::new("drawio-save")
                    .ghost()
                    .xsmall()
                    .label(crate::i18n::media::DRAWIO_SAVE.get())
                    .on_click(cx.listener(|pane, _, _, cx| pane.save(cx))),
            )
            .child(
                Button::new("drawio-export-pdf")
                    .ghost()
                    .xsmall()
                    .label(crate::i18n::media::DRAWIO_EXPORT_PDF.get())
                    .on_click(cx.listener(|pane, _, _, cx| pane.export_sidecar("pdf", cx))),
            )
            .child(
                Button::new("drawio-export-svg")
                    .ghost()
                    .xsmall()
                    .label(crate::i18n::media::DRAWIO_EXPORT_SVG.get())
                    .on_click(cx.listener(|pane, _, _, cx| pane.export_sidecar("svg", cx))),
            )
            .child(
                Button::new("drawio-export-png")
                    .ghost()
                    .xsmall()
                    .label(crate::i18n::media::DRAWIO_EXPORT_PNG.get())
                    .on_click(cx.listener(|pane, _, _, cx| pane.export_sidecar("png", cx))),
            );

        if !self.is_reading {
            toolbar = toolbar.child(
                ui::control("drawio-close", IconName::Close)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            crate::i18n::media::HTML_CLOSE_TAB.get(),
                        )
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(DrawioPaneEvent::Close);
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
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(rgb(0x37352f))
                            .child(crate::i18n::media::DRAWIO_INIT_FAILED.get()),
                    )
                    .child(div().text_xs().max_w(px(460.)).child(err.clone()))
                    .child(
                        ui::button_secondary("drawio-retry", crate::i18n::common::RETRY.get())
                            .on_click(cx.listener(|pane, _, window, cx| {
                                pane.attempted = false;
                                pane.error = None;
                                pane.ensure_webview(window, cx);
                                cx.notify();
                            })),
                    ),
            );
        } else if !self.visible {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(crate::i18n::media::DRAWIO_PAUSED.get()),
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
                    .child(crate::i18n::media::DRAWIO_INITIALIZING.get()),
            );
        }

        root
    }
}

fn resolve_drawio_dir() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let packaged = parent.join("drawio");
        if packaged.join("index.html").is_file() {
            return Ok(packaged);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/drawio");
    if dev.join("index.html").is_file() {
        return Ok(dev);
    }
    Err(crate::i18n::media::DRAWIO_ERR_MISSING_ASSETS.get().into())
}

fn is_drawio_svg(path: &Path) -> bool {
    path.to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".drawio.svg")
}

fn sidecar_output_path(path: &Path, ext: &str) -> PathBuf {
    let raw = path.to_string_lossy();
    let lower = raw.to_ascii_lowercase();
    if lower.ends_with(".drawio.svg") {
        let stem = &raw[..raw.len() - ".drawio.svg".len()];
        PathBuf::from(format!("{stem}.{ext}"))
    } else {
        path.with_extension(ext)
    }
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| e.to_string())
}

fn decode_data_url(data: &str) -> Result<Vec<u8>, String> {
    if let Some((_, b64)) = data.split_once(";base64,") {
        return decode_base64(b64);
    }
    if let Some((_, utf8)) = data.split_once(";utf8,") {
        return Ok(utf8.as_bytes().to_vec());
    }
    Ok(data.as_bytes().to_vec())
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for byte in input.bytes() {
        if byte.is_ascii_whitespace() || byte == b'=' {
            continue;
        }
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("Invalid base64 byte: {byte}")),
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

fn svg_to_pdf(svg: &str) -> Result<Vec<u8>, String> {
    let options = crate::mermaid::usvg_options();
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    let raw_size = tree.size();
    let width = (raw_size.width() * 0.75).max(1.0);
    let height = (raw_size.height() * 0.75).max(1.0);
    let size = KrillaSize::from_wh(width, height).ok_or("Invalid SVG dimensions")?;
    let settings = PageSettings::from_wh(width, height).ok_or("Invalid PDF page dimensions")?;

    let mut document = Document::new();
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();
    surface
        .draw_svg(&tree, size, SvgSettings::default())
        .ok_or("Failed to draw SVG into PDF")?;
    surface.finish();
    page.finish();
    document.finish().map_err(|e| format!("{e:?}"))
}

fn png_to_pdf(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if png_bytes.len() < 24 || &png_bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("Invalid PNG data".into());
    }
    let pixel_w = u32::from_be_bytes(png_bytes[16..20].try_into().unwrap());
    let pixel_h = u32::from_be_bytes(png_bytes[20..24].try_into().unwrap());
    let width = ((pixel_w as f32) * 0.25).max(1.0);
    let height = ((pixel_h as f32) * 0.25).max(1.0);
    let size = KrillaSize::from_wh(width, height).ok_or("Invalid PNG dimensions")?;
    let settings = PageSettings::from_wh(width, height).ok_or("Invalid PDF page dimensions")?;
    let image = krilla::image::Image::from_png(krilla::Data::from(png_bytes.to_vec()), true)
        .map_err(|e| format!("Failed to decode PNG for PDF: {e:?}"))?;

    let mut document = Document::new();
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();
    surface.draw_image(image, size);
    surface.finish();
    page.finish();
    document.finish().map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use super::{decode_data_url, sidecar_output_path};
    use std::path::{Path, PathBuf};

    #[test]
    fn sidecar_output_strips_drawio_and_drawio_svg() {
        assert_eq!(
            sidecar_output_path(Path::new("fig/arch.drawio"), "pdf"),
            PathBuf::from("fig/arch.pdf")
        );
        assert_eq!(
            sidecar_output_path(Path::new("fig/arch.drawio.svg"), "pdf"),
            PathBuf::from("fig/arch.pdf")
        );
        assert_eq!(
            sidecar_output_path(Path::new("fig/arch.drawio.svg"), "png"),
            PathBuf::from("fig/arch.png")
        );
    }

    #[test]
    fn decode_data_url_handles_base64_and_utf8() {
        assert_eq!(
            decode_data_url("data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=").unwrap(),
            b"<svg></svg>"
        );
        assert_eq!(
            decode_data_url("data:image/svg+xml;utf8,<svg/>").unwrap(),
            b"<svg/>"
        );
    }
}
