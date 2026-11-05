//! The line arithmetic, with no grammar involved: these hold at every
//! feature set, so they keep `LineIndex` honest in a build with no grammar.

use super::{LineIndex, Pos, Span};

/// A grammar may report a range past the text it was given. This crate does
/// not vouch for a third-party grammar's arithmetic, so the position is
/// clamped into the source rather than reported as-is.
#[test]
fn a_range_past_the_source_is_clamped_rather_than_trusted() {
    let source = "fn broken( {";
    let index = LineIndex::new(source);
    let span = index.span(usize::MAX..usize::MAX);
    assert!(span.start.byte <= source.len(), "start clamped");
    assert!(span.end.byte <= source.len(), "end clamped");
}

#[test]
fn an_offset_inside_a_character_is_floored_to_its_first_byte() {
    let source = "\u{e9}\u{e9}\nb";
    let index = LineIndex::new(source);
    let span = index.span(1..3);
    assert_eq!(span.start.byte, 0, "inside the first character");
    assert_eq!(span.start.column, 1, "columns are 1-based");
    assert_eq!(span.end.byte, 2, "inside the second character");
    assert_eq!(span.end.column, 2, "the second character's column");
}

#[test]
fn a_range_entirely_past_the_source_resolves_to_the_last_line() {
    let source = "one\ntwo\nthree\n";
    let index = LineIndex::new(source);
    let span = index.span(9_000..9_001);
    assert_eq!(span.start.byte, source.len());
    assert_eq!(
        span.start.line, 4,
        "the trailing newline opens a fourth line"
    );
}

#[test]
fn the_first_byte_of_the_file_is_line_one_column_one_byte_zero() {
    let source = "alpha\nbeta\n";
    let index = LineIndex::new(source);
    assert_eq!(
        index.position(0),
        Pos {
            line: 1,
            column: 1,
            byte: 0
        },
        "1-based line and column over a 0-based byte offset"
    );
}

#[test]
fn a_byte_on_a_line_start_belongs_to_that_line_not_the_previous_one() {
    let source = "alpha\nbeta\n";
    let index = LineIndex::new(source);
    // Byte 6 is the `b` of `beta`, the first byte of the second line.
    assert_eq!(index.position(6).line, 2);
    assert_eq!(index.position(6).column, 1);
}

#[test]
fn a_multi_byte_line_reports_characters_in_the_column_and_bytes_in_the_offset() {
    let source = "éé\n";
    let index = LineIndex::new(source);
    // Two two-byte characters, so byte 4 is the end of the line and is two
    // characters past its start.
    let pos = index.position(4);
    assert_eq!(pos.column, 3, "four bytes is two characters, one-based");
    assert_eq!(pos.byte, 4, "the byte offset is the byte offset");
}

#[test]
fn an_empty_source_resolves_to_the_only_position_it_has() {
    let index = LineIndex::new("");
    let span = index.span(0..0);
    assert_eq!(span.start.byte, 0);
    assert_eq!(span.start.line, 1);
    assert!(span.is_empty());
}

#[test]
fn a_span_covers_exactly_the_bytes_between_its_ends() {
    let source = "alpha\nbeta\ngamma\n";
    let index = LineIndex::new(source);
    let span = index.span(6..10);
    assert_eq!(&source[span.byte_range()], "beta");
    assert_eq!(span.start.line, 2);
    assert_eq!(span.end.line, 2);
}

#[test]
fn a_zero_width_span_is_empty_and_keeps_its_position() {
    let source = "alpha\nbeta\n";
    let span = Span::at(index_position(source, 6));
    assert!(span.is_empty());
    assert_eq!(span.start, span.end);
    assert_eq!(span.start.line, 2);
}

/// The position of `byte` in `source`, through the same index the span
/// helpers use.
fn index_position(source: &str, byte: usize) -> Pos {
    LineIndex::new(source).position(byte)
}
