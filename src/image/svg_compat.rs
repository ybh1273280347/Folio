use std::collections::HashMap;

use super::clipboard::sanitize_svg_bytes;

#[derive(Debug, Clone)]
pub struct PreprocessedSvg {
    pub bytes: Vec<u8>,
    pub has_complex_rich_text: bool,
}

/// Expand CSS Color 5 `light-dark(light, dark)` expressions (including nested
/// `rgb(...)` / `rgba(...)` arguments) into their light-theme color value.
pub fn expand_light_dark(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let bytes = input.as_bytes();
    const NEEDLE: &[u8] = b"light-dark(";

    while i < bytes.len() {
        if i + NEEDLE.len() <= bytes.len()
            && bytes[i..i + NEEDLE.len()].eq_ignore_ascii_case(NEEDLE)
        {
            let prev_is_ident = i > 0
                && matches!(
                    bytes[i - 1],
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_'
                );
            if !prev_is_ident
                && let Some((light_arg, end_pos)) = parse_light_dark_call(&input[i + NEEDLE.len()..])
            {
                let expanded_inner = expand_light_dark(light_arg.trim());
                out.push_str(&expanded_inner);
                i += NEEDLE.len() + end_pos;
                continue;
            }
        }
        let ch = input[i..].chars().next().expect("valid UTF-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn parse_light_dark_call(after_open: &str) -> Option<(&str, usize)> {
    let mut depth = 1usize;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut first_comma: Option<usize> = None;
    let bytes = after_open.as_bytes();

    for (idx, &b) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => quote = Some(b),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let comma = first_comma?;
                    return Some((&after_open[..comma], idx + 1));
                }
            }
            b',' if depth == 1 && first_comma.is_none() => {
                first_comma = Some(idx);
            }
            _ => {}
        }
    }
    None
}

/// Preprocess draw.io / browser SVGs for pure-Rust `usvg`/`resvg` + `krilla-svg`:
/// 1. Strip trailing NUL bytes;
/// 2. Expand `light-dark(light, dark)` in both attributes and `style="..."`;
/// 3. Convert `<switch>`/`<foreignObject>` HTML labels into multi-line `<text><tspan>`
///    and detect complex rich text (`mjx-container`, `<table>`, etc.).
pub fn preprocess_svg_bytes(raw: &[u8]) -> PreprocessedSvg {
    let clean = sanitize_svg_bytes(raw);
    let text = String::from_utf8_lossy(&clean);
    let expanded = expand_light_dark(&text);

    let lower_probe = expanded.to_ascii_lowercase();
    if !lower_probe.contains("<foreignobject")
        && !lower_probe.contains("<switch")
        && !expanded.contains("Text is not SVG - cannot display")
    {
        return PreprocessedSvg {
            bytes: expanded.into_bytes(),
            has_complex_rich_text: false,
        };
    }

    let Ok(doc) = roxmltree::Document::parse(&expanded) else {
        return PreprocessedSvg {
            bytes: expanded.into_bytes(),
            has_complex_rich_text: false,
        };
    };

    let mut replacements: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut has_complex_rich_text = false;

    collect_replacements(doc.root(), &mut replacements, &mut has_complex_rich_text);
    if replacements.is_empty() {
        return PreprocessedSvg {
            bytes: expanded.into_bytes(),
            has_complex_rich_text,
        };
    }

    replacements.sort_by_key(|(range, _)| range.start);
    let mut result = String::with_capacity(expanded.len());
    let mut cursor = 0usize;
    for (range, replacement) in replacements {
        if range.start < cursor || range.end > expanded.len() {
            continue;
        }
        result.push_str(&expanded[cursor..range.start]);
        result.push_str(&replacement);
        cursor = range.end;
    }
    result.push_str(&expanded[cursor..]);

    PreprocessedSvg {
        bytes: result.into_bytes(),
        has_complex_rich_text,
    }
}

