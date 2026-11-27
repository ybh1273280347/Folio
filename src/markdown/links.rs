//! Markdown link extraction, path-relocation rewriting, and SQLite backlink index.
//!
//! Uses `.folio/metadata.sqlite3` alongside `canvas_links` so Markdown cross-references
//! and asset links can be queried as backlinks and automatically refactored when files
//! or folders are renamed or moved inside the workspace.

use crate::canvas::anchor::remap_rel_path;
use crate::files;
use markdown::mdast::Node;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownLinkKind {
    Link,
    Image,
    Definition,
}

impl MarkdownLinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Image => "image",
            Self::Definition => "definition",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "image" => Self::Image,
            "definition" => Self::Definition,
            _ => Self::Link,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedMarkdownLink {
    pub target_path: String,
    pub anchor: Option<String>,
    pub label: String,
    pub line: usize,
    pub kind: MarkdownLinkKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownBacklink {
    pub source_path: String,
    pub target_path: String,
    pub anchor: Option<String>,
    pub label: String,
    pub line: usize,
    pub kind: MarkdownLinkKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathStyle {
    RootRelative,
    DotRelative,
    Relative,
}

#[derive(Debug)]
struct LocatedLink {
    dest_start: usize,
    dest_end: usize,
    angle_bracketed: bool,
    url: String,
    label: String,
    line: usize,
    kind: MarkdownLinkKind,
}

fn markdown_parse_options() -> markdown::ParseOptions {
    let mut opts = markdown::ParseOptions::gfm();
    opts.constructs.math_text = true;
    opts.constructs.math_flow = true;
    opts
}

/// Extracts all project-internal Markdown links, images, and link definitions from `text`.
pub fn extract_markdown_links(source_path: &str, text: &str) -> Vec<ExtractedMarkdownLink> {
    let source_path = normalize_slash(source_path);
    let mut out = Vec::new();
    for located in collect_located_links(text) {
        let Some(parsed) = parse_local_target(&source_path, &located.url) else {
            continue;
        };
        out.push(ExtractedMarkdownLink {
            target_path: parsed.target_path,
            anchor: parsed.anchor,
            label: located.label,
            line: located.line,
            kind: located.kind,
        });
    }
    out
}

/// Rewrites all affected project-internal links in `text` after a rename or move.
///
/// `old_source_path` is where the Markdown file lived before `moves`; `new_source_path` is
/// where it lives now. Returns `Some((updated_text, replaced_count))` only when at least one
/// link destination actually changed.
pub fn rewrite_markdown_links(
    text: &str,
    old_source_path: &str,
    new_source_path: &str,
    moves: &[(String, String)],
    is_folder: bool,
) -> Option<(String, usize)> {
    if moves.is_empty() {
        return None;
    }
    let old_source = normalize_slash(old_source_path);
    let new_source = normalize_slash(new_source_path);
    let source_dir_changed = parent_dir(&old_source) != parent_dir(&new_source);

    let located = collect_located_links(text);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    for item in located {
        let Some(parsed) = parse_local_target(&old_source, &item.url) else {
            continue;
        };
        let remapped_target = remap_rel_path_compat(&parsed.target_path, moves, is_folder);
        let target_changed = remapped_target.is_some();
        if !target_changed && (!source_dir_changed || parsed.style == PathStyle::RootRelative) {
            continue;
        }
        let next_target = remapped_target.as_deref().unwrap_or(&parsed.target_path);
        // A co-moved relative target can still resolve correctly with its original spelling.
        if parse_local_target(&new_source, &item.url)
            .is_some_and(|current| current.target_path == next_target)
        {
            continue;
        }
        let next_path = match parsed.style {
            PathStyle::RootRelative => format!("/{next_target}"),
            PathStyle::DotRelative => {
                let rel = relative_path(parent_dir(&new_source), next_target);
                if rel.starts_with("../") || rel.starts_with("./") {
                    rel
                } else {
                    format!("./{rel}")
                }
            }
            PathStyle::Relative => relative_path(parent_dir(&new_source), next_target),
        };
        let formatted_path = encode_destination(&next_path, item.angle_bracketed, false);
        let replacement = match &parsed.anchor {
            Some(anchor) => format!(
                "{formatted_path}#{}",
                encode_destination(anchor, item.angle_bracketed, true)
            ),
            None => formatted_path,
        };
        edits.push((item.dest_start, item.dest_end, replacement));
    }

    if edits.is_empty() {
        return None;
    }

    // AST destinations are disjoint, including images nested in link labels.
    // Copy the gaps once so large refactors do not repeatedly shift the rest of the file.
    edits.sort_unstable_by_key(|edit| edit.0);
    let replaced = edits.len();
    let mut updated = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end, replacement) in edits {
        updated.push_str(&text[cursor..start]);
        updated.push_str(&replacement);
        cursor = end;
    }
    updated.push_str(&text[cursor..]);
    Some((updated, replaced))
}

/// Inverse-maps a current path back to its pre-move path if it was part of `moves`.
pub fn reverse_remap_rel_path(
    current_path: &str,
    moves: &[(String, String)],
    is_folder: bool,
) -> Option<String> {
    let normalized = normalize_slash(current_path);
    for (old, new) in moves {
        let old_norm = normalize_slash(old);
        let new_norm = normalize_slash(new);
        if path_eq(&normalized, &new_norm) {
            return Some(old_norm);
        }
        if is_folder {
            let prefix = format!("{new_norm}/");
            if let Some(rest) = strip_path_prefix(&normalized, &prefix) {
                return Some(format!("{old_norm}/{rest}"));
            }
        }
    }
    None
}

/// Case-aware path remapping matching Windows NTFS semantics on Windows and exact match elsewhere.
pub fn remap_rel_path_compat(
    path: &str,
    moves: &[(String, String)],
    is_folder: bool,
) -> Option<String> {
    if let Some(exact) = remap_rel_path(path, moves, is_folder) {
        return Some(exact);
    }
    #[cfg(windows)]
    {
        let normalized = normalize_slash(path);
        for (old, new) in moves {
            let old_norm = normalize_slash(old);
            let new_norm = normalize_slash(new);
            if normalized.eq_ignore_ascii_case(&old_norm) {
                return Some(new_norm);
            }
            if is_folder {
                let prefix = format!("{old_norm}/");
                if let Some(rest) = strip_path_prefix(&normalized, &prefix) {
                    return Some(format!("{new_norm}/{rest}"));
                }
            }
        }
    }
    None
}

fn path_eq(a: &str, b: &str) -> bool {
    #[cfg(windows)]
    {
        a.eq_ignore_ascii_case(b)
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if let Some(rest) = path.strip_prefix(prefix) {
        return Some(rest);
    }
    #[cfg(windows)]
    {
        let prefix_len = prefix.len();
        if path.len() >= prefix_len
            && path.is_char_boundary(prefix_len)
            && path[..prefix_len].eq_ignore_ascii_case(prefix)
        {
            return Some(&path[prefix_len..]);
        }
    }
    None
}

fn normalize_slash(path: &str) -> String {
    path.replace('\\', "/")
}

fn parent_dir(rel_path: &str) -> &str {
    match rel_path.rfind('/') {
        Some(idx) => &rel_path[..idx],
        None => "",
    }
}

struct ParsedLocalTarget {
    target_path: String,
    anchor: Option<String>,
    style: PathStyle,
}

/// Parses the semantic URL from the Markdown AST. The original destination span
/// is used only for editing, since its escapes and character references are syntax.
fn parse_local_target(source_path: &str, url: &str) -> Option<ParsedLocalTarget> {
    let raw = url.trim();
    if raw.is_empty() || raw.starts_with('#') || raw.starts_with("//") {
        return None;
    }
    if let Ok(parsed) = url::Url::parse(raw) {
        // Skip external schemes (`https://`, `mailto:`, `file://`, `data:`, etc.).
        if parsed.scheme().len() > 1 {
            return None;
        }
    }
    let (raw_path, anchor) = match raw.split_once('#') {
        Some((path, frag)) => (path, Some(frag.to_owned())),
        None => (raw, None),
    };
    let decoded = files::percent_decode(raw_path);
    if decoded.is_empty() {
        return None;
    }
    let normalized = decoded.replace('\\', "/");
    // Skip Windows drive-letter absolute paths like `C:/...`.
    let bytes = normalized.as_bytes();
    if normalized.starts_with("//")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return None;
    }

    let (style, resolved) = if let Some(ws_rel) = normalized.strip_prefix('/') {
        (PathStyle::RootRelative, normalize_lexical("", ws_rel)?)
    } else if normalized.starts_with("./") {
        (
            PathStyle::DotRelative,
            normalize_lexical(parent_dir(source_path), &normalized)?,
        )
    } else {
        (
            PathStyle::Relative,
            normalize_lexical(parent_dir(source_path), &normalized)?,
        )
    };
    Some(ParsedLocalTarget {
        target_path: resolved,
        anchor,
        style,
    })
}

/// Computes a standard Markdown link destination from `source_rel` to `target_rel`,
/// preserving readable UTF-8 paths while percent-encoding spaces and delimiters.
pub fn format_relative_link_url(
    source_rel: &str,
    target_rel: &str,
    anchor: Option<&str>,
) -> String {
    let source_norm = normalize_slash(source_rel);
    let target_norm = normalize_slash(target_rel);
    let rel = relative_path(parent_dir(&source_norm), &target_norm);
    let encoded = encode_destination(&rel, false, false);
    match format_heading_fragment(anchor.unwrap_or_default()) {
        Some(fragment) => format!("{encoded}{fragment}"),
        None => encoded,
    }
}

/// A completion starts from the literal heading title, including any percent signs.
pub fn format_heading_fragment(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()).then(|| format!("#{}", encode_destination(title, false, false)))
}

