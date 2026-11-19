//! In-app Chinese/English selection translation popover with Google & Baidu providers.
use crate::{
    model::TranslateSettings,
    theme::color as rgb,
};
use gpui::MouseButton;
use gpui_kit::{
    assets::IconName,
    base::StyledExt,
    component::tooltip::Tooltip,
    prelude::FluentBuilder,
    *,
};
use std::{
    ffi::c_void,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::form_urlencoded;

const POPOVER_WIDTH: f32 = 368.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslateDictEntry {
    pub pos: String,
    pub terms: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TranslateResult {
    pub provider: &'static str,
    pub translated: String,
    pub detected_lang: String,
    pub dict: Vec<TranslateDictEntry>,
}

pub struct TranslatePopover {
    open: bool,
    provider: &'static str,
    source_text: String,
    translated_text: String,
    dict: Vec<TranslateDictEntry>,
    detected_lang: String,
    target_lang: &'static str,
    loading: bool,
    error: Option<String>,
    copied: bool,
    anchor: Option<Point<Pixels>>,
    request_id: u64,
    task: Task<()>,
    copy_task: Task<()>,
}

impl Default for TranslatePopover {
    fn default() -> Self {
        Self {
            open: false,
            provider: "",
            source_text: String::new(),
            translated_text: String::new(),
            dict: Vec::new(),
            detected_lang: String::new(),
            target_lang: "zh",
            loading: false,
            error: None,
            copied: false,
            anchor: None,
            request_id: 0,
            task: Task::ready(()),
            copy_task: Task::ready(()),
        }
    }
}

impl TranslatePopover {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn dismiss(&mut self) {
        self.open = false;
        self.loading = false;
        self.error = None;
        self.copied = false;
        self.request_id = self.request_id.wrapping_add(1);
        self.task = Task::ready(());
    }

    pub fn open_and_translate<T: 'static>(
        owner: &mut T,
        text: String,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<T>,
        get_mut: fn(&mut T) -> &mut TranslatePopover,
    ) {
        let trimmed = text.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        let target_lang = default_target_lang(&trimmed);
        {
            let pop = get_mut(owner);
            pop.open = true;
            pop.provider = "";
            pop.source_text = trimmed;
            pop.translated_text.clear();
            pop.dict.clear();
            pop.detected_lang.clear();
            pop.target_lang = target_lang;
            pop.error = None;
            pop.copied = false;
            pop.anchor = anchor;
        }
        Self::run_request(owner, cx, get_mut);
    }

    fn switch_target<T: 'static>(
        owner: &mut T,
        target: &'static str,
        cx: &mut Context<T>,
        get_mut: fn(&mut T) -> &mut TranslatePopover,
    ) {
        {
            let pop = get_mut(owner);
            if pop.target_lang == target && !pop.loading && pop.error.is_none() {
                return;
            }
            pop.target_lang = target;
            pop.copied = false;
        }
        Self::run_request(owner, cx, get_mut);
    }

    fn run_request<T: 'static>(
        owner: &mut T,
        cx: &mut Context<T>,
        get_mut: fn(&mut T) -> &mut TranslatePopover,
    ) {
        let (req_id, source, target) = {
            let pop = get_mut(owner);
            pop.request_id = pop.request_id.wrapping_add(1);
            pop.loading = true;
            pop.error = None;
            (pop.request_id, pop.source_text.clone(), pop.target_lang)
        };
        let settings = crate::storage::load_settings()
            .map(|s| s.translate)
            .unwrap_or_default();
        cx.notify();

        let task_bg = cx
            .background_executor()
            .spawn(async move { translate_google_then_baidu(&source, target, &settings) });
        let task = cx.spawn(async move |this, cx| {
            let outcome = task_bg.await;
            let _ = this.update(cx, |owner, cx| {
                let pop = get_mut(owner);
                if !pop.open || pop.request_id != req_id {
                    return;
                }
                pop.loading = false;
                match outcome {
                    Ok(res) => {
                        pop.provider = res.provider;
                        pop.translated_text = res.translated;
                        pop.detected_lang = res.detected_lang;
                        pop.dict = res.dict;
                        pop.error = None;
                    }
                    Err(err) => {
                        pop.provider = "";
                        pop.error = Some(err);
                    }
                }
                cx.notify();
            });
        });
        get_mut(owner).task = task;
    }

    fn copy_result<T: 'static>(
        owner: &mut T,
        cx: &mut Context<T>,
        get_mut: fn(&mut T) -> &mut TranslatePopover,
    ) {
        let text = get_mut(owner).translated_text.clone();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let pop = get_mut(owner);
        pop.copied = true;
        cx.notify();

        pop.copy_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1400))
                .await;
            let _ = this.update(cx, |owner, cx| {
                get_mut(owner).copied = false;
                cx.notify();
            });
        });
    }

    pub fn render<T: 'static>(
        &self,
        id_prefix: &'static str,
        font_family: Option<&str>,
        font_size: Option<u8>,
        pane_size: Option<Size<Pixels>>,
        cx: &mut Context<T>,
        get_mut: fn(&mut T) -> &mut TranslatePopover,
    ) -> Option<AnyElement> {
        if !self.open {
            return None;
        }

        let explicit_font = font_family.map(crate::ui::sans_font);
        let base_size = (font_size.unwrap_or(14) as f32).clamp(12.0, 18.0);
        let title_size = (base_size - 1.5).max(11.0);
        let small_size = (base_size - 2.0).max(11.0);

        let lang_part = if !self.detected_lang.is_empty() {
            format!(
                "{} → {}",
                lang_label(&self.detected_lang),
                lang_label(self.target_lang)
            )
        } else {
            format!("自动 → {}", lang_label(self.target_lang))
        };
        let lang_badge = if self.provider.is_empty() {
            lang_part
        } else {
            format!("{} · {lang_part}", self.provider)
        };

        let target_pills = [("zh", "中"), ("en", "EN")]
            .into_iter()
            .map(|(code, label)| {
                let active = self.target_lang == code;
                div()
                    .id(SharedString::from(format!("{id_prefix}-tl-{code}")))
                    .px_1p5()
                    .h(px(20.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(small_size))
                    .when(active, |s| {
                        s.bg(rgb(0x52769a))
                            .text_color(crate::theme::on_accent())
                            .font_medium()
                    })
                    .when(!active, |s| {
                        s.bg(rgb(0xf2f3f5))
                            .text_color(rgb(0x5b5b58))
                            .hover(|h| h.bg(rgb(0xe5e6eb)))
                            .active(|a| a.bg(rgb(crate::ui::PRESSED_BG)))
                    })
                    .child(label)
                    .on_click(cx.listener(move |owner, _, _, cx| {
                        Self::switch_target(owner, code, cx, get_mut);
                    }))
            });

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(title_size))
                            .font_semibold()
                            .text_color(rgb(0x37352f))
                            .flex_shrink_0()
                            .child("翻译"),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(rgb(0xf7f7f5))
                            .border_1()
                            .border_color(rgb(0xe8e7e3))
                            .text_size(px(small_size))
                            .text_color(rgb(0x787774))
                            .truncate()
                            .child(lang_badge),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .children(target_pills)
                    .child(div().w(px(1.)).h(px(14.)).mx_0p5().bg(rgb(0xe8e7e3)))
                    .child(
                        div()
                            .id(SharedString::from(format!("{id_prefix}-copy")))
                            .w(px(24.))
                            .h(px(24.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(if self.copied {
                                rgb(0x1677ff)
                            } else {
                                rgb(0x5b5b58)
                            })
                            .hover(|s| s.bg(rgb(0xf2f3f5)))
                            .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                            .tooltip({
                                let copied = self.copied;
                                move |window, cx| {
                                    Tooltip::new(if copied { "已复制译文" } else { "复制译文" })
                                        .build(window, cx)
                                }
                            })
                            .child(
                                crate::ui::icon(if self.copied {
                                    IconName::Check
                                } else {
                                    IconName::Copy
                                })
                                .w(px(13.))
                                .h(px(13.)),
                            )
                            .on_click(cx.listener(move |owner, _, _, cx| {
                                Self::copy_result(owner, cx, get_mut);
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{id_prefix}-close")))
                            .w(px(24.))
                            .h(px(24.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(rgb(0x5b5b58))
                            .hover(|s| s.bg(rgb(0xf2f3f5)))
                            .active(|s| s.bg(rgb(crate::ui::PRESSED_BG)))
                            .tooltip(|window, cx| Tooltip::new("关闭 (Esc)").build(window, cx))
                            .child(crate::ui::icon(IconName::Close).w(px(13.)).h(px(13.)))
                            .on_click(cx.listener(move |owner, _, _, cx| {
                                get_mut(owner).dismiss();
                                cx.notify();
                            })),
                    ),
            );

        let source_box = div()
            .id(SharedString::from(format!("{id_prefix}-source")))
            .max_h(px(68.))
            .overflow_y_scroll()
            .px_2p5()
            .py_1p5()
            .rounded(px(6.))
            .bg(rgb(0xf7f7f5))
            .border_1()
            .border_color(rgb(0xefeeea))
            .text_size(px(small_size))
            .line_height(relative(1.45))
            .text_color(rgb(0x787774))
            .child(self.source_text.clone());

        let body = div()
            .id(SharedString::from(format!("{id_prefix}-body")))
            .max_h(px(260.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .when(self.loading, |box_| {
                box_.child(
                    div()
                        .py_2()
                        .text_size(px(base_size))
                        .text_color(rgb(0x787774))
                        .child("正在翻译…"),
                )
            })
            .when(!self.loading && self.error.is_some(), |box_| {
                let err = self.error.clone().unwrap_or_default();
                box_.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .py_1()
                        .child(
                            div()
                                .text_size(px(small_size))
                                .line_height(relative(1.45))
                                .text_color(rgb(0xd93025))
                                .child(err),
                        )
                        .child(
                            div().flex().child(
                                crate::ui::button_secondary(
                                    SharedString::from(format!("{id_prefix}-retry")),
                                    "重试",
                                )
                                .on_click(cx.listener(move |owner, _, _, cx| {
                                    Self::run_request(owner, cx, get_mut);
                                })),
                            ),
                        ),
                )
            })
            .when(!self.loading && self.error.is_none(), |box_| {
                box_.child(
                    div()
                        .text_size(px(base_size))
                        .line_height(relative(1.55))
                        .text_color(rgb(0x1f2328))
                        .child(self.translated_text.clone()),
                )
                .when(!self.dict.is_empty(), |box_| {
                    box_.child(div().h(px(1.)).bg(rgb(0xf1f0eb))).child(
                        div().flex().flex_col().gap_1().children(self.dict.iter().map(|entry| {
                            div()
                                .flex()
                                .items_start()
                                .gap_1p5()
                                .text_size(px(small_size))
                                .line_height(relative(1.45))
                                .when(!entry.pos.is_empty(), |row| {
                                    row.child(
                                        div()
                                            .px_1()
                                            .rounded(px(3.))
                                            .bg(rgb(0xf2f3f5))
                                            .text_color(rgb(0x5b5b58))
                                            .flex_shrink_0()
                                            .child(entry.pos.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(rgb(0x4b4a45))
                                        .child(entry.terms.join("；")),
                                )
                        })),
                    )
                })
            });

        let mut card = div()
            .id(SharedString::from(format!("{id_prefix}-card")))
            .absolute()
            .w(px(POPOVER_WIDTH))
            .max_h(px(420.))
            .p_3()
            .rounded(px(10.))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xe2e4e8))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(explicit_font, |s, f| s.font_family(f))
            .text_size(px(base_size))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(source_box)
            .child(body);

        if let (Some(anchor), Some(size)) = (self.anchor, pane_size) {
            let max_x = (size.width - px(POPOVER_WIDTH + 16.0)).max(px(16.0));
            let max_y = (size.height - px(280.0)).max(px(16.0));
            let left = anchor.x.clamp(px(16.0), max_x);
            let top = anchor.y.clamp(px(16.0), max_y);
            card = card.left(left).top(top);
        } else {
            card = card.top(px(44.)).right(px(20.));
        }

        Some(deferred(card).with_priority(2).into_any_element())
    }
}

fn default_target_lang(text: &str) -> &'static str {
    let mut cjk = 0usize;
    let mut non_cjk = 0usize;
    for ch in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch)
            || ('\u{3400}'..='\u{4DBF}').contains(&ch)
        {
            cjk += 1;
        } else if ch.is_ascii_alphabetic() {
            non_cjk += 1;
        }
    }
    if cjk > 0 && cjk * 2 >= non_cjk {
        "en"
    } else {
        "zh"
    }
}

fn lang_label(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "zh-cn" | "zh" | "zh-hans" => "中文".to_owned(),
        "zh-tw" | "zh-hant" | "cht" => "繁中".to_owned(),
        "en" => "英语".to_owned(),
        "ja" | "jp" => "日语".to_owned(),
        "ko" | "kor" => "韩语".to_owned(),
        "fr" | "fra" => "法语".to_owned(),
        "de" => "德语".to_owned(),
        "es" | "spa" => "西语".to_owned(),
        "ru" => "俄语".to_owned(),
        "la" => "拉丁语".to_owned(),
        other => other.to_uppercase(),
    }
}

fn translate_google_then_baidu(
    text: &str,
    target_lang: &str,
    settings: &TranslateSettings,
) -> Result<TranslateResult, String> {
    let baidu_ready =
        !settings.baidu_app_id.trim().is_empty() && !settings.baidu_api_key.trim().is_empty();
    let google_timeout = if baidu_ready { 2_500 } else { 4_500 };
    match google_translate(text, target_lang, google_timeout) {
        Ok(res) => Ok(res),
        Err(google_err) => {
            if !baidu_ready {
                return Err(google_err);
            }
            baidu_translate(
                text,
                target_lang,
                settings.baidu_app_id.trim(),
                settings.baidu_api_key.trim(),
            )
            .map_err(|baidu_err| format!("{google_err}；百度备用也失败：{baidu_err}"))
        }
    }
}

fn google_translate(
    text: &str,
    target_lang: &str,
    connect_timeout_ms: i32,
) -> Result<TranslateResult, String> {
    let tl = if target_lang == "zh" { "zh-CN" } else { target_lang };
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("client", "gtx")
        .append_pair("sl", "auto")
        .append_pair("tl", tl)
        .append_pair("dt", "t")
        .append_pair("dt", "bd")
        .append_pair("q", text)
        .finish();

    let raw = post_form_https(
        "translate.googleapis.com",
        "/translate_a/single",
        &body,
        connect_timeout_ms,
        true,
        "Google 翻译",
    )?;
    parse_google_response(&raw)
}

fn baidu_translate(
    text: &str,
    target_lang: &str,
    app_id: &str,
    api_key: &str,
) -> Result<TranslateResult, String> {
    if app_id.is_empty() || api_key.is_empty() {
        return Err("请在 设置 → API 服务 中填写百度翻译 APP ID 与密钥".to_owned());
    }
    let to = if target_lang == "en" { "en" } else { "zh" };
    let salt = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(1_435_660_288)
        .to_string();
    let sign = md5_hex(format!("{app_id}{text}{salt}{api_key}").as_bytes());
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("q", text)
        .append_pair("from", "auto")
        .append_pair("to", to)
        .append_pair("appid", app_id)
        .append_pair("salt", &salt)
        .append_pair("sign", &sign)
        .finish();

    let raw = post_form_https(
        "fanyi-api.baidu.com",
        "/api/trans/vip/translate",
        &body,
        4_500,
        false,
        "百度翻译",
    )?;
    parse_baidu_response(&raw)
}

fn parse_google_response(raw: &str) -> Result<TranslateResult, String> {
    let json: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 Google 翻译响应失败：{e}"))?;
    let Some(arr) = json.as_array() else {
        return Err("Google 翻译返回格式异常".to_owned());
    };

    let mut translated = String::new();
    if let Some(sentences) = arr.first().and_then(|v| v.as_array()) {
        for seg in sentences {
            if let Some(part) = seg.as_array().and_then(|s| s.first()).and_then(|v| v.as_str()) {
                translated.push_str(part);
            }
        }
    }
    if translated.trim().is_empty() {
        return Err("未获取到有效译文".to_owned());
    }

    let detected_lang = arr
        .get(2)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();

    let mut dict = Vec::new();
    if let Some(entries) = arr.get(1).and_then(|v| v.as_array()) {
        for entry in entries.iter().take(4) {
            let Some(parts) = entry.as_array() else {
                continue;
            };
            let pos = parts
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_owned();
            let terms: Vec<String> = parts
                .get(1)
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .take(5)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !terms.is_empty() {
                dict.push(TranslateDictEntry { pos, terms });
            }
        }
    }

    Ok(TranslateResult {
        provider: "Google",
        translated,
        detected_lang,
        dict,
    })
}

fn parse_baidu_response(raw: &str) -> Result<TranslateResult, String> {
    let json: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("解析百度翻译响应失败：{e}"))?;

    if let Some(code) = json.get("error_code").and_then(|v| {
        v.as_str()
            .map(str::to_owned)
            .or_else(|| v.as_i64().map(|n| n.to_string()))
    }) {
        let msg = json
            .get("error_msg")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let detail = match code.as_str() {
            "52001" => "请求超时",
            "52002" => "百度服务系统错误",
            "52003" => "未授权用户，请检查 APP ID 是否正确及是否已开通通用文本翻译",
            "54001" => "签名错误，请检查 APP ID 与密钥是否匹配",
            "54003" => "访问频率受限，请稍后再试",
            "54004" => "百度翻译账户余额不足",
            "54005" => "请求文本过长",
            "58000" => "客户端 IP 非法（如设置了 IP 白名单）",
            _ => msg,
        };
        return Err(format!("百度翻译错误 ({code})：{detail}"));
    }

    let mut lines = Vec::new();
    if let Some(items) = json.get("trans_result").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(dst) = item.get("dst").and_then(|v| v.as_str()) {
                lines.push(dst);
            }
        }
    }
    let translated = lines.join("\n");
    if translated.trim().is_empty() {
        return Err("百度翻译未返回有效译文".to_owned());
    }
    let detected_lang = json
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();

    Ok(TranslateResult {
        provider: "百度",
        translated,
        detected_lang,
        dict: Vec::new(),
    })
}

