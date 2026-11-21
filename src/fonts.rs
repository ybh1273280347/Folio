//! Shared SVG font database and `usvg` fallback resolver across Mermaid,
//! draw.io, image viewing, and PDF export pipelines.
use std::sync::{Arc, LazyLock};

pub(crate) fn usvg_options() -> usvg::Options<'static> {
    static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        configure_generic_families(&mut database);
        Arc::new(database)
    });

    let default_font_selector = usvg::FontResolver::default_font_selector();
    let default_fallback_selector = usvg::FontResolver::default_fallback_selector();

    let select_font: usvg::FontSelectionFn<'static> = Box::new(move |font, db| {
        if let Some(id) = query_mapped_font(font, db) {
            return Some(id);
        }
        default_font_selector(font, db)
    });

    let select_fallback: usvg::FallbackSelectionFn<'static> =
        Box::new(move |ch, exclude_fonts, db| {
            if let Some(id) = select_preferred_fallback(ch, exclude_fonts, db) {
                return Some(id);
            }
            default_fallback_selector(ch, exclude_fonts, db)
        });

    let mut options = usvg::Options {
        font_resolver: usvg::FontResolver {
            select_font,
            select_fallback,
        },
        ..Default::default()
    };
    options.font_family = default_sans_family_name(&FONTS).to_owned();
    options.fontdb = FONTS.clone();
    options
}

fn has_family(db: &usvg::fontdb::Database, name: &str) -> bool {
    db.query(&usvg::fontdb::Query {
        families: &[usvg::fontdb::Family::Name(name)],
        ..Default::default()
    })
    .is_some()
}

fn default_sans_family_name(db: &usvg::fontdb::Database) -> &'static str {
    for candidate in [
        "Microsoft YaHei",
        "Segoe UI",
        "Arial",
        "PingFang SC",
        "Noto Sans CJK SC",
    ] {
        if has_family(db, candidate) {
            return candidate;
        }
    }
    "sans-serif"
}

fn configure_generic_families(db: &mut usvg::fontdb::Database) {
    for candidate in ["Microsoft YaHei", "Segoe UI", "Arial", "PingFang SC"] {
        if has_family(db, candidate) {
            db.set_sans_serif_family(candidate);
            break;
        }
    }
    for candidate in ["Consolas", "Courier New", "Menlo"] {
        if has_family(db, candidate) {
            db.set_monospace_family(candidate);
            break;
        }
    }
    for candidate in ["Times New Roman", "Georgia", "SimSun"] {
        if has_family(db, candidate) {
            db.set_serif_family(candidate);
            break;
        }
    }
}