fn collect_replacements(
    node: roxmltree::Node<'_, '_>,
    replacements: &mut Vec<(std::ops::Range<usize>, String)>,
    has_complex_rich_text: &mut bool,
) {
    if node.is_element() {
        let tag = node.tag_name().name();
        if tag.eq_ignore_ascii_case("switch") {
            if let Some(fo) = node
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("foreignObject"))
            {
                if contains_complex_rich_text(fo) {
                    *has_complex_rich_text = true;
                }
                let fallback_text = find_switch_fallback_text(node);
                if let Some(svg_text) = convert_foreign_object_to_svg_text(fo, fallback_text) {
                    replacements.push((node.range(), svg_text));
                    return;
                }
                if let Some(fb) = fallback_text
                    && is_drawio_warning_text(fb)
                {
                    replacements.push((node.range(), String::new()));
                    return;
                }
            }
        } else if tag.eq_ignore_ascii_case("foreignObject") {
            if contains_complex_rich_text(node) {
                *has_complex_rich_text = true;
            }
            if let Some(svg_text) = convert_foreign_object_to_svg_text(node, None) {
                replacements.push((node.range(), svg_text));
                return;
            }
        } else if tag.eq_ignore_ascii_case("a")
            && node.children().any(|c| {
                c.is_element()
                    && c.tag_name().name().eq_ignore_ascii_case("text")
                    && is_drawio_warning_text(c)
            })
        {
            replacements.push((node.range(), String::new()));
            return;
        } else if tag.eq_ignore_ascii_case("text") && is_drawio_warning_text(node) {
            replacements.push((node.range(), String::new()));
            return;
        }
    }

    for child in node.children() {
        collect_replacements(child, replacements, has_complex_rich_text);
    }
}

fn contains_complex_rich_text(fo: roxmltree::Node<'_, '_>) -> bool {
    fo.descendants().any(|n| {
        n.is_element()
            && matches!(
                n.tag_name().name().to_ascii_lowercase().as_str(),
                "mjx-container" | "table" | "math"
            )
    })
}

fn find_switch_fallback_text<'a, 'input>(
    switch_node: roxmltree::Node<'a, 'input>,
) -> Option<roxmltree::Node<'a, 'input>> {
    switch_node.children().find_map(|child| {
        if !child.is_element() {
            return None;
        }
        let name = child.tag_name().name();
        if name.eq_ignore_ascii_case("text") {
            Some(child)
        } else if name.eq_ignore_ascii_case("a") {
            child
                .children()
                .find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case("text"))
        } else {
            None
        }
    })
}

fn is_drawio_warning_text(text_node: roxmltree::Node<'_, '_>) -> bool {
    let mut buf = String::new();
    for desc in text_node.descendants() {
        if desc.is_text()
            && let Some(t) = desc.text()
        {
            buf.push_str(t);
        }
    }
    buf.trim().eq_ignore_ascii_case("Text is not SVG - cannot display")
}

#[derive(Default)]
struct TextStyleProps {
    padding_top: Option<f32>,
    margin_left: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    align_items: Option<String>,
    justify_content: Option<String>,
    text_align: Option<String>,
    color: Option<String>,
    font_size: Option<f32>,
    font_family: Option<String>,
    font_weight: Option<String>,
    font_style: Option<String>,
}

fn parse_inline_style(style: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        if !key.is_empty() && !val.is_empty() {
            map.insert(key, val.to_owned());
        }
    }
    map
}