#[cfg(windows)]
fn env_proxy() -> Option<String> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                let stripped = trimmed
                    .strip_prefix("http://")
                    .or_else(|| trimmed.strip_prefix("https://"))
                    .unwrap_or(trimmed);
                return Some(stripped.trim_end_matches('/').to_owned());
            }
        }
    }
    None
}

#[cfg(windows)]
fn post_form_https(
    host: &str,
    path: &str,
    form_body: &str,
    connect_timeout_ms: i32,
    use_env_proxy: bool,
    provider_label: &str,
) -> Result<String, String> {
    use windows::{
        Win32::Networking::WinHttp::{
            WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
            WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
            WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
            WINHTTP_QUERY_STATUS_CODE,
        },
        core::{HSTRING, PCWSTR},
    };

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    let ua = HSTRING::from("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Folio/1.0");
    let proxy = if use_env_proxy { env_proxy() } else { None };
    let proxy_h = proxy.as_deref().map(HSTRING::from);
    let session = unsafe {
        if let Some(ref p) = proxy_h {
            WinHttpOpen(
                &ua,
                WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                p,
                PCWSTR::null(),
                0,
            )
        } else {
            WinHttpOpen(
                &ua,
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        }
    };
    if session.is_null() {
        return Err(format!("初始化 {provider_label} WinHTTP 会话失败"));
    }
    let session = Handle(session);

    unsafe {
        let _ = WinHttpSetTimeouts(
            session.0,
            connect_timeout_ms,
            connect_timeout_ms,
            5_000,
            8_000,
        );
    }

    let host_h = HSTRING::from(host);
    let connect = unsafe { WinHttpConnect(session.0, &host_h, 443, 0) };
    if connect.is_null() {
        return Err(format!("连接 {provider_label} 失败"));
    }
    let connect = Handle(connect);

    let verb = HSTRING::from("POST");
    let path_h = HSTRING::from(path);
    let request = unsafe {
        WinHttpOpenRequest(
            connect.0,
            &verb,
            &path_h,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    };
    if request.is_null() {
        return Err(format!("创建 {provider_label} 请求失败"));
    }
    let request = Handle(request);

    let headers = HSTRING::from("Content-Type: application/x-www-form-urlencoded; charset=utf-8");
    let body_bytes = form_body.as_bytes();
    let sent = unsafe {
        WinHttpSendRequest(
            request.0,
            Some(&*headers),
            Some(body_bytes.as_ptr() as *const c_void),
            body_bytes.len() as u32,
            body_bytes.len() as u32,
            0,
        )
    };
    if sent.is_err() {
        return Err(format!("请求 {provider_label} 失败（网络超时或未通）"));
    }

    if unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }.is_err() {
        return Err(format!("接收 {provider_label} 响应超时"));
    }

    let mut status: u32 = 0;
    let mut status_len: u32 = std::mem::size_of::<u32>() as u32;
    let _ = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast::<c_void>()),
            &mut status_len,
            std::ptr::null_mut(),
        )
    };
    if status == 429 {
        return Err(format!("{provider_label} 请求过于频繁 (HTTP 429)"));
    }
    if !(200..300).contains(&status) {
        return Err(format!("{provider_label} 返回状态码 HTTP {status}"));
    }

    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let mut read = 0u32;
        if unsafe {
            WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast::<c_void>(),
                buf.len() as u32,
                &mut read,
            )
        }
        .is_err()
        {
            return Err(format!("读取 {provider_label} 响应出错"));
        }
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > 1_048_576 {
            return Err(format!("{provider_label} 响应体积过大"));
        }
    }

    String::from_utf8(out).map_err(|_| format!("{provider_label} 响应不是有效的 UTF-8 文本"))
}

