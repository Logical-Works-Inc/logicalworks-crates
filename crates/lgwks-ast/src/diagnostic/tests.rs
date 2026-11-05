//! Unit tests of the diagnostic spans, severities and recovery walk.

use super::*;
use crate::{Language, parse};

/// A file whose second function is malformed, so the recovery is confined to
/// a known line and the byte offset is predictable.
const BROKEN: &str = "fn main() { let x = 1; }\nfn broken( {\n";

/// A wide, shallow tree: many top-level functions, each only a few nodes
/// deep. This is the shape that separates a depth-proportional walk from a
/// sibling-frontier one.
fn wide_source(count: usize) -> String {
    (0..count)
        .map(|index| format!("fn f{index}() {{}}\n"))
        .collect::<String>()
}

#[test]
fn a_clean_tree_reports_nothing() {
    let source = "fn main() {}\n";
    let tree = parse(source, Language::Rust);
    assert_eq!(
        diagnostics("ok.rs", &tree.root(), source, "Rust"),
        Vec::new(),
        "a clean tree has nothing to report"
    );
    assert_eq!(recovery_count(&tree.root()), 0);
}

#[test]
fn a_recovery_is_reported_on_the_line_that_carries_it() {
    let tree = parse(BROKEN, Language::Rust);
    let found = diagnostics("bad.rs", &tree.root(), BROKEN, "Rust");
    assert_eq!(found.len(), 1, "one malformed function reports once");
    let first = &found[0];
    assert_eq!(
        first.span().start.line,
        2,
        "the recovery is on the second line, not the clean first one"
    );
    // The byte range is a real slice of the source, and it is exactly the
    // malformed text: not an approximation of it, and not an offset past it.
    let range = first.span().byte_range();
    assert!(
        range.start <= range.end && range.end <= BROKEN.len(),
        "span {range:?} is not inside a source of {} bytes",
        BROKEN.len()
    );
    assert_eq!(
        &BROKEN[range], "fn broken( {",
        "the span covers the malformed text and nothing else"
    );
}

#[test]
fn a_recovery_is_never_reported_as_an_error() {
    let tree = parse(BROKEN, Language::Rust);
    let found = diagnostics("bad.rs", &tree.root(), BROKEN, "Rust");
    assert!(
        found.iter().all(|one| one.severity() == Severity::Warning),
        "a recovered parse is a warning; only a refusal is an error"
    );
    assert_eq!(recovery_count(&tree.root()), found.len());
}

#[test]
fn findings_come_back_in_source_order() {
    let source = "fn a( {\nfn b( {\nfn c( {\n";
    let tree = parse(source, Language::Rust);
    let found = diagnostics("many.rs", &tree.root(), source, "Rust");
    assert!(found.len() > 1, "three broken functions report separately");
    let starts: Vec<usize> = found.iter().map(|one| one.span().start.byte).collect();
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(starts, sorted, "findings are not in source order");
}

#[test]
fn lines_and_columns_are_one_based_and_bytes_are_zero_based() {
    let tree = parse(BROKEN, Language::Rust);
    let found = diagnostics("p.rs", &tree.root(), BROKEN, "Rust");
    let first = &found[0];
    assert!(
        first.span().start.line >= 1,
        "a report reading line 0 is a defect in the report"
    );
    assert!(
        first.span().start.column >= 1,
        "a report reading column 0 is a defect in the report"
    );
    // The 1-based convention is what makes the second line report 2. A
    // 0-based reading of the same byte offset would say 1, so this is the
    // assertion that pins the convention rather than the arithmetic.
    assert_eq!(
        first.span().start.line,
        BROKEN[..first.span().start.byte]
            .matches('\n')
            .count()
            .saturating_add(1),
        "the reported line is one more than the newlines before it"
    );
}