fn parse_px(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    let num = trimmed
        .strip_suffix("px")
        .or_else(|| trimmed.strip_suffix("pt"))
        .unwrap_or(trimmed)
        .trim();
    num.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn collect_style_props(fo: roxmltree::Node<'_, '_>) -> TextStyleProps {
    let mut props = TextStyleProps::default();

    if let Some(w) = fo.attribute("width").and_then(parse_px) {
        props.width = Some(w);
    }
    if let Some(h) = fo.attribute("height").and_then(parse_px) {
        props.height = Some(h);
    }

    for node in fo.descendants().filter(|n| n.is_element()) {
        let tag = node.tag_name().name().to_ascii_lowercase();
        if matches!(tag.as_str(), "b" | "strong") && props.font_weight.is_none() {
            props.font_weight = Some("bold".to_owned());
        } else if matches!(tag.as_str(), "i" | "em") && props.font_style.is_none() {
            props.font_style = Some("italic".to_owned());
        } else if tag == "font" {
            if let Some(color) = node.attribute("color").map(str::trim).filter(|c| !c.is_empty()) {
                props.color = Some(color.to_owned());
            }
            if let Some(face) = node.attribute("face").map(str::trim).filter(|f| !f.is_empty()) {
                props.font_family = Some(face.to_owned());
            }
        }

        let Some(style_str) = node.attribute("style") else {
            continue;
        };
        let map = parse_inline_style(style_str);
        if let Some(v) = map.get("padding-top").and_then(|s| parse_px(s))
            && props.padding_top.is_none()
        {
            props.padding_top = Some(v);
        }
        if let Some(v) = map.get("margin-left").and_then(|s| parse_px(s))
            && props.margin_left.is_none()
        {
            props.margin_left = Some(v);
        }
        if let Some(v) = map.get("width").and_then(|s| parse_px(s)) {
            props.width = Some(v);
        }
        if let Some(v) = map.get("height").and_then(|s| parse_px(s)) {
            props.height = Some(v);
        }
        if let Some(v) = map.get("align-items")
            && props.align_items.is_none()
        {
            props.align_items = Some(v.to_ascii_lowercase());
        }
        if let Some(v) = map.get("justify-content")
            && props.justify_content.is_none()
        {
            props.justify_content = Some(v.to_ascii_lowercase());
        }
        if let Some(v) = map.get("text-align") {
            props.text_align = Some(v.to_ascii_lowercase());
        }
        if let Some(v) = map.get("color") {
            props.color = Some(v.clone());
        }
        if let Some(v) = map.get("font-size").and_then(|s| parse_px(s))
            && v > 0.0
        {
            props.font_size = Some(v);
        }
        if let Some(v) = map.get("font-family") {
            props.font_family = Some(v.clone());
        }
        if let Some(v) = map.get("font-weight") {
            props.font_weight = Some(v.clone());
        }
        if let Some(v) = map.get("font-style") {
            props.font_style = Some(v.clone());
        }
    }

    props
}

fn extract_foreign_object_lines(fo: roxmltree::Node<'_, '_>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for child in fo.children() {
        walk_html_lines(child, &mut lines, &mut current);
    }
    flush_trimmed_line(&mut lines, &mut current, false);
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

fn flush_trimmed_line(lines: &mut Vec<String>, current: &mut String, allow_empty: bool) {
    let trimmed = current.trim();
    if !trimmed.is_empty() || allow_empty {
        lines.push(trimmed.to_owned());
    }
    current.clear();
}

fn walk_html_lines(
    node: roxmltree::Node<'_, '_>,
    lines: &mut Vec<String>,
    current: &mut String,
) {
    if node.is_text() {
        if let Some(raw) = node.text() {
            push_collapsed_html_text(current, raw);
        }
        return;
    }
    if !node.is_element() {
        return;
    }
    let tag = node.tag_name().name().to_ascii_lowercase();
    if matches!(tag.as_str(), "style" | "script") {
        return;
    }
    if tag == "br" {
        flush_trimmed_line(lines, current, true);
        return;
    }

    let is_block = matches!(
        tag.as_str(),
        "div" | "p" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
    );
    if is_block && !current.trim().is_empty() {
        flush_trimmed_line(lines, current, false);
    }

    for child in node.children() {
        walk_html_lines(child, lines, current);
    }

    if is_block && !current.trim().is_empty() {
        flush_trimmed_line(lines, current, false);
    }
}

fn push_collapsed_html_text(dst: &mut String, raw: &str) {
    let mut prev_space = dst.ends_with(' ');
    for ch in raw.chars() {
        if ch == '\u{00a0}' {
            dst.push(' ');
            prev_space = false;
        } else if ch.is_whitespace() {
            if !prev_space && !dst.is_empty() {
                dst.push(' ');
                prev_space = true;
            }
        } else {
            dst.push(ch);
            prev_space = false;
        }
    }
}

fn convert_foreign_object_to_svg_text(
    fo: roxmltree::Node<'_, '_>,
    fallback_text: Option<roxmltree::Node<'_, '_>>,
) -> Option<String> {
    let lines = extract_foreign_object_lines(fo);
    if lines.is_empty() {
        return None;
    }

    let props = collect_style_props(fo);
    let usable_fallback = fallback_text.filter(|fb| !is_drawio_warning_text(*fb));

    let font_size = props
        .font_size
        .or_else(|| {
            usable_fallback
                .and_then(|fb| fb.attribute("font-size"))
                .and_then(parse_px)
        })
        .unwrap_or(12.0)
        .max(1.0);
    let line_height = font_size * 1.2;

    let text_anchor = match props
        .text_align
        .as_deref()
        .or(props.justify_content.as_deref())
    {
        Some(v) if v.contains("right") || v.contains("flex-end") || v == "end" => "end",
        Some(v) if v.contains("left") || v.contains("flex-start") || v == "start" => "start",
        Some(v) if v.contains("center") => "middle",
        _ => usable_fallback
            .and_then(|fb| fb.attribute("text-anchor"))
            .unwrap_or("middle"),
    };

    let x = if let Some(margin_left) = props.margin_left {
        let width = props.width.unwrap_or(0.0);
        match text_anchor {
            "middle" => margin_left + width * 0.5,
            "end" => margin_left + width,
            _ => margin_left,
        }
    } else if let Some(fb_x) = usable_fallback
        .and_then(|fb| fb.attribute("x"))
        .and_then(parse_px)
    {
        fb_x
    } else {
        let fo_x = fo.attribute("x").and_then(parse_px).unwrap_or(0.0);
        let fo_w = props.width.unwrap_or(0.0);
        match text_anchor {
            "middle" => fo_x + fo_w * 0.5,
            "end" => fo_x + fo_w,
            _ => fo_x,
        }
    };

    let y_first = if let Some(padding_top) = props.padding_top {
        let align = props.align_items.as_deref().unwrap_or("center");
        let total_span = (lines.len().saturating_sub(1) as f32) * line_height;
        if align.contains("flex-start") || align.contains("start") {
            padding_top + font_size * 0.85
        } else if align.contains("flex-end") || align.contains("end") {
            padding_top - total_span - font_size * 0.15
        } else {
            padding_top - total_span * 0.5 + font_size * 0.35
        }
    } else if let Some(fb_y) = usable_fallback
        .and_then(|fb| fb.attribute("y"))
        .and_then(parse_px)
    {
        let total_span = (lines.len().saturating_sub(1) as f32) * line_height;
        fb_y - total_span * 0.5
    } else {
        let fo_y = fo.attribute("y").and_then(parse_px).unwrap_or(0.0);
        let fo_h = props.height.unwrap_or(font_size * 1.4);
        let total_span = (lines.len().saturating_sub(1) as f32) * line_height;
        fo_y + fo_h * 0.5 - total_span * 0.5 + font_size * 0.35
    };

    let fill = props
        .color
        .as_deref()
        .or_else(|| usable_fallback.and_then(|fb| fb.attribute("fill")))
        .unwrap_or("#000000");
    let font_family = props
        .font_family
        .as_deref()
        .or_else(|| usable_fallback.and_then(|fb| fb.attribute("font-family")))
        .unwrap_or("Helvetica, Arial, sans-serif");

    let mut out = format!(
        r#"<text x="{x:.2}" y="{y_first:.2}" fill="{}" font-family="{}" font-size="{font_size:.2}px" text-anchor="{text_anchor}""#,
        escape_xml_attr(fill),
        escape_xml_attr(font_family),
    );
    if let Some(weight) = props.font_weight.as_deref() {
        out.push_str(&format!(r#" font-weight="{}""#, escape_xml_attr(weight)));
    }
    if let Some(style) = props.font_style.as_deref() {
        out.push_str(&format!(r#" font-style="{}""#, escape_xml_attr(style)));
    }
    out.push('>');

    for (idx, line) in lines.iter().enumerate() {
        let dy = if idx == 0 { "0" } else { "1.2em" };
        out.push_str(&format!(
            r#"<tspan x="{x:.2}" dy="{dy}">{}</tspan>"#,
            escape_xml_text(line)
        ));
    }
    out.push_str("</text>");
    Some(out)
}

fn escape_xml_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_light_dark_in_attributes_and_styles_with_nested_rgb() {
        let svg = r##"<rect fill="light-dark(#ffffff, #121212)" style="stroke: light-dark(rgb(10, 20, 30), rgba(255, 255, 255, 0.5)); fill: light-dark( #f0f9ff , #0f172a );"/>"##;
        let expanded = expand_light_dark(svg);
        assert_eq!(
            expanded,
            r##"<rect fill="#ffffff" style="stroke: rgb(10, 20, 30); fill: #f0f9ff;"/>"##
        );
    }

    #[test]
    fn converts_drawio_multiline_foreign_object_to_tspan_and_strips_warning() {
        let drawio_svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <g>
    <rect x="20" y="20" width="120" height="60" fill="light-dark(#dae8fc, #1e293b)" stroke="light-dark(#6c8ebf, #64748b)"/>
    <switch>
      <foreignObject style="overflow: visible; text-align: left;" width="100%" height="100%">
        <div xmlns="http://www.w3.org/1999/xhtml" style="display: flex; align-items: unsafe center; justify-content: unsafe center; width: 118px; height: 1px; padding-top: 50px; margin-left: 21px;">
          <div style="box-sizing: border-box; text-align: center; color: light-dark(#1e293b, #f8fafc); ">
            <div style="display: inline-block; font-size: 14px; font-family: Helvetica; line-height: 1.2;">第一行<br/>第二行<div>第三行</div></div>
          </div>
        </div>
      </foreignObject>
      <text x="80" y="54" fill="#000000" text-anchor="middle">第一行</text>
    </switch>
  </g>
  <switch>
    <g/>
    <a href="https://www.drawio.com/doc/faq/svg-export-text-problems"><text x="100" y="95" text-anchor="middle">Text is not SVG - cannot display</text></a>
  </switch>
</svg>
"##;

        let preprocessed = preprocess_svg_bytes(drawio_svg.as_bytes());
        assert!(!preprocessed.has_complex_rich_text);
        let out = String::from_utf8(preprocessed.bytes).unwrap();
        assert!(!out.contains("light-dark("));
        assert!(!out.contains("Text is not SVG - cannot display"));
        assert!(out.contains(r##"fill="#dae8fc""##));
        assert!(out.contains(r##"fill="#1e293b""##));
        assert!(out.contains(r#"<tspan x="80.00" dy="0">第一行</tspan>"#));
        assert!(out.contains(r#"<tspan x="80.00" dy="1.2em">第二行</tspan>"#));
        assert!(out.contains(r#"<tspan x="80.00" dy="1.2em">第三行</tspan>"#));
        assert!(usvg::Tree::from_str(&out, &usvg::Options::default()).is_ok());
    }

    #[test]
    fn detects_complex_rich_text_for_math_and_tables() {
        let math_svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40">
  <switch>
    <foreignObject width="100%" height="100%">
      <div xmlns="http://www.w3.org/1999/xhtml"><mjx-container>x^2</mjx-container></div>
    </foreignObject>
    <text x="50" y="20">x^2</text>
  </switch>
</svg>"#;
        assert!(preprocess_svg_bytes(math_svg.as_bytes()).has_complex_rich_text);

        let table_svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40">
  <switch>
    <foreignObject width="100%" height="100%">
      <div xmlns="http://www.w3.org/1999/xhtml"><table><tr><td>A</td></tr></table></div>
    </foreignObject>
    <text x="50" y="20">A</text>
  </switch>
</svg>"#;
        assert!(preprocess_svg_bytes(table_svg.as_bytes()).has_complex_rich_text);
    }
}