#[cfg(not(windows))]
fn post_form_https(
    host: &str,
    path: &str,
    form_body: &str,
    _connect_timeout_ms: i32,
    _use_env_proxy: bool,
    provider_label: &str,
) -> Result<String, String> {
    let url = format!("https://{host}{path}");
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "10",
            "-H",
            "Content-Type: application/x-www-form-urlencoded; charset=utf-8",
            "--data",
            form_body,
            &url,
        ])
        .output()
        .map_err(|e| format!("调用 curl 请求 {provider_label} 失败：{e}"))?;
    if !output.status.success() {
        return Err(format!("请求 {provider_label} 失败"));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{provider_label} 响应不是有效的 UTF-8 文本"))
}

fn md5_hex(input: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut data = input.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    for chunk in data.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let off = i * 4;
            *w = u32::from_le_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for (i, (&s, &k)) in S.iter().zip(K.iter()).enumerate() {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(k).wrapping_add(m[g]).rotate_left(s));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut hex = String::with_capacity(32);
    for word in [a0, b0, c0, d0] {
        for byte in word.to_le_bytes() {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_sentences_lang_and_dict() {
        let sample = r#"[
            [["你好，", "Hello, "], ["世界", "world"]],
            [["noun", ["世界", "地球"]]],
            "en"
        ]"#;
        let parsed = parse_google_response(sample).expect("valid google payload");
        assert_eq!(parsed.provider, "Google");
        assert_eq!(parsed.translated, "你好，世界");
        assert_eq!(parsed.detected_lang, "en");
        assert_eq!(parsed.dict.len(), 1);
        assert_eq!(parsed.dict[0].pos, "noun");
        assert_eq!(parsed.dict[0].terms, vec!["世界", "地球"]);
    }

    #[test]
    fn parses_baidu_response_and_error() {
        let ok = r#"{"from":"en","to":"zh","trans_result":[{"src":"hello","dst":"你好"}]}"#;
        let parsed = parse_baidu_response(ok).expect("valid baidu payload");
        assert_eq!(parsed.provider, "百度");
        assert_eq!(parsed.translated, "你好");
        assert_eq!(parsed.detected_lang, "en");

        let err = r#"{"error_code":"54001","error_msg":"Invalid Sign"}"#;
        assert!(parse_baidu_response(err).unwrap_err().contains("54001"));
    }

    #[test]
    fn md5_matches_rfc1321_and_baidu_example() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"2015063000000001apple143566028812345678"),
            "f89f9594663708c1605f3d736d01d2d4"
        );
    }

    #[test]
    fn auto_picks_zh_or_en_only() {
        assert_eq!(default_target_lang("Hello world"), "zh");
        assert_eq!(default_target_lang("你好世界"), "en");
    }
}
