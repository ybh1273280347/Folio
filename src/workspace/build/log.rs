//! Read-only foreground build output with selection and clipboard copy.
use crate::theme::color as rgb;
use gpui_kit::{assets::IconName, component::text::TextView, *};

pub(super) fn panel(
    logs: String,
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let copied = logs.clone();
    div()
        .id("build-log")
        .size_full()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(rgb(0xffffff))
        .border_l_1()
        .border_color(rgb(0xe8e7e3))
        .overflow_hidden()
        .font_family("Consolas")
        .text_xs()
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .child(crate::i18n::media::BUILD_LOG_TITLE.get())
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::ui::button_secondary("copy-build-log", crate::i18n::media::BUILD_LOG_COPY_ALL.get()).on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copied.clone(),
                                    ));
                                },
                            ),
                        )
                        .child(crate::ui::control("close-logs", IconName::Close).on_click(close)),
                ),
        )
        .child(
            div()
                .id("build-log-scroll")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .child(
                    TextView::markdown("build-log-text", format!("````text\n{logs}\n````"))
                        .selectable(true),
                ),
        )
        .into_any_element()
}
