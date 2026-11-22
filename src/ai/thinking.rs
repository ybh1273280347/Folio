//! The thinking scroll box keeps native text shaping off the UI thread for long streams.

use super::ProcessStyle;
use gpui_kit::{base::ElementExt, *};
use std::sync::Arc;

const INLINE_LAYOUT_BYTES: usize = 8 * 1024;

pub(super) fn render_thinking_detail(
    id: SharedString,
    text: String,
    streaming: bool,
    ui_font: &SharedString,
    ps: ProcessStyle,
) -> AnyElement {
    ThinkingScrollBox {
        id,
        source: text.into(),
        streaming,
        ui_font: ui_font.clone(),
        ps,
    }
    .into_any_element()
}

#[derive(IntoElement)]
struct ThinkingScrollBox {
    id: SharedString,
    source: SharedString,
    streaming: bool,
    ui_font: SharedString,
    ps: ProcessStyle,
}

impl RenderOnce for ThinkingScrollBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let content = window.use_keyed_state((self.id.clone(), 44usize), cx, |_, _| ThinkingText {
            source: self.source.clone(),
            streaming: self.streaming,
            scroll: ScrollHandle::default(),
            geometry: None,
            prepared: None,
            worker: None,
        });
        let current = content.read(cx);
        let scroll = current.scroll.clone();
        if current.source != self.source || current.streaming != self.streaming {
            content.update(cx, |content, cx| {
                if content
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| !self.source.starts_with(prepared.source.as_str()))
                {
                    content.prepared = None;
                }
                content.source = self.source;
                content.streaming = self.streaming;
                cx.notify();
            });
        }
        if self.streaming {
            scroll.scroll_to_bottom();
        }
        div().on_scroll(|_, _, cx| cx.stop_propagation()).child(
            div()
                .id(self.id)
                .track_scroll(&scroll)
                .px_1p5()
                .py_1()
                .max_h(px(160.))
                .overflow_y_scroll()
                .font_family(self.ui_font)
                .text_size(self.ps.detail_size)
                .line_height(relative(1.55))
                .text_color(self.ps.fg_secondary)
                .child(content),
        )
    }
}

#[derive(Clone, PartialEq)]
struct TextGeometry {
    width: Pixels,
    style: TextStyle,
    font_size: Pixels,
    line_height: Pixels,
}

struct PreparedThinkingText {
    source: SharedString,
    geometry: TextGeometry,
    lines: Vec<WrappedLine>,
    height: Pixels,
}

struct ThinkingText {
    source: SharedString,
    streaming: bool,
    scroll: ScrollHandle,
    geometry: Option<TextGeometry>,
    prepared: Option<Arc<PreparedThinkingText>>,
    worker: Option<Task<()>>,
}

impl ThinkingText {
    fn prepare_latest(&mut self, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry.as_ref() else {
            return;
        };
        if self.source.len() <= INLINE_LAYOUT_BYTES
            || self.worker.is_some()
            || self.prepared.as_ref().is_some_and(|prepared| {
                prepared.source == self.source && prepared.geometry == *geometry
            })
        {
            return;
        }
        let source = self.source.clone();
        let geometry = geometry.clone();
        let text_system = cx.text_system().clone();
        let prepare = cx.background_spawn(async move {
            let run = geometry.style.to_run(source.len());
            let lines: Vec<_> = WindowTextSystem::new(text_system)
                .shape_text(
                    source.clone(),
                    geometry.font_size,
                    &[run],
                    Some(geometry.width),
                    None,
                )
                .expect("thinking text has one style run covering all bytes")
                .into_iter()
                .collect();
            let height = lines
                .iter()
                .map(|line| line.size(geometry.line_height).height)
                .sum();
            Arc::new(PreparedThinkingText {
                source,
                geometry,
                lines,
                height,
            })
        });
        self.worker = Some(cx.spawn(async move |text, cx| {
            let prepared = prepare.await;
            let _ = text.update(cx, |text, cx| {
                text.worker = None;
                // A completed prefix remains useful during a fast stream. Replacements
                // and geometry changes invalidate results. The next pass uses the latest input.
                if text.geometry.as_ref() == Some(&prepared.geometry)
                    && text.source.starts_with(prepared.source.as_str())
                {
                    text.prepared = Some(prepared);
                    if text.streaming {
                        text.scroll.scroll_to_bottom();
                    }
                    cx.notify();
                }
                text.prepare_latest(cx);
            });
        }));
    }
}

impl Render for ThinkingText {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div().relative().w_full();
        if self.source.len() <= INLINE_LAYOUT_BYTES {
            return body.child(self.source.clone());
        }

        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let entity = cx.entity();
        let body = body.on_prepaint(move |bounds, _, cx| {
            let geometry = TextGeometry {
                width: bounds.size.width,
                style,
                font_size,
                line_height,
            };
            let current = entity.read(cx);
            if current.geometry.as_ref() != Some(&geometry)
                || (current.worker.is_none()
                    && current.prepared.as_ref().is_none_or(|prepared| {
                        prepared.source != current.source || prepared.geometry != geometry
                    }))
            {
                entity.update(cx, |text, cx| {
                    text.geometry = Some(geometry);
                    text.prepare_latest(cx);
                });
            }
        });

        if let Some(prepared) = self.prepared.clone() {
            let height = prepared.height;
            body.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        let mut origin = bounds.origin;
                        let line_height = prepared.geometry.line_height;
                        for line in &prepared.lines {
                            line.paint(
                                origin,
                                line_height,
                                prepared.geometry.style.text_align,
                                Some(bounds),
                                window,
                                cx,
                            )
                            .expect("paint prepared thinking text");
                            origin.y += line.size(line_height).height;
                        }
                    },
                )
                .w_full()
                .h(height),
            )
        } else {
            // A bounded prefix keeps the first paint immediate while the full layout is pending.
            let end = self.source.floor_char_boundary(INLINE_LAYOUT_BYTES);
            body.child(SharedString::from(&self.source[..end]))
        }
    }
}
