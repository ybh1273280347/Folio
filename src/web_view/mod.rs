//! Shared WebView2 infrastructure for embedded web-rendered panes (HTML, draw.io, etc.).

pub mod drawio;
pub mod html;

pub use drawio::{DrawioPane, DrawioPaneEvent};
pub use html::{HtmlPane, HtmlPaneEvent};

use crate::storage;
use gpui_kit::Window;
use gpui_webview::WebView;
use std::{
    cell::RefCell,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Waker},
};

pub(crate) struct ParentWindowHandle(raw_window_handle::RawWindowHandle);

impl raw_window_handle::HasWindowHandle for ParentWindowHandle {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: Callers only await creation inside `cx.spawn_in(window, ...)`,
        // which is scoped to the live GPUI `Window` on the main UI thread.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

pub(crate) fn parent_window_handle(window: &Window) -> Result<ParentWindowHandle, String> {
    raw_window_handle::HasWindowHandle::window_handle(window)
        .map(|handle| ParentWindowHandle(handle.as_raw()))
        .map_err(|_| crate::i18n::media::HTML_ERR_NO_HWND.get().into())
}

pub(crate) fn new_base_builder<'a>() -> wry::WebViewBuilder<'a> {
    wry::WebViewBuilder::new()
        .with_bounds(wry::Rect::default())
        .with_hotkeys_zoom(true)
        .with_devtools(cfg!(debug_assertions))
}

pub(crate) async fn finish_child_webview(
    mut builder: wry::WebViewBuilder<'_>,
    parent: &ParentWindowHandle,
) -> Result<wry::WebView, String> {
    #[cfg(target_os = "windows")]
    {
        use wry::WebViewBuilderExtWindows;
        let data_dir = storage::data_directory().ok().map(|dir| dir.join("webview2"));
        let env = get_or_create_environment_async(data_dir).await?;
        builder = builder.with_environment(env);
    }

    builder
        .build_as_child_async(parent)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
thread_local! {
    static SHARED_WEBVIEW2_ENV: RefCell<Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment>> = const { RefCell::new(None) };
}

#[cfg(target_os = "windows")]
struct OneshotState<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

#[cfg(target_os = "windows")]
struct OneshotFuture<T>(Arc<Mutex<OneshotState<T>>>);

#[cfg(target_os = "windows")]
impl<T> Future for OneshotFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let mut state = self.0.lock().expect("oneshot mutex");
        if let Some(value) = state.value.take() {
            Poll::Ready(value)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[cfg(target_os = "windows")]
async fn create_environment_for_dir(
    data_dir: &Path,
) -> Result<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment, String> {
    use webview2_com::{
        CreateCoreWebView2EnvironmentCompletedHandler,
        Microsoft::Web::WebView2::Win32::CreateCoreWebView2EnvironmentWithOptions,
    };
    use windows_core::{HSTRING, PCWSTR};

    let _ = std::fs::create_dir_all(data_dir);
    let state = Arc::new(Mutex::new(OneshotState {
        value: None,
        waker: None,
    }));
    let sender = state.clone();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |error_code, environment| {
            let outcome = match error_code {
                Ok(()) => environment
                    .ok_or_else(|| crate::i18n::media::HTML_ERR_ENV_NULL.get().to_owned()),
                Err(err) => Err(crate::i18n::media::html_err_env_create(&err)),
            };
            let mut guard = sender.lock().expect("oneshot mutex");
            guard.value = Some(outcome);
            if let Some(waker) = guard.waker.take() {
                waker.wake();
            }
            Ok(())
        },
    ));
    let dir_h = HSTRING::from(data_dir.to_string_lossy().as_ref());
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &dir_h, None, &handler)
    }
    .map_err(|e| crate::i18n::media::html_err_env_options(&e))?;

    OneshotFuture(state).await
}

#[cfg(target_os = "windows")]
async fn get_or_create_environment_async(
    primary_dir: Option<PathBuf>,
) -> Result<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment, String> {
    if let Some(env) = SHARED_WEBVIEW2_ENV.with(|cell| cell.borrow().clone()) {
        return Ok(env);
    }
    let primary = primary_dir.unwrap_or_else(|| std::env::temp_dir().join("folio-gpui-webview2"));
    let env = match create_environment_for_dir(&primary).await {
        Ok(env) => env,
        Err(first_err) => {
            let isolated = primary.with_file_name(format!("webview2-{}", std::process::id()));
            create_environment_for_dir(&isolated).await.map_err(|second_err| {
                crate::i18n::media::html_err_isolated_retry(&first_err, &second_err)
            })?
        }
    };
    SHARED_WEBVIEW2_ENV.with(|cell| {
        *cell.borrow_mut() = Some(env.clone());
    });
    Ok(env)
}

#[cfg(target_os = "windows")]
pub(crate) fn apply_virtual_host_mapping(
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
        .map_err(|e| crate::i18n::media::html_err_webview2_3(&e))?;
    let host_h = HSTRING::from(host);
    let folder_h = HSTRING::from(folder.to_string_lossy().as_ref());
    unsafe {
        core3.SetVirtualHostNameToFolderMapping(
            &host_h,
            &folder_h,
            COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
        )
    }
    .map_err(|e| crate::i18n::media::html_err_host_mapping(&e))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn apply_virtual_host_mapping(
    _webview: &WebView,
    _host: &str,
    _folder: &Path,
) -> Result<(), String> {
    Err(crate::i18n::media::HTML_ERR_NON_WINDOWS.get().into())
}