/// Detects an active `[[query` trigger at the end of `prefix`, returning `(token_bytes, query)`.
pub fn extract_wikilink_query(prefix: &str) -> Option<(usize, String)> {
    let open_pos = prefix.rfind("[[")?;
    let query = &prefix[open_pos + 2..];
    if query.len() > 80 || query.contains("]]") || query.contains('\n') || query.contains('\r') {
        return None;
    }
    Some((prefix.len() - open_pos, query.to_owned()))
}

fn normalize_lexical(base_dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = base_dir
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn relative_path(from_dir: &str, target_path: &str) -> String {
    let from_parts: Vec<&str> = from_dir.split('/').filter(|p| !p.is_empty()).collect();
    let target_parts: Vec<&str> = target_path.split('/').filter(|p| !p.is_empty()).collect();

    let mut common = 0usize;
    while common < from_parts.len()
        && common < target_parts.len()
        && path_eq(from_parts[common], target_parts[common])
    {
        common += 1;
    }

    let mut result = Vec::new();
    for _ in common..from_parts.len() {
        result.push("..");
    }
    for part in &target_parts[common..] {
        result.push(*part);
    }
    if result.is_empty() {
        "./".to_owned()
    } else {
        result.join("/")
    }
}

fn encode_destination(path: &str, angle_bracketed: bool, preserve_percent_encoding: bool) -> String {
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        match ch {
            '%' if !preserve_percent_encoding => out.push_str("%25"),
            ' ' if !angle_bracketed => out.push_str("%20"),
            '(' if !angle_bracketed => out.push_str("%28"),
            ')' if !angle_bracketed => out.push_str("%29"),
            '\t' => out.push_str("%09"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            '#' => out.push_str("%23"),
            '&' => out.push_str("%26"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\\' => out.push_str("%5C"),
            _ => out.push(ch),
        }
    }
    out
}

fn collect_located_links(source: &str) -> Vec<LocatedLink> {
    let Ok(root) = markdown::to_mdast(source, &markdown_parse_options()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_ast(&root, source, 0, &mut out);
    out
}

fn walk_ast(node: &Node, source: &str, quote_depth: usize, out: &mut Vec<LocatedLink>) {
    let quote_depth = quote_depth + usize::from(matches!(node, Node::Blockquote(_)));
    match node {
        Node::Code(_) | Node::InlineCode(_) | Node::Math(_) | Node::InlineMath(_) => return,
        Node::Link(link) => {
            if let Some(pos) = &link.position
                && let Some(span) = locate_inline_link_dest(source, pos.start.offset, pos.end.offset, &link.children, quote_depth)
            {
                let label = extract_plain_text(node);
                out.push(LocatedLink {
                    dest_start: span.start,
                    dest_end: span.end,
                    angle_bracketed: span.angle_bracketed,
                    url: link.url.clone(),
                    label,
                    line: pos.start.line,
                    kind: MarkdownLinkKind::Link,
                });
            }
            // Still walk children so `[![alt](img.png)](doc.md)` locates the inner image too.
            for child in &link.children {
                walk_ast(child, source, quote_depth, out);
            }
            return;
        }
        Node::Image(image) => {
            if let Some(pos) = &image.position
                && let Some(span) = locate_inline_image_dest(source, pos.start.offset, pos.end.offset, quote_depth)
            {
                out.push(LocatedLink {
                    dest_start: span.start,
                    dest_end: span.end,
                    angle_bracketed: span.angle_bracketed,
                    url: image.url.clone(),
                    label: image.alt.clone(),
                    line: pos.start.line,
                    kind: MarkdownLinkKind::Image,
                });
            }
            return;
        }
        Node::Definition(def) => {
            if let Some(pos) = &def.position
                && let Some(span) = locate_definition_dest(source, pos.start.offset, pos.end.offset, quote_depth)
            {
                let label = def.label.clone().unwrap_or_else(|| def.identifier.clone());
                out.push(LocatedLink {
                    dest_start: span.start,
                    dest_end: span.end,
                    angle_bracketed: span.angle_bracketed,
                    url: def.url.clone(),
                    label,
                    line: pos.start.line,
                    kind: MarkdownLinkKind::Definition,
                });
            }
            return;
        }
        _ => {}
    }

    if let Some(children) = node.children() {
        for child in children {
            walk_ast(child, source, quote_depth, out);
        }
    }
}

fn extract_plain_text(node: &Node) -> String {
    let mut buf = String::new();
    collect_plain_text(node, &mut buf);
    buf.trim().chars().take(80).collect()
}

fn collect_plain_text(node: &Node, out: &mut String) {
    match node {
        Node::Text(t) => out.push_str(&t.value),
        Node::InlineCode(c) => out.push_str(&c.value),
        Node::Image(img) => out.push_str(&img.alt),
        _ => {
            if let Some(children) = node.children() {
                for child in children {
                    collect_plain_text(child, out);
                }
            }
        }
    }
}

struct DestSpan {
    start: usize,
    end: usize,
    angle_bracketed: bool,
}

fn locate_inline_link_dest(
    source: &str,
    start: usize,
    end: usize,
    children: &[Node],
    quote_depth: usize,
) -> Option<DestSpan> {
    let bytes = source.as_bytes();
    if start >= end || end > bytes.len() || bytes[start] != b'[' {
        return None;
    }
    // Start scanning for the closing `]` after the last child node if present.
    let mut i = children
        .last()
        .and_then(|c| c.position())
        .map(|p| p.end.offset)
        .unwrap_or(start + 1)
        .max(start + 1);
    while i < end {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b']' {
            break;
        }
        i += 1;
    }
    if i + 1 >= end || bytes[i] != b']' || bytes[i + 1] != b'(' {
        return None;
    }
    parse_dest_span(source, i + 2, end, true, quote_depth)
}

fn locate_inline_image_dest(source: &str, start: usize, end: usize, quote_depth: usize) -> Option<DestSpan> {
    let bytes = source.as_bytes();
    if start + 2 >= end || end > bytes.len() || bytes[start] != b'!' || bytes[start + 1] != b'[' {
        return None;
    }
    let mut i = start + 2;
    let mut bracket_depth = 1usize;
    while i < end {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        match bytes[i] {
            b'[' => bracket_depth += 1,
            b']' => {
                bracket_depth -= 1;
                if bracket_depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if i + 1 >= end || bytes[i] != b']' || bytes[i + 1] != b'(' {
        return None;
    }
    parse_dest_span(source, i + 2, end, true, quote_depth)
}

fn locate_definition_dest(source: &str, start: usize, end: usize, quote_depth: usize) -> Option<DestSpan> {
    let bytes = source.as_bytes();
    if start >= end || end > bytes.len() {
        return None;
    }
    let mut i = start;
    while i < end && bytes[i] != b'[' {
        i += 1;
    }
    if i >= end {
        return None;
    }
    i += 1;
    while i < end {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b']' {
            break;
        }
        i += 1;
    }
    if i + 1 >= end || bytes[i] != b']' || bytes[i + 1] != b':' {
        return None;
    }
    parse_dest_span(source, i + 2, end, false, quote_depth)
}

fn parse_dest_span(
    source: &str,
    mut i: usize,
    end: usize,
    inside_parens: bool,
    quote_depth: usize,
) -> Option<DestSpan> {
    let bytes = source.as_bytes();
    let mut quote_prefix = 0;
    while i < end {
        match bytes[i] {
            b'\r' | b'\n' => quote_prefix = quote_depth,
            b' ' | b'\t' => {}
            // Continuation lines retain their container prefix in the original source.
            b'>' if quote_prefix > 0 => quote_prefix -= 1,
            _ => break,
        }
        i += 1;
    }
    if i >= end {
        return None;
    }
    if bytes[i] == b'<' {
        let dest_start = i + 1;
        let mut j = dest_start;
        while j < end {
            if bytes[j] == b'\\' {
                j += 2;
                continue;
            }
            if bytes[j] == b'>' {
                return Some(DestSpan {
                    start: dest_start,
                    end: j,
                    angle_bracketed: true,
                });
            }
            if bytes[j] == b'\n' || bytes[j] == b'\r' {
                return None;
            }
            j += 1;
        }
        return None;
    }

    let dest_start = i;
    let mut j = i;
    let mut paren_depth = 0usize;
    while j < end {
        let b = bytes[j];
        if b == b'\\' && j + 1 < end {
            j += 2;
            continue;
        }
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            break;
        }
        if inside_parens {
            if b == b'(' {
                paren_depth += 1;
            } else if b == b')' {
                if paren_depth == 0 {
                    break;
                }
                paren_depth -= 1;
            }
        }
        j += 1;
    }
    if j <= dest_start {
        return None;
    }
    Some(DestSpan {
        start: dest_start,
        end: j,
        angle_bracketed: false,
    })
}

// ============================================================================
// SQLite Derived Index (`markdown_links` & `markdown_link_files`)
// ============================================================================

pub fn metadata_db_path(project_root: Option<&Path>) -> std::io::Result<PathBuf> {
    crate::canvas::index::metadata_db_path(project_root)
}

const LINK_INDEX_VERSION: i64 = 1;
const TARGET_COLLATION: &str = if cfg!(windows) { "NOCASE" } else { "BINARY" };

fn open_links_db(db_path: &Path, create: bool) -> Result<Option<Connection>, String> {
    if !db_path.exists() && !create {
        return Ok(None);
    }
    if create && let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(db_path, flags).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(250))
        .map_err(|e| e.to_string())?;
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS markdown_links (
            source_path TEXT NOT NULL,
            target_path TEXT NOT NULL,
            anchor      TEXT NOT NULL DEFAULT '',
            label       TEXT NOT NULL DEFAULT '',
            line        INTEGER NOT NULL DEFAULT 1,
            link_kind   TEXT NOT NULL DEFAULT 'link',
            trash_id    TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_md_links_source ON markdown_links(source_path, trash_id);
        CREATE INDEX IF NOT EXISTS idx_md_links_target ON markdown_links(target_path COLLATE {TARGET_COLLATION}, trash_id);
        CREATE INDEX IF NOT EXISTS idx_md_links_trash ON markdown_links(trash_id);
        CREATE TABLE IF NOT EXISTS markdown_link_files (
            source_path TEXT PRIMARY KEY,
            mtime_ns    INTEGER NOT NULL,
            file_size   INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS markdown_link_index_version (version INTEGER PRIMARY KEY);",
    ))
    .map_err(|e| e.to_string())?;
    Ok(Some(conn))
}

fn file_stamp(full_path: &Path) -> Option<(i64, i64)> {
    let meta = fs::metadata(full_path).ok()?;
    let size = meta.len() as i64;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64);
    Some((mtime_ns, size))
}

/// Syncs links for a single saved Markdown file.
pub fn sync_markdown_links(
    db_path: &Path,
    source_path: &str,
    text: &str,
    disk_path: &Path,
) -> Result<(), String> {
    let normalized = normalize_slash(source_path);
    let links = extract_markdown_links(&normalized, text);
    let stamp = file_stamp(disk_path);
    let Some(mut conn) = open_links_db(db_path, !links.is_empty() || stamp.is_some())? else {
        return Ok(());
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    write_source_links(&tx, &normalized, &links, stamp)?;
    tx.commit().map_err(|e| e.to_string())
}

fn write_source_links(
    tx: &rusqlite::Transaction<'_>,
    source_path: &str,
    links: &[ExtractedMarkdownLink],
    stamp: Option<(i64, i64)>,
) -> Result<(), String> {
    tx.execute(
        "DELETE FROM markdown_links WHERE source_path = ?1 AND trash_id = ''",
        [source_path],
    )
    .map_err(|e| e.to_string())?;
    if !links.is_empty() {
        let mut stmt = tx
            .prepare(
                "INSERT INTO markdown_links (source_path, target_path, anchor, label, line, link_kind, trash_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, '')",
            )
            .map_err(|e| e.to_string())?;
        for link in links {
            stmt.execute(params![
                source_path,
                link.target_path,
                link.anchor.as_deref().unwrap_or(""),
                link.label,
                link.line as i64,
                link.kind.as_str(),
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    if let Some((mtime_ns, file_size)) = stamp {
        tx.execute(
            "INSERT INTO markdown_link_files (source_path, mtime_ns, file_size)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(source_path) DO UPDATE SET mtime_ns = excluded.mtime_ns, file_size = excluded.file_size",
            params![source_path, mtime_ns, file_size],
        )
        .map_err(|e| e.to_string())?;
    } else {
        tx.execute("DELETE FROM markdown_link_files WHERE source_path = ?1", [source_path])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Incrementally synchronizes all `.md` files in a project using `(mtime_ns, file_size)` stamps.
pub fn sync_project_markdown_links(
    db_path: &Path,
    project_root: &Path,
    md_rel_paths: &[String],
) -> Result<(), String> {
    if md_rel_paths.is_empty() && !db_path.exists() {
        return Ok(());
    }
    let Some(mut conn) = open_links_db(db_path, !md_rel_paths.is_empty())? else {
        return Ok(());
    };
    // Stamps from older extraction rules cannot validate the current derived rows.
    let version_matches = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM markdown_link_index_version WHERE version = ?1)",
        [LINK_INDEX_VERSION],
        |row| row.get::<_, bool>(0),
    ).map_err(|e| e.to_string())?;
    let cached: BTreeMap<String, (i64, i64)> = {
        let mut stmt = conn
            .prepare("SELECT source_path, mtime_ns, file_size FROM markdown_link_files")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>().map_err(|e| e.to_string())?
    };

    let current_set: BTreeSet<String> = md_rel_paths.iter().map(|p| normalize_slash(p)).collect();
    let mut stale: BTreeSet<_> = cached.keys()
        .filter(|path| !current_set.contains(*path)).cloned().collect();
    if !version_matches {
        // Early releases could write unstamped draft rows. Remove missing sources too.
        let mut stmt = conn.prepare("SELECT DISTINCT source_path FROM markdown_links WHERE trash_id = ''")
            .map_err(|e| e.to_string())?;
        let sources = stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for source in sources {
            let source = source.map_err(|e| e.to_string())?;
            if !current_set.contains(&source) {
                stale.insert(source);
            }
        }
    }

    let mut to_update = Vec::new();
    let mut read_error = None;
    for rel in &current_set {
        let full = project_root.join(rel);
        let Some(stamp) = file_stamp(&full) else {
            continue;
        };
        if version_matches && cached.get(rel) == Some(&stamp) {
            continue;
        }
        match fs::read(&full) {
            Ok(bytes) => to_update.push((
                rel,
                extract_markdown_links(rel, &String::from_utf8_lossy(&bytes)),
                stamp,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                read_error.get_or_insert_with(|| format!("{}: {error}", full.display()));
            }
        }
    }

    if stale.is_empty() && to_update.is_empty() && version_matches {
        return read_error.map_or(Ok(()), Err);
    }

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    for rel in &stale {
        tx.execute(
            "DELETE FROM markdown_links WHERE source_path = ?1 AND trash_id = ''",
            [rel],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM markdown_link_files WHERE source_path = ?1", [rel])
            .map_err(|e| e.to_string())?;
    }
    for (rel, links, stamp) in &to_update {
        write_source_links(&tx, rel, links, Some(*stamp))?;
    }
    if !version_matches && read_error.is_none() {
        tx.execute("DELETE FROM markdown_link_index_version", [])
            .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO markdown_link_index_version (version) VALUES (?1)", [LINK_INDEX_VERSION])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    read_error.map_or(Ok(()), Err)
}

pub fn remove_markdown_links(db_path: &Path, source_path: &str) -> Result<(), String> {
    let Some(mut conn) = open_links_db(db_path, false)? else {
        return Ok(());
    };
    let normalized = normalize_slash(source_path);
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM markdown_links WHERE source_path = ?1 AND trash_id = ''",
        [&normalized],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM markdown_link_files WHERE source_path = ?1",
        [&normalized],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn trash_markdown_links(db_path: &Path, source_path: &str, trash_id: &str) -> Result<(), String> {
    let Some(mut conn) = open_links_db(db_path, false)? else {
        return Ok(());
    };
    let normalized = normalize_slash(source_path);
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE markdown_links SET trash_id = ?1 WHERE source_path = ?2 AND trash_id = ''",
        params![trash_id, normalized],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM markdown_link_files WHERE source_path = ?1",
        [&normalized],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn restore_markdown_links(
    db_path: &Path,
    trash_id: &str,
    target_rel_path: &str,
    target_file: &Path,
) -> Result<(), String> {
    // Relative targets follow the restored file's actual directory and payload.
    let source = normalize_slash(target_rel_path);
    let stamp = file_stamp(target_file);
    let bytes = fs::read(target_file).map_err(|e| e.to_string())?;
    let links = extract_markdown_links(&source, &String::from_utf8_lossy(&bytes));
    let Some(mut conn) = open_links_db(db_path, true)? else {
        unreachable!("opening an index with create enabled returns a connection");
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM markdown_links WHERE trash_id = ?1", [trash_id])
        .map_err(|e| e.to_string())?;
    write_source_links(&tx, &source, &links, stamp)?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn purge_trashed_markdown_links(db_path: &Path, trash_id: &str) -> Result<(), String> {
    let Some(conn) = open_links_db(db_path, false)? else {
        return Ok(());
    };
    conn.execute("DELETE FROM markdown_links WHERE trash_id = ?1", [trash_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Returns all active Markdown files that reference any path affected by `moves`.
pub fn sources_referencing_moves(
    db_path: &Path,
    moves: &[(String, String)],
    is_folder: bool,
) -> Result<Vec<String>, String> {
    if moves.is_empty() {
        return Ok(Vec::new());
    }
    let Some(conn) = open_links_db(db_path, false)? else {
        return Ok(Vec::new());
    };
    let mut exact = conn.prepare(&format!(
        "SELECT DISTINCT source_path FROM markdown_links
         WHERE target_path = ?1 COLLATE {TARGET_COLLATION} AND trash_id = ''",
    )).map_err(|e| e.to_string())?;
    let mut descendants = conn.prepare(&format!(
        "SELECT DISTINCT source_path FROM markdown_links
         WHERE target_path COLLATE {TARGET_COLLATION} >= ?1
           AND target_path COLLATE {TARGET_COLLATION} < ?2 AND trash_id = ''",
    )).map_err(|e| e.to_string())?;
    let mut matched = BTreeSet::new();
    for (old, _) in moves {
        let old = normalize_slash(old);
        let rows = exact.query_map([&old], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            matched.insert(row.map_err(|e| e.to_string())?);
        }
        if is_folder {
            // '/' and '0' are adjacent, so the indexed range contains only this folder's children.
            let rows = descendants.query_map([format!("{old}/"), format!("{old}0")], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            for row in rows {
                matched.insert(row.map_err(|e| e.to_string())?);
            }
        }
    }
    Ok(matched.into_iter().collect())
}

/// Drops old source keys before the workspace re-indexes moved files at their new locations.
pub fn remove_relocated_sources(
    db_path: &Path,
    moves: &[(String, String)],
    is_folder: bool,
) -> Result<(), String> {
    if moves.is_empty() {
        return Ok(());
    }
    let Some(mut conn) = open_links_db(db_path, false)? else {
        return Ok(());
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;

    for (old, _) in moves {
        let old = normalize_slash(old);
        let lower = is_folder.then(|| format!("{old}/"));
        let upper = is_folder.then(|| format!("{old}0"));
        tx.execute(
            "DELETE FROM markdown_links WHERE trash_id = ''
             AND (source_path = ?1 OR (source_path >= ?2 AND source_path < ?3))",
            params![old, lower, upper],
        ).map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM markdown_link_files
             WHERE source_path = ?1 OR (source_path >= ?2 AND source_path < ?3)",
            params![old, lower, upper],
        ).map_err(|e| e.to_string())?;
    }

    tx.commit().map_err(|e| e.to_string())
}

/// Queries all active Markdown backlinks pointing to `target_path` (excluding self-links).
pub fn backlinks_for(db_path: &Path, target_path: &str) -> Result<Vec<MarkdownBacklink>, String> {
    let Some(conn) = open_links_db(db_path, false)? else {
        return Ok(Vec::new());
    };
    let normalized = normalize_slash(target_path);
    let query = format!(
        "SELECT source_path, target_path, anchor, label, line, link_kind
         FROM markdown_links
         WHERE target_path = ?1 COLLATE {TARGET_COLLATION} AND trash_id = ''
         ORDER BY source_path, line"
    );
    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let rows = stmt.query_map([&normalized], |row| {
        let anchor_raw: String = row.get(2)?;
        let kind_raw: String = row.get(5)?;
        Ok(MarkdownBacklink {
            source_path: row.get(0)?,
            target_path: row.get(1)?,
            anchor: (!anchor_raw.is_empty()).then_some(anchor_raw),
            label: row.get(3)?,
            line: row.get::<_, i64>(4)?.max(1) as usize,
            kind: MarkdownLinkKind::from_str(&kind_raw),
        })
    }).map_err(|e| e.to_string())?;
    let mut links = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
    links.retain(|link| !path_eq(&link.source_path, &normalized));
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_links_images_and_definitions_while_skipping_code_and_math() {
        let doc = r#"# Overview

See [Intro](./intro.md#背景) and ![Arch](../assets/arch%20v1.png "diagram").
Nested: [![Thumb](img/t.png)](guide.md).
Ref: [Spec][spec]

Inline code `[not](a.md)` and math $[x](a.md)$.

```markdown
[fenced](a.md)
![fenced-img](img/t.png)
```

$$
[block-math](a.md)
$$

External: [web](https://example.com/a.md), [mail](mailto:a@b.com), [local](#Overview).

[spec]: <../specs/my spec.md#section-1> "Specification"
"#;

        let links = extract_markdown_links("notes/overview.md", doc);
        let targets: Vec<(&str, Option<&str>, MarkdownLinkKind)> = links
            .iter()
            .map(|l| (l.target_path.as_str(), l.anchor.as_deref(), l.kind))
            .collect();
        assert_eq!(
            targets,
            vec![
                ("notes/intro.md", Some("背景"), MarkdownLinkKind::Link),
                ("assets/arch v1.png", None, MarkdownLinkKind::Image),
                ("notes/guide.md", None, MarkdownLinkKind::Link),
                ("notes/img/t.png", None, MarkdownLinkKind::Image),
                ("specs/my spec.md", Some("section-1"), MarkdownLinkKind::Definition),
            ]
        );
    }

    #[test]
    fn rewrites_inbound_links_preserving_anchors_titles_and_styles() {
        let doc = concat!(
            "Read [Intro](./intro.md#快速%20开始 \"title\") and [Root](/notes/intro.md#sec).\n",
            "Also ![Pic](<../img/my pic.png>) and [ref][r].\n\n",
            "[r]: ./intro.md\n",
        );

        let moves = vec![
            ("notes/intro.md".to_owned(), "docs/getting started.md".to_owned()),
            ("img/my pic.png".to_owned(), "assets/pic v2.png".to_owned()),
        ];
        let (updated, count) =
            rewrite_markdown_links(doc, "notes/index.md", "notes/index.md", &moves, false)
                .expect("should rewrite");

        assert_eq!(count, 4);
        assert_eq!(
            updated,
            concat!(
                "Read [Intro](../docs/getting%20started.md#快速%20开始 \"title\") and [Root](/docs/getting%20started.md#sec).\n",
                "Also ![Pic](<../assets/pic v2.png>) and [ref][r].\n\n",
                "[r]: ../docs/getting%20started.md\n",
            )
        );
    }

    #[test]
    fn rewrites_outbound_links_when_source_moves_across_directories() {
        let doc = concat!(
            "Sibling [B](b.md), dot [C](./c.md), up ![Logo](../assets/logo.png), root [R](/README.md).\n",
            "[Folder](deep), [Project root](..).\n",
        );
        let moves = vec![("notes/a.md".to_owned(), "notes/deep/a.md".to_owned())];
        let (updated, count) =
            rewrite_markdown_links(doc, "notes/a.md", "notes/deep/a.md", &moves, false)
                .expect("should rewrite outbound relative paths");

        assert_eq!(count, 5);
        assert_eq!(
            updated,
            concat!(
                "Sibling [B](../b.md), dot [C](../c.md), up ![Logo](../../assets/logo.png), root [R](/README.md).\n",
                "[Folder](./), [Project root](../..).\n"
            )
        );
    }

    #[test]
    fn folder_move_keeps_internal_sibling_links_unchanged_and_updates_external_links() {
        let doc = "Internal [B](b.md) and external [Root](../README.md) and root-rel [Self](/docs/b.md).\n";
        let moves = vec![("docs".to_owned(), "archive/docs".to_owned())];
        let (updated, count) = rewrite_markdown_links(
            doc,
            "docs/a.md",
            "archive/docs/a.md",
            &moves,
            true,
        )
        .expect("should update external and root-relative links");

        assert_eq!(count, 2);
        assert_eq!(
            updated,
            "Internal [B](b.md) and external [Root](../../README.md) and root-rel [Self](/archive/docs/b.md).\n"
        );

        let only_internal = "Only [B](b.md) and ![Img](./img.png).\n";
        assert!(
            rewrite_markdown_links(
                only_internal,
                "docs/a.md",
                "archive/docs/a.md",
                &moves,
                true,
            )
            .is_none()
        );
    }

    #[test]
    fn multi_file_move_keeps_co_moved_sibling_links_intact() {
        let doc = "Co-moved [B](b.md) and stayed [C](c.md).\n";
        let moves = vec![
            ("a.md".to_owned(), "sub/a.md".to_owned()),
            ("b.md".to_owned(), "sub/b.md".to_owned()),
        ];
        let (updated, count) =
            rewrite_markdown_links(doc, "a.md", "sub/a.md", &moves, false).unwrap();
        assert_eq!(count, 1);
        assert_eq!(updated, "Co-moved [B](b.md) and stayed [C](../c.md).\n");
    }

    #[test]
    fn rewrites_quoted_link_destinations_without_changing_container_prefix() {
        let doc = "> > [A](\r\n> > <a&amp;b.md#100%25>)\r\n";
        let moves = [("a&b.md".to_owned(), "100%20#完成.md".to_owned())];
        let (updated, count) = rewrite_markdown_links(doc, "index.md", "index.md", &moves, false)
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(updated, "> > [A](\r\n> > <100%2520%23完成.md#100%25>)\r\n");
        let links = extract_markdown_links("index.md", &updated);
        assert_eq!(links[0].target_path, "100%20#完成.md");
        assert_eq!(links[0].anchor.as_deref(), Some("100%25"));
    }

    #[test]
    fn sqlite_index_sync_backlinks_relocate_and_trash_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let db = root.join(".folio").join("metadata.sqlite3");

        fs::create_dir_all(root.join("notes")).unwrap();
        let a_path = root.join("notes/a.md");
        let b_path = root.join("notes/b.md");
        fs::write(&a_path, "# A\nSee [B](b.md#结论) and [Self](a.md).\n").unwrap();
        fs::write(&b_path, "# B\n").unwrap();

        // Simulate a legacy index that stamped an unsaved draft as the current file.
        sync_markdown_links(&db, "notes/a.md", "[Draft](draft.md)", &a_path).unwrap();
        sync_project_markdown_links(
            &db,
            root,
            &["notes/a.md".to_owned(), "notes/b.md".to_owned()],
        )
        .unwrap();
        assert!(backlinks_for(&db, "notes/draft.md").unwrap().is_empty());

        let backlinks = backlinks_for(&db, "notes/b.md").unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].source_path, "notes/a.md");
        assert_eq!(backlinks[0].anchor.as_deref(), Some("结论"));
        assert_eq!(backlinks[0].label, "B");
        assert_eq!(backlinks[0].line, 2);

        // Self-links are excluded from backlinks.
        assert!(backlinks_for(&db, "notes/a.md").unwrap().is_empty());

        // Finding sources referencing a moved target.
        let affected = sources_referencing_moves(
            &db,
            &[("notes/b.md".to_owned(), "notes/b-new.md".to_owned())],
            false,
        )
        .unwrap();
        assert_eq!(affected, vec!["notes/a.md".to_owned()]);

        // Trash hides links; restore brings them back.
        trash_markdown_links(&db, "notes/a.md", "trash-1").unwrap();
        assert!(backlinks_for(&db, "notes/b.md").unwrap().is_empty());

        fs::create_dir_all(root.join("docs")).unwrap();
        let restored = root.join("docs/a-restored.md");
        fs::copy(&a_path, &restored).unwrap();
        restore_markdown_links(&db, "trash-1", "docs/a-restored.md", &restored).unwrap();
        assert!(backlinks_for(&db, "notes/b.md").unwrap().is_empty());
        let after_restore = backlinks_for(&db, "docs/b.md").unwrap();
        assert_eq!(after_restore.len(), 1);
        assert_eq!(after_restore[0].source_path, "docs/a-restored.md");

        // Renaming the source to a non-Markdown file removes its outgoing links.
        remove_relocated_sources(
            &db,
            &[("docs/a-restored.md".to_owned(), "docs/a.txt".to_owned())],
            false,
        )
        .unwrap();
        assert!(backlinks_for(&db, "docs/b.md").unwrap().is_empty());

        // Trash + purge permanently removes records.
        sync_markdown_links(&db, "notes/a.md", "[B](b.md)", &a_path).unwrap();
        trash_markdown_links(&db, "notes/a.md", "trash-2").unwrap();
        purge_trashed_markdown_links(&db, "trash-2").unwrap();
        assert!(backlinks_for(&db, "notes/b.md").unwrap().is_empty());
    }

    #[test]
    fn wikilink_query_and_url_formatting() {
        assert_eq!(
            extract_wikilink_query("参考[[设计文档#架构"),
            Some(("[[设计文档#架构".len(), "设计文档#架构".to_owned()))
        );
        assert_eq!(extract_wikilink_query("已完成[[a]]后续"), None);
        assert_eq!(
            format_relative_link_url("notes/sub/a.md", "docs/my guide.md", Some("快速开始")),
            "../../docs/my%20guide.md#快速开始"
        );
        assert_eq!(
            format_heading_fragment("完成 100%20"),
            Some("#完成%20100%2520".to_owned())
        );
    }

    #[test]
    fn completion_roundtrips_a_literal_file_name_and_heading_in_one_undo() {
        let mut editor = markdown_editor::BlockEditorModel::from_markdown(
            "参考 [[引导",
            gpui_kit::base::text::MarkdownExtensions::default(),
        )
        .unwrap();
        editor.select_end().unwrap();
        let mut capture = editor.capture_suffix("[[引导".len()).unwrap();
        let url = format_relative_link_url("notes/a.md", "docs/100%20#完成.md", Some("完成 50%"));
        editor.replace_captured_with_link(&mut capture, "引导", &url).unwrap();
        let links = extract_markdown_links("notes/a.md", &editor.source());
        assert_eq!(links[0].target_path, "docs/100%20#完成.md");
        assert_eq!(files::percent_decode(links[0].anchor.as_deref().unwrap()), "完成 50%");
        assert!(editor.undo());
        assert_eq!(editor.caret_prefix(), Some("参考 [[引导"));
    }
}
