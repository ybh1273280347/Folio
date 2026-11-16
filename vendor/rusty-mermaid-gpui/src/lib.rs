mod paint;

use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::base::StyledExt as _;
use gpui::{App, IntoElement, Styled, Window, canvas, div, px};
use rusty_mermaid_core::{Scene, Theme, Transform};

/// Scene data prepared for repeated GPUI paints.
///
/// Inline Markdown and text runs are prepared once. Geometry and shaping are
/// retained for the last two effective scales.
pub struct PreparedScene {
    pub(crate) scene: Arc<Scene>,
    pub(crate) theme: Theme,
    pub(crate) template: paint::PreparedCache,
    pub(crate) scales: Vec<paint::PreparedScale>,
    paint_error: RefCell<Option<String>>,
}

impl PreparedScene {
    /// Prepare a scene for native GPUI rendering.
    pub fn new(scene: Arc<Scene>, theme: Theme) -> Result<Self, String> {
        validate_scene(&scene)?;
        Ok(Self {
            template: paint::PreparedCache::from_scene(&scene, &theme),
            scene,
            theme,
            scales: Vec::new(),
            paint_error: RefCell::new(None),
        })
    }

    /// A drawing failure is surfaced by the host on the following frame.
    pub fn paint_error(&self) -> Option<String> {
        self.paint_error.borrow().clone()
    }

    pub(crate) fn record_paint_error(&self, error: String, window: &mut Window, cx: &mut App) {
        *self.paint_error.borrow_mut() = Some(error);
        // Do not publish a partially painted diagram. The next render replaces
        // it with the host's normal failure element, including during export.
        window.vector_image_pending();
        cx.defer(|cx| cx.refresh_windows());
    }

    pub(crate) fn prepare(
        &mut self,
        scale: f32,
        window: &mut Window,
    ) -> paint::PreparedFrame {
        if let Some(index) = self.scales.iter().position(|entry| entry.scale == scale) {
            let entry = self.scales.remove(index);
            let frame = entry.frame();
            self.scales.push(entry);
            return frame;
        }

        let mut cache = self.template.clone();
        let padding = self.theme.padding as f32 * scale;
        let entry = match paint::prepare_cache(
            &mut cache,
            &self.scene,
            &self.theme,
            scale,
            padding,
            padding,
            window,
        ) {
            Ok(()) => paint::PreparedScale::success(scale, Rc::new(cache)),
            Err(error) => {
                paint::PreparedScale::failure(scale, paint::prepare_error(window, error))
            }
        };
        let frame = entry.frame();
        if self.scales.len() == 2 {
            self.scales.remove(0);
        }
        self.scales.push(entry);
        frame
    }

    pub(crate) fn paint(
        &self,
        bounds: gpui::Bounds<gpui::Pixels>,
        frame: &paint::PreparedFrame,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint::paint_scene(self, bounds, frame, window, cx);
    }
}

/// Reject transforms unsupported by the native drawing backend.
pub fn validate_scene(scene: &Scene) -> Result<(), String> {
    for element in scene.elements() {
        validate_primitive(&element.primitive)?;
    }
    Ok(())
}

fn validate_primitive(primitive: &rusty_mermaid_core::Primitive) -> Result<(), String> {
    if let rusty_mermaid_core::Primitive::Group { transform, children } = primitive {
        match transform {
            Transform::Rotate { degrees, .. } if *degrees != 0.0 => {
                return Err("当前原生绘制后端不支持此图表变换".to_owned());
            }
            Transform::Scale(sx, sy) if sx != sy => {
                return Err("当前原生绘制后端不支持此图表变换".to_owned());
            }
            Transform::Identity
            | Transform::Translate(..)
            | Transform::Scale(..)
            | Transform::Rotate { .. } => {}
        }
        for child in children {
            validate_primitive(child)?;
        }
    }
    Ok(())
}

/// Create a styled GPUI element that fits the prepared scene into its width.
pub fn render_element(
    prepared: Rc<RefCell<PreparedScene>>,
    font_scale: f32,
) -> impl IntoElement + Styled {
    let (scene_width, scene_height, padding) = {
        let prepared = prepared.borrow();
        (
            prepared.scene.width as f32,
            prepared.scene.height as f32,
            prepared.theme.padding as f32,
        )
    };
    let natural_width = scene_width + padding * 2.0;
    let natural_height = scene_height + padding * 2.0;
    let prepaint_prepared = Rc::clone(&prepared);
    let paint_prepared = Rc::clone(&prepared);

    div().w_full().flex().justify_center().child(
        canvas(
            move |bounds, window, _cx| {
                let width: f32 = bounds.size.width.into();
                let scale = width / natural_width;
                prepaint_prepared.borrow_mut().prepare(scale, window)
            },
            move |bounds, frame, window, cx| {
                paint_prepared
                    .borrow()
                    .paint(bounds, &frame, window, cx);
            },
        )
        .w_full()
        .max_w(px(natural_width * font_scale))
        .aspect_ratio(natural_width / natural_height),
    )
}
