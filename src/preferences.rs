//! Appearance preferences are persisted independently of project files and PDF typesetting.
use crate::{model::AppSettings, storage};
use gpui_kit::{
    base::StyledExt,
    component::{
        select::{SearchableVec, Select, SelectEvent, SelectState},
        slider::{Slider, SliderEvent, SliderState},
    },
    *,
};

pub struct SettingsChanged(pub AppSettings);
pub struct Preferences {
    settings: AppSettings,
    interface_font: Entity<SelectState<SearchableVec<String>>>,
    editor_font: Entity<SelectState<SearchableVec<String>>>,
    sizes: [Entity<SliderState>; 3],
    error: Option<String>,
}
impl EventEmitter<SettingsChanged> for Preferences {}

impl Preferences {
    pub fn new(settings: AppSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fonts = cx.text_system().all_font_names();
        fonts.sort();
        fonts.dedup();
        fonts.insert(0, "系统默认".into());
        let interface_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts.clone()), None, window, cx).searchable(true)
        });
        let editor_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true)
        });
        let sizes = std::array::from_fn(|index| {
            cx.new(|_| {
                SliderState::new()
                    .min(11.)
                    .max(if index == 0 { 20. } else { 26. })
                    .step(1.)
                    .default_value(
                        [
                            settings.interface_font_size,
                            settings.editor_font_size,
                            settings.markdown_font_size,
                        ][index] as f32,
                    )
            })
        });
        for (index, slider) in sizes.iter().enumerate() {
            cx.subscribe(slider, move |panel, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event else {
                    return;
                };
                let size = value.end().round() as u8;
                match index {
                    0 => panel.settings.interface_font_size = size,
                    1 => panel.settings.editor_font_size = size,
                    _ => panel.settings.markdown_font_size = size,
                }
                panel.changed(cx);
            })
            .detach();
        }
        for (state, selected) in [
            (&interface_font, &settings.interface_font),
            (&editor_font, &settings.editor_font),
        ] {
            state.update(cx, |state, cx| {
                state.set_selected_value(
                    &if selected.is_empty() {
                        "系统默认".into()
                    } else {
                        selected.clone()
                    },
                    window,
                    cx,
                )
            });
        }
        cx.subscribe(
            &interface_font,
            |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(Some(font)) = event else {
                    return;
                };
                panel.settings.interface_font = if font == "系统默认" {
                    String::new()
                } else {
                    font.clone()
                };
                panel.changed(cx);
            },
        )
        .detach();
        cx.subscribe(
            &editor_font,
            |panel, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(Some(font)) = event else {
                    return;
                };
                panel.settings.editor_font = if font == "系统默认" {
                    String::new()
                } else {
                    font.clone()
                };
                panel.changed(cx);
            },
        )
        .detach();
        Self {
            settings,
            interface_font,
            editor_font,
            sizes,
            error: None,
        }
    }
    pub fn sync(&mut self, settings: &AppSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        for (slider, size) in self.sizes.iter().zip([
            settings.interface_font_size,
            settings.editor_font_size,
            settings.markdown_font_size,
        ]) {
            slider.update(cx, |state, cx| state.set_value(size as f32, window, cx));
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.error = storage::save_settings(&self.settings)
            .err()
            .map(|error| error.to_string());
        cx.emit(SettingsChanged(self.settings.clone()));
        cx.notify();
    }
    fn size_row(
        &self,
        id: &'static str,
        label: &'static str,
        value: u8,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let index = match id {
            "ui-size" => 0,
            "editor-size" => 1,
            _ => 2,
        };
        let _ = cx;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label)
                    .child(format!("{value} px")),
            )
            .child(Slider::new(&self.sizes[index]))
            .into_any_element()
    }
}
fn control(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .border_1()
        .border_color(rgb(0xe8e7e3))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xefefed)))
        .child(label)
}
impl Render for Preferences {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap_5().text_color(rgb(0x37352f))
            .child(div().text_xl().font_semibold().child("设置"))
            .child(div().flex().flex_col().gap_2().child("界面字体").child(Select::new(&self.interface_font)))
            .child(div().flex().flex_col().gap_2().child("编辑器字体").child(Select::new(&self.editor_font)))
            .child(self.size_row("ui-size", "界面字号", self.settings.interface_font_size, cx))
            .child(self.size_row("editor-size", "编辑器字号", self.settings.editor_font_size, cx))
            .child(self.size_row("markdown-size", "Markdown 字号", self.settings.markdown_font_size, cx))
            .child(div().flex().items_center().justify_between().child("编辑器限制行宽")
                .child(control("limit-width", if self.settings.line_width_limited { "已开启" } else { "已关闭" }).on_click(cx.listener(|panel, _, _, cx| {
                    panel.settings.line_width_limited = !panel.settings.line_width_limited; panel.changed(cx);
                }))))
            .child(div().p_4().rounded(px(5.)).bg(rgb(0xf7f6f5)).flex().flex_col().gap_3()
                .font_family(if self.settings.interface_font.is_empty() { SharedString::from("Segoe UI") } else { self.settings.interface_font.clone().into() })
                .child("Aa 中文字体")
                .child(div().font_family(if self.settings.editor_font.is_empty() { SharedString::from("Consolas") } else { self.settings.editor_font.clone().into() })
                    .text_size(px(self.settings.editor_font_size as f32)).child("\\section{Introduction}")))
            .child(div().text_xs().text_color(rgb(0x787774)).child("Ctrl+O 打开项目    Ctrl+S 保存    Ctrl+Enter 编译\nCtrl+P 查找文件    Ctrl+滚轮 缩放"))
            .child(control("reset-settings", "恢复默认").on_click(cx.listener(|panel, _, window, cx| {
                panel.settings = AppSettings::default();
                for slider in &panel.sizes { slider.update(cx, |state, cx| state.set_value(14., window, cx)); }
                panel.interface_font.update(cx, |state, cx| state.set_selected_value(&"系统默认".into(), window, cx));
                panel.editor_font.update(cx, |state, cx| state.set_selected_value(&"系统默认".into(), window, cx));
                panel.changed(cx);
            })))
            .children(self.error.iter().map(|error| div().text_color(rgb(0xa16b60)).child(error.clone())))
    }
}
