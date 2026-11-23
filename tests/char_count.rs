//! Editor badges show whitespace-free character counts, so the counting rules
//! and the buffer cache can be checked without starting a GPU window.
#![allow(dead_code)]
#[path = "../src/i18n/mod.rs"]
mod i18n;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/theme.rs"]
mod theme;

use model::{DiskDocument, DocumentBuffer, nonspace_char_count};

#[test]
fn whitespace_is_excluded_from_the_character_count() {
    assert_eq!(nonspace_char_count(""), 0);
    assert_eq!(nonspace_char_count("  \t\n\r\n "), 0);
    assert_eq!(nonspace_char_count("abc"), 3);
    // Markdown and LaTeX syntax characters are document text, so they count.
    assert_eq!(nonspace_char_count("# 标题 **加粗**"), 9);
    assert_eq!(nonspace_char_count("\\section{一}"), 11);
}

#[test]
fn ideographic_and_no_break_spaces_are_whitespace() {
    // U+3000 and U+00A0 are what CJK documents normally receive.
    assert_eq!(nonspace_char_count("一二\u{3000}三\u{a0}四"), 4);
}

#[test]
fn the_buffer_keeps_its_count_current() {
    let mut buffer = DocumentBuffer::from_disk(DiskDocument {
        text: "第一行\n".to_owned(),
        revision: "r1".to_owned(),
    });
    assert_eq!(buffer.char_count, 3);

    buffer.set_text("第一行 第二行\n".to_owned());
    assert_eq!(buffer.char_count, 6);

    buffer.replace_from_disk(DiskDocument {
        text: "x y".to_owned(),
        revision: "r2".to_owned(),
    });
    assert_eq!(buffer.char_count, 2);
}