fn query_mapped_font(
    font: &usvg::Font,
    db: &usvg::fontdb::Database,
) -> Option<usvg::fontdb::ID> {
    use usvg::fontdb::{Family, Query, Stretch, Style, Weight};

    let mut families: Vec<Family<'_>> = Vec::with_capacity(font.families().len() * 2 + 2);
    let mut wants_mono = false;
    let mut wants_serif = false;

    for family in font.families() {
        match family {
            usvg::FontFamily::Serif => {
                wants_serif = true;
                families.push(Family::Serif);
            }
            usvg::FontFamily::SansSerif => families.push(Family::SansSerif),
            usvg::FontFamily::Cursive => families.push(Family::Cursive),
            usvg::FontFamily::Fantasy => families.push(Family::Fantasy),
            usvg::FontFamily::Monospace => {
                wants_mono = true;
                families.push(Family::Monospace);
            }
            usvg::FontFamily::Named(raw) => {
                let clean = raw.trim().trim_matches(|c| c == '\'' || c == '"').trim();
                if clean.is_empty() {
                    continue;
                }
                let lower = clean.to_ascii_lowercase();
                match lower.as_str() {
                    "helvetica" | "helvetica neue" | "-apple-system" | "blinkmacsystemfont"
                    | "system-ui" | "ui-sans-serif" => {
                        families.push(Family::Name(clean));
                        families.push(Family::Name("Arial"));
                        families.push(Family::Name("Segoe UI"));
                        families.push(Family::Name("Microsoft YaHei"));
                    }
                    "pingfang sc" | "hiragino sans gb" | "microsoft yahei ui" => {
                        families.push(Family::Name(clean));
                        families.push(Family::Name("Microsoft YaHei"));
                        families.push(Family::Name("SimHei"));
                    }
                    "courier" | "menlo" | "monaco" | "sf mono" | "ui-monospace" => {
                        wants_mono = true;
                        families.push(Family::Name(clean));
                        families.push(Family::Name("Consolas"));
                        families.push(Family::Name("Courier New"));
                    }
                    "times" | "ui-serif" => {
                        wants_serif = true;
                        families.push(Family::Name(clean));
                        families.push(Family::Name("Times New Roman"));
                        families.push(Family::Name("SimSun"));
                    }
                    _ => families.push(Family::Name(clean)),
                }
            }
        }
    }

    if wants_mono {
        families.push(Family::Monospace);
    } else if wants_serif {
        families.push(Family::Serif);
    } else {
        families.push(Family::SansSerif);
    }

    let stretch = match font.stretch() {
        usvg::FontStretch::UltraCondensed => Stretch::UltraCondensed,
        usvg::FontStretch::ExtraCondensed => Stretch::ExtraCondensed,
        usvg::FontStretch::Condensed => Stretch::Condensed,
        usvg::FontStretch::SemiCondensed => Stretch::SemiCondensed,
        usvg::FontStretch::Normal => Stretch::Normal,
        usvg::FontStretch::SemiExpanded => Stretch::SemiExpanded,
        usvg::FontStretch::Expanded => Stretch::Expanded,
        usvg::FontStretch::ExtraExpanded => Stretch::ExtraExpanded,
        usvg::FontStretch::UltraExpanded => Stretch::UltraExpanded,
    };
    let style = match font.style() {
        usvg::FontStyle::Normal => Style::Normal,
        usvg::FontStyle::Italic => Style::Italic,
        usvg::FontStyle::Oblique => Style::Oblique,
    };

    db.query(&Query {
        families: &families,
        weight: Weight(font.weight()),
        stretch,
        style,
    })
}

fn select_preferred_fallback(
    ch: char,
    exclude_fonts: &[usvg::fontdb::ID],
    db: &usvg::fontdb::Database,
) -> Option<usvg::fontdb::ID> {
    use usvg::fontdb::{Family, Query, Style, Weight};

    let base_face = exclude_fonts.first().and_then(|id| db.face(*id));
    let target_weight = base_face.map(|f| f.weight).unwrap_or(Weight(400));
    let target_style = base_face.map(|f| f.style).unwrap_or(Style::Normal);

    const PREFERRED_FALLBACKS: &[&str] = &[
        "Microsoft YaHei",
        "Microsoft YaHei UI",
        "PingFang SC",
        "Noto Sans CJK SC",
        "SimHei",
        "DengXian",
        "Segoe UI Symbol",
        "Segoe UI Emoji",
        "Segoe UI",
        "Arial",
    ];

    for family_name in PREFERRED_FALLBACKS {
        for (weight, style) in [
            (target_weight, target_style),
            (Weight(400), Style::Normal),
        ] {
            let Some(id) = db.query(&Query {
                families: &[Family::Name(family_name)],
                weight,
                style,
                ..Default::default()
            }) else {
                continue;
            };
            if exclude_fonts.contains(&id) {
                continue;
            }
            let has_glyph = db
                .with_face_data(id, |data, index| {
                    ttf_parser::Face::parse(data, index)
                        .ok()
                        .and_then(|face| face.glyph_index(ch))
                        .is_some()
                })
                .unwrap_or(false);
            if has_glyph {
                return Some(id);
            }
        }
    }
    None
}
