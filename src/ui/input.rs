//! Unified Folio input shells wrapping `gpui-component` input/textarea kernels
//! without leaking upstream default chrome.
use gpui_kit::{
    assets::IconName,
    component::input::{Input, InputState, Textarea, TextareaState},
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldSize {
    /// `28px` height, `6px` radius, `12px` text — for popovers and sidebars.
    Compact,
    /// `32px` height, `8px` radius, `12.5px` text — for modals and preferences.
    Regular,
}

fn focus_halo_shadow(palette: crate::theme::Palette) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), palette.accent_alpha(0.16).into())
            .blur_radius(px(0.))
            .spread_radius(px(2.)),
    ]
}

/// Folio-styled single-line text field wrapping a borderless `Input`.
#[derive(IntoElement)]
pub struct TextField {
    state: Entity<InputState>,
    size: FieldSize,
    surface_bg: bool,
    mask_toggle: bool,
    text_size: Option<Pixels>,
    font_family: Option<SharedString>,
}

impl TextField {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            size: FieldSize::Regular,
            surface_bg: false,
            mask_toggle: false,
            text_size: None,
            font_family: None,
        }
    }

    pub fn compact(mut self) -> Self {
        self.size = FieldSize::Compact;
        self
    }

    pub fn surface_bg(mut self) -> Self {
        self.surface_bg = true;
        self
    }

    pub fn mask_toggle(mut self) -> Self {
        self.mask_toggle = true;
        self
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = Some(size);
        self
    }

    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = crate::theme::palette();
        let focused = self.state.focus_handle(cx).is_focused(window);
        let (height, pad_x, radius, default_text_size) = match self.size {
            FieldSize::Compact => (px(28.), px(8.), px(6.), px(12.)),
            FieldSize::Regular => (px(32.), px(10.), px(8.), px(12.5)),
        };
        let bg = if self.surface_bg {
            gpui_kit::rgb(palette.surface)
        } else {
            gpui_kit::rgb(palette.background)
        };
        let border = if focused {
            gpui_kit::rgb(palette.accent)
        } else {
            palette.card_border()
        };
        let text_size = self.text_size.unwrap_or(default_text_size);
        let mut input = Input::new(&self.state)
            .appearance(false)
            .bordered(false)
            .flex_1()
            .min_w_0()
            .px_0()
            .py_0()
            .text_size(text_size)
            .text_color(gpui_kit::rgb(palette.text));
        if self.mask_toggle {
            input = input.mask_toggle();
        }
        if let Some(family) = self.font_family {
            input = input.font_family(family);
        }

        div()
            .w_full()
            .h(height)
            .px(pad_x)
            .rounded(radius)
            .bg(bg)
            .border_1()
            .border_color(border)
            .when(focused, |el| el.shadow(focus_halo_shadow(palette)))
            .flex()
            .items_center()
            .child(input)
    }
}

/// Folio-styled search input with leading Lucide search icon and automatic clear button.
#[derive(IntoElement)]
pub struct SearchField {
    id: ElementId,
    state: Entity<InputState>,
    size: FieldSize,
    surface_bg: bool,
    text_size: Option<Pixels>,
    font_family: Option<SharedString>,
    on_clear: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl SearchField {
    pub fn new(id: impl Into<ElementId>, state: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            size: FieldSize::Compact,
            surface_bg: false,
            text_size: None,
            font_family: None,
            on_clear: None,
        }
    }

    pub fn regular(mut self) -> Self {
        self.size = FieldSize::Regular;
        self
    }

    pub fn surface_bg(mut self) -> Self {
        self.surface_bg = true;
        self
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = Some(size);
        self
    }

    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    pub fn on_clear(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_clear = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SearchField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = crate::theme::palette();
        let focused = self.state.focus_handle(cx).is_focused(window);
        let has_query = !self.state.read(cx).value().is_empty();
        let (height, pad_x, radius, default_text_size) = match self.size {
            FieldSize::Compact => (px(28.), px(8.), px(6.), px(12.)),
            FieldSize::Regular => (px(32.), px(10.), px(8.), px(12.5)),
        };
        let bg = if self.surface_bg {
            gpui_kit::rgb(palette.surface)
        } else {
            gpui_kit::rgb(palette.background)
        };
        let border = if focused {
            gpui_kit::rgb(palette.accent)
        } else {
            palette.card_border()
        };
        let text_size = self.text_size.unwrap_or(default_text_size);
        let mut input = Input::new(&self.state)
            .appearance(false)
            .bordered(false)
            .flex_1()
            .min_w_0()
            .px_0()
            .py_0()
            .text_size(text_size)
            .text_color(gpui_kit::rgb(palette.text));
        if let Some(family) = self.font_family {
            input = input.font_family(family);
        }
        let clear_state = self.state.clone();
        let on_clear = self.on_clear;
        let clear_id = ElementId::NamedChild(self.id.into(), "clear".into());

        div()
            .w_full()
            .h(height)
            .px(pad_x)
            .rounded(radius)
            .bg(bg)
            .border_1()
            .border_color(border)
            .when(focused, |el| el.shadow(focus_halo_shadow(palette)))
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                super::icon(IconName::Search)
                    .size(px(13.))
                    .flex_shrink_0()
                    .text_color(gpui_kit::rgb(palette.muted)),
            )
            .child(input)
            .when(has_query, |bar| {
                bar.child(
                    div()
                        .id(clear_id)
                        .size(px(18.))
                        .rounded_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .hover(move |s| {
                            s.bg(gpui_kit::rgb(palette.hover))
                                .text_color(gpui_kit::rgb(palette.text))
                        })
                        .on_click(move |_, window, cx| {
                            clear_state.update(cx, |input, cx| input.set_value("", window, cx));
                            if let Some(cb) = &on_clear {
                                cb(window, cx);
                            }
                        })
                        .child(super::icon(IconName::Close).size(px(11.))),
                )
            })
    }
}

/// Folio-styled multi-line textarea shell wrapping a borderless `Textarea`.
#[derive(IntoElement)]
pub struct TextAreaField {
    state: Entity<TextareaState>,
    height: Option<Pixels>,
    surface_bg: bool,
    text_size: Pixels,
}

impl TextAreaField {
    pub fn new(state: &Entity<TextareaState>) -> Self {
        Self {
            state: state.clone(),
            height: None,
            surface_bg: false,
            text_size: px(12.5),
        }
    }

    pub fn h(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    pub fn surface_bg(mut self) -> Self {
        self.surface_bg = true;
        self
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = size;
        self
    }
}

impl RenderOnce for TextAreaField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = crate::theme::palette();
        let focused = self.state.focus_handle(cx).is_focused(window);
        let bg = if self.surface_bg {
            gpui_kit::rgb(palette.surface)
        } else {
            gpui_kit::rgb(palette.background)
        };
        let border = if focused {
            gpui_kit::rgb(palette.accent)
        } else {
            palette.card_border()
        };

        div()
            .w_full()
            .when_some(self.height, |d, h| d.h(h))
            .px_2p5()
            .py_1p5()
            .rounded(px(8.))
            .bg(bg)
            .border_1()
            .border_color(border)
            .when(focused, |el| el.shadow(focus_halo_shadow(palette)))
            .child(
                Textarea::new(&self.state)
                    .appearance(false)
                    .bordered(false)
                    .size_full()
                    .px_0()
                    .py_0()
                    .text_size(self.text_size)
                    .text_color(gpui_kit::rgb(palette.text)),
            )
    }
}