#[test]
fn a_column_counts_characters_and_not_bytes() {
    // `é` is two bytes and one character. A report pointing after it must
    // not claim the reader typed two characters more than they did.
    let source = "// é\nfn broken( {\n";
    let tree = parse(source, Language::Rust);
    let found = diagnostics("u.rs", &tree.root(), source, "Rust");
    let first = &found[0];
    let line_start = match source[..first.span().start.byte].rfind('\n') {
        Some(at) => at.saturating_add(1),
        None => 0,
    };
    let column_chars = source[line_start..first.span().start.byte].chars().count();
    assert_eq!(
        first.span().start.column,
        column_chars.saturating_add(1),
        "column is 1-based and counted in characters"
    );
    // The byte offset is genuinely further along than the character count,
    // which is the whole reason this distinction exists.
    assert!(
        first.span().start.byte > first.span().start.column,
        "the multi-byte line should separate byte and character offsets"
    );
}

#[test]
fn an_end_of_file_position_is_the_last_line_and_zero_width() {
    let source = "fn ok() {}\n";
    let span = end_of(source);
    assert!(span.is_empty(), "a whole-file condition has no width");
    assert_eq!(span.start.byte, source.len());
    assert_eq!(
        span.start.line, 2,
        "the trailing newline opens the second line"
    );
}

#[test]
fn an_unlabeled_file_renders_as_a_placeholder_and_not_a_bare_colon() {
    let tree = parse(BROKEN, Language::Rust);
    let found = diagnostics("", &tree.root(), BROKEN, "Rust");
    let rendered = found[0].render();
    assert!(
        rendered.starts_with("<source>:"),
        "an empty path rendered as {rendered}, which reads as a truncated filename"
    );
}

#[test]
fn the_render_matches_the_documented_shape() {
    let tree = parse(BROKEN, Language::Rust);
    let rendered = diagnostics("src/a.rs", &tree.root(), BROKEN, "Rust")[0].render();
    assert!(rendered.starts_with("src/a.rs:"), "{rendered}");
    assert!(
        rendered.contains(": W: "),
        "a warning renders as ` W: `: {rendered}"
    );
    assert!(
        !rendered.ends_with('\n'),
        "render carries no trailing newline"
    );
}

#[test]
fn a_message_names_the_grammar_it_came_from() {
    let tree = parse(BROKEN, Language::Rust);
    let found = diagnostics("m.rs", &tree.root(), BROKEN, "Rust");
    assert!(
        found[0].message().contains("Rust"),
        "the message names the grammar: {}",
        found[0].message()
    );
}

#[test]
fn the_walk_retains_depth_and_not_the_sibling_frontier() {
    // The resource half of INV-AST-3, held by this walk: a wide tree must
    // not accumulate one retained entry per sibling, or a file with N
    // top-level items costs O(N) resident memory.
    let source = wide_source(4_096);
    let tree = parse(&source, Language::Rust);
    assert_eq!(
        tree.root().children().len(),
        4_096,
        "the fixture presents 4,096 immediate siblings to the walk"
    );
    let peak = peak_retained_frames(&tree.root());
    assert!(
        peak < 64,
        "descended {peak} levels for 4,096 wide siblings; that is fan-out, not depth"
    );
}

#[test]
fn a_deep_narrow_tree_still_descends_one_level_per_ancestor() {
    // The other half of the same invariant: a deep tree does cost depth,
    // and the cost is the active depth rather than the total node count.
    let depth = 200;
    let source = format!(
        "{}fn f() {{}}{}",
        "fn f() {".repeat(depth),
        "}".repeat(depth)
    );
    let tree = parse(&source, Language::Rust);
    let peak = peak_retained_frames(&tree.root());
    assert!(
        peak > 1,
        "a {depth}-deep tree descended only {peak} levels; the walk is not descending"
    );
    let nodes = crate::inspect_ast(&tree.root(), None).nodes;
    assert!(
        peak < nodes,
        "descended {peak} levels for a tree of {nodes} nodes; that is not \
         depth-proportional"
    );
}
