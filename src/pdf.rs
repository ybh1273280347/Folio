//! PDF lifetime follows the visible workspace pane; the GPUI viewer owns page
//! virtualization, off-thread rendering, zoom anchoring and fit-to-viewport.
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_pdf::{PdfStyle, PdfView};
use std::{path::PathBuf, rc::Rc};

pub struct PdfPane {
    path: PathBuf,
    view: Option<Entity<PdfView>>,
    visible: bool,
}

impl PdfPane {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = Self::open_view(&path, window, cx);
        Self {
            path,
            view: Some(view),
            visible: true,
        }
    }

    fn open_view(path: &PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Entity<PdfView> {
        let style = Rc::new(|| PdfStyle {
            bg: rgb(0xf1f1ef).into(),
            border: rgb(0xe8e7e3).into(),
            placeholder_bg: rgb(0xffffff).into(),
            placeholder_fg: rgb(0x787774).into(),
            header_fg: rgb(0x37352f).into(),
            header_muted: rgb(0x787774).into(),
        });
        let view = cx.new(|cx| PdfView::new(path.clone(), style, Rc::new(|| 1.), window, cx));
        // Start at the same readable, viewport-filling scale as the original preview.
        view.update(cx, |view, cx| view.fit_width(cx));
        view
    }

    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.view = Some(Self::open_view(&self.path, window, cx));
        } else if let Some(view) = self.view.take() {
            view.update(cx, |view, cx| view.release(window, cx));
        }
    }

    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.view.take() {
            view.update(cx, |view, cx| view.release(window, cx));
        }
    }
}

impl Render for PdfPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(rgb(0xf1f1ef))
            .when_some(self.view.clone(), |area, view| area.child(view))
    }
}
