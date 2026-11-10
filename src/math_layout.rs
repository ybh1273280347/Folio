//! In-process OpenType MATH layout; editor and PDF paint the same glyph positions.
use crate::model::MathFont;
use gpui_kit::*;
use latex_rust::{BoxContent, Dim, MathBox};
use std::{collections::HashMap, sync::OnceLock};

type LayoutError = Box<dyn std::error::Error>;
const EM: f32 = 10.;
static CJK_FONT: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
const FONTS: [(&str, &[u8]); 7] = [
    (
        "STIXTwoMath-Regular.otf",
        include_bytes!("../resources/fonts/STIXTwoMath-Regular.otf"),
    ),
    (
        "STIXTwoText-Regular.otf",
        include_bytes!("../resources/fonts/STIXTwoText-Regular.otf"),
    ),
    (
        "STIXTwoText-Italic.otf",
        include_bytes!("../resources/fonts/STIXTwoText-Italic.otf"),
    ),
    (
        "STIXTwoText-Bold.otf",
        include_bytes!("../resources/fonts/STIXTwoText-Bold.otf"),
    ),
    (
        "STIXTwoText-BoldItalic.otf",
        include_bytes!("../resources/fonts/STIXTwoText-BoldItalic.otf"),
    ),
    (
        "latinmodern-math.otf",
        include_bytes!("../resources/fonts/latinmodern-math.otf"),
    ),
    (
        "texgyrepagella-math.otf",
        include_bytes!("../resources/fonts/texgyrepagella-math.otf"),
    ),
];

pub fn register_fonts(cx: &App) {
    cx.text_system()
        .add_fonts(
            FONTS
                .iter()
                .map(|(_, bytes)| std::borrow::Cow::Borrowed(*bytes))
                .collect(),
        )
        .expect("bundled math fonts load");
}

/// Parse each font once per worker, rather than once per formula.
#[derive(Default)]
pub struct Typesetter {
    fonts: HashMap<&'static str, latex_rust::MathFont>,
}
impl Typesetter {
    pub fn typeset(
        &mut self,
        latex: &str,
        display: bool,
        selected: MathFont,
    ) -> Result<MathLayout, LayoutError> {
        if !self.fonts.contains_key(selected.filename()) {
            let bytes = FONTS
                .iter()
                .find(|(file, _)| *file == selected.filename())
                .expect("bundled math font")
                .1;
            // Fallback metrics must participate in layout, including fractions and accents.
            let font = latex_rust::MathFont::from_bytes(bytes)?.with_glyph_fallback(move |ch| {
                // Large text-font bytes are loaded only when a formula actually needs fallback.
                let bytes = CJK_FONT
                    .get_or_init(|| {
                        let windows = std::env::var_os("WINDIR").expect("Windows provides WINDIR");
                        std::fs::read(std::path::PathBuf::from(windows).join("Fonts/msyh.ttc"))
                            .map_err(|error| error.to_string())
                    })
                    .as_ref()
                    .map_err(|message| latex_rust::FontError::Fallback {
                        message: format!("中文字体加载失败：{message}"),
                    })?;
                let cjk = ttf_parser::Face::parse(bytes, 0)
                    .map_err(|_| latex_rust::FontError::InvalidFace)?;
                let Some(id) = cjk.glyph_index(ch) else {
                    return Ok(None);
                };
                let bounds = cjk.glyph_bounding_box(id);
                let advance_fu = cjk.glyph_hor_advance(id).expect("horizontal CJK font");
                let em = cjk.units_per_em();
                Ok(Some(latex_rust::GlyphMetrics {
                    fallback_font: true,
                    ch,
                    glyph_id: id.0,
                    advance_fu,
                    advance: Dim::from_font_units(advance_fu.into(), em),
                    height: Dim::from_font_units(bounds.map_or(0, |b| b.y_max.max(0)).into(), em),
                    depth: Dim::from_font_units(bounds.map_or(0, |b| (-b.y_min).max(0)).into(), em),
                }))
            });
            self.fonts.insert(selected.filename(), font);
        }
        let ast = latex_rust::parse(latex).map_err(latex_rust::Error::from)?;
        let style = if display {
            latex_rust::MathStyle::Display
        } else {
            latex_rust::MathStyle::Text
        };
        let boxed = latex_rust::layout(&ast, &self.fonts[selected.filename()], style)?;
        let mut layout = MathLayout {
            font: selected,
            operations: Vec::new(),
            width: dim(&boxed.width),
            height: dim(&boxed.height) + dim(&boxed.depth),
            baseline: dim(&boxed.height),
        };
        layout.place(&boxed, 0., layout.baseline, rgb(0x37352f).into());
        Ok(layout)
    }
}

enum Operation {
    Glyph {
        ch: char,
        id: u16,
        fallback: bool,
        x: f32,
        y: f32,
        size: f32,
        color: Hsla,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Hsla,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: Hsla,
    },
}
pub struct MathLayout {
    font: MathFont,
    operations: Vec<Operation>,
    pub width: f32,
    pub height: f32,
    pub baseline: f32,
}
fn dim(value: &Dim) -> f32 {
    f32::from_bits(value.to_ieee32_bits()) * EM
}
fn color(value: &latex_rust::Color) -> Hsla {
    rgb((value.r as u32) << 16 | (value.g as u32) << 8 | value.b as u32).into()
}

impl MathLayout {
    // Coordinates are baseline-relative; positive shift raises a child.
    fn place(&mut self, b: &MathBox, x: f32, parent_y: f32, ink: Hsla) {
        let y = parent_y - dim(&b.shift);
        let (w, h, d) = (dim(&b.width), dim(&b.height), dim(&b.depth));
        match &b.content {
            BoxContent::Empty | BoxContent::Kern(_) => {}
            BoxContent::Glyph {
                ch,
                glyph_id,
                scale,
                fallback_font,
                ..
            } => self.operations.push(Operation::Glyph {
                ch: *ch,
                id: *glyph_id,
                fallback: *fallback_font,
                x,
                y,
                size: dim(scale),
                color: ink,
            }),
            BoxContent::Rule => self.rect(x, y - h, w, h + d, ink),
            BoxContent::HList(children) => {
                let mut left = x;
                for child in children {
                    self.place(child, left, y, ink);
                    left += dim(&child.width);
                }
            }
            BoxContent::VList(children) => {
                if let Some(first) = children.first() {
                    self.place(first, x, y, ink);
                    let mut bottom = y + dim(&first.depth);
                    for child in children.iter().skip(1) {
                        self.place(child, x, bottom + dim(&child.height), ink);
                        bottom += dim(&child.height) + dim(&child.depth);
                    }
                }
            }
            BoxContent::Overlap(children) => {
                for child in children {
                    self.place(child, x, y, ink);
                }
            }
            BoxContent::Color(c, inner) => self.place(inner, x, y, color(c)),
            BoxContent::BackColor(c, inner) => {
                self.rect(x, y - h, w, h + d, color(c));
                self.place(inner, x, y, ink);
            }
            BoxContent::Line {
                x1,
                y1,
                x2,
                y2,
                thickness,
            } => self.operations.push(Operation::Line {
                x1: x + dim(x1),
                y1: y - dim(y1),
                x2: x + dim(x2),
                y2: y - dim(y2),
                thickness: dim(thickness),
                color: ink,
            }),
            BoxContent::Frame {
                thickness,
                stroke,
                inner,
            } => {
                let t = dim(thickness);
                let c = stroke.as_ref().map(color).unwrap_or(ink);
                self.rect(x, y - h, w, t, c);
                self.rect(x, y + d - t, w, t, c);
                self.rect(x, y - h, t, h + d, c);
                self.rect(x + w - t, y - h, t, h + d, c);
                self.place(inner, x, y, ink);
            }
        }
    }
    fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: Hsla) {
        self.operations.push(Operation::Rect {
            x,
            y,
            width,
            height,
            color,
        });
    }
    pub fn paint(&self, origin: Point<Pixels>, scale: f32, window: &mut Window, cx: &App) {
        let main = cx.text_system().resolve_font(&font(self.font.family()));
        let mut fallback = None;
        for op in &self.operations {
            match *op {
                Operation::Glyph {
                    ch,
                    id,
                    fallback: other,
                    x,
                    y,
                    size,
                    color,
                } => {
                    if window.is_vector_capture() {
                        window.set_vector_text(ch.to_string());
                    }
                    let face = if other {
                        *fallback.get_or_insert_with(|| {
                            cx.text_system().resolve_font(&font("Microsoft YaHei"))
                        })
                    } else {
                        main
                    };
                    window
                        .paint_glyph(
                            origin + point(px(x * scale), px(y * scale)),
                            face,
                            GlyphId(id.into()),
                            px(size * scale),
                            color,
                        )
                        .expect("native formula glyph paints");
                }
                Operation::Rect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => window.paint_quad(fill(
                    Bounds::new(
                        origin + point(px(x * scale), px(y * scale)),
                        size(px(width * scale), px(height * scale)),
                    ),
                    color,
                )),
                Operation::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    thickness,
                    color,
                } => {
                    let mut path = PathBuilder::stroke(px(thickness * scale));
                    path.move_to(origin + point(px(x1 * scale), px(y1 * scale)));
                    path.line_to(origin + point(px(x2 * scale), px(y2 * scale)));
                    window.paint_path(path.build().expect("formula line"), color);
                }
            }
        }
    }
}
