//! Which lines of a source file are test code, for the layer scan (#1096).
//!
//! The scan reads shipped code only. The file is parsed by lgwks_ast's Rust
//! grammar (`try_parse`), so the tree decides where an item and its attributes
//! begin and end: no string, comment or brace can move a boundary. An item is
//! test code when its attribute run holds `#[test]`, or a `#[cfg(..)]` that no
//! build without `test` satisfies (the Rust Reference, "Conditional
//! compilation"): `cfg(test)`, `cfg(all(test, ..))`, or an `any` whose every
//! branch needs `test`. `any(test, ..)` and `not(test)` compile outside tests,
//! so their items are read. A predicate the reader cannot parse, or one past its
//! bounds, is read as shipped, and a file the parser refuses or times out on is
//! read whole: the scan reads more, never less. The scan reads a shipped view of
//! the file: the bytes of each test item and comment are blanked, newlines kept,
//! so the code beside them on a line is still read.
//!
//! The predicate evaluator is the second copy of `lgwks_ast::scan`'s private
//! cfg rule; #1100 exposes that rule and removes this one.

use std::ops::Range;
use std::time::Duration;

use lgwks_ast::{Language, ParseError, try_parse_within};

use super::layer_edges::upward_refusals;
use super::{catches_a_panic, edges};

/// The most distinct options other than `test` a predicate may name before it
/// is read as shipped: 2^12 assignments bound the work on a hostile predicate.
const MAX_OPTIONS: usize = 12;

/// The deepest nesting of `all`, `any` and `not` a predicate may hold before it
/// is read as shipped, so a hostile predicate cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

/// The most nodes a predicate may hold before it is read as shipped. The
/// evaluation is assignments times nodes, so 2^12 times 256 bounds it.
const MAX_NODES: usize = 256;

/// How long one file's parse may run. The shipped sources parse in
/// milliseconds; the bound only stops a hostile file, and a timeout is named
/// as its own edge, never read as a syntax refusal.
const PARSE_DEADLINE: Duration = Duration::from_secs(60);

/// One parsed configuration predicate.
enum Cfg {
    /// The `test` option.
    Test,
    /// Any other option or `key = "value"` pair, by its spelling.
    Named(String),
    /// Every branch holds; true when empty.
    All(Vec<Cfg>),
    /// Some branch holds; false when empty.
    Any(Vec<Cfg>),
    /// The branch does not hold.
    Not(Box<Cfg>),
}

impl Cfg {
    /// Every distinct named option, in first-seen order, until one past
    /// [`MAX_OPTIONS`]: past that the predicate is read as shipped, so the
    /// linear check below never runs over more than 13 names.
    fn named<'a>(&'a self, into: &mut Vec<&'a str>) {
        if into.len() > MAX_OPTIONS {
            return;
        }
        match *self {
            Self::Test => {}
            Self::Named(ref name) => {
                if !into.contains(&name.as_str()) {
                    into.push(name);
                }
            }
            Self::All(ref items) | Self::Any(ref items) => {
                items.iter().for_each(|item| item.named(into));
            }
            Self::Not(ref item) => item.named(into),
        }
    }

    /// Whether the predicate holds with `test` off and option `i` of `named`
    /// set when bit `i` of `bits` is.
    fn holds(&self, named: &[&str], bits: u32) -> bool {
        match *self {
            Self::Test => false,
            Self::Named(ref name) => named
                .iter()
                .position(|known| known == name)
                .and_then(|at| u32::try_from(at).ok())
                .and_then(|shift| bits.checked_shr(shift))
                .is_some_and(|value| (value & 1) == 1),
            Self::All(ref items) => items.iter().all(|item| item.holds(named, bits)),
            Self::Any(ref items) => items.iter().any(|item| item.holds(named, bits)),
            Self::Not(ref item) => !item.holds(named, bits),
        }
    }
}

/// One token of a predicate.
#[derive(Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Text(String),
    Equals,
    Open,
    Close,
    Comma,
}

/// The tokens of `predicate`, or `None` for a character no predicate holds or a
/// string literal that never closes.
fn tokens(predicate: &str) -> Option<Vec<Token>> {
    let mut found = Vec::new();
    let mut chars = predicate.chars().peekable();
    while let Some(next) = chars.next() {
        match next {
            '(' => found.push(Token::Open),
            ')' => found.push(Token::Close),
            ',' => found.push(Token::Comma),
            '=' => found.push(Token::Equals),
            '"' => {
                let mut text = String::new();
                let mut closed = false;
                let mut escaped = false;
                for inner in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if inner == '\\' {
                        escaped = true;
                    } else if inner == '"' {
                        closed = true;
                        break;
                    }
                    text.push(inner);
                }
                if !closed {
                    return None;
                }
                found.push(Token::Text(text));
            }
            space if space.is_whitespace() => {}
            letter if letter.is_alphanumeric() || letter == '_' => {
                let mut word = String::from(letter);
                while let Some(more) = chars.next_if(|more| more.is_alphanumeric() || *more == '_')
                {
                    word.push(more);
                }
                found.push(Token::Word(word));
            }
            _ => return None,
        }
    }
    Some(found)
}

/// A recursive-descent reader over the tokens of one predicate.
struct Reader {
    tokens: Vec<Token>,
    at: usize,
    /// The predicate nodes read so far, against [`MAX_NODES`].
    nodes: usize,
}

impl Reader {
    /// The next token, consumed.
    fn take(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at = self.at.saturating_add(1);
        token
    }

    /// The next token, left in place.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// One predicate at nesting `depth`.
    fn predicate(&mut self, depth: usize) -> Option<Cfg> {
        self.nodes = self.nodes.saturating_add(1);
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return None;
        }
        let Some(Token::Word(word)) = self.take() else {
            return None;
        };
        let combinator = matches!(word.as_str(), "all" | "any" | "not");
        match self.peek() {
            Some(&Token::Open) if combinator => {
                self.at = self.at.saturating_add(1);
                let mut items = Vec::new();
                while self.peek() != Some(&Token::Close) {
                    items.push(self.predicate(depth.saturating_add(1))?);
                    match self.peek() {
                        Some(&Token::Comma) => self.at = self.at.saturating_add(1),
                        Some(&Token::Close) => {}
                        _ => return None,
                    }
                }
                self.at = self.at.saturating_add(1);
                match word.as_str() {
                    "all" => Some(Cfg::All(items)),
                    "any" => Some(Cfg::Any(items)),
                    _ => {
                        let [only] = <[Cfg; 1]>::try_from(items).ok()?;
                        Some(Cfg::Not(Box::new(only)))
                    }
                }
            }
            Some(&Token::Equals) => {
                self.at = self.at.saturating_add(1);
                let Some(Token::Text(value)) = self.take() else {
                    return None;
                };
                Some(Cfg::Named(format!("{word}=\"{value}\"")))
            }
            _ if combinator => None,
            _ if word == "test" => Some(Cfg::Test),
            _ => Some(Cfg::Named(word)),
        }
    }
}

/// The predicate `text` parses to, when all of it is one predicate.
fn parse(text: &str) -> Option<Cfg> {
    let mut reader = Reader {
        tokens: tokens(text)?,
        at: 0,
        nodes: 0,
    };
    let cfg = reader.predicate(0)?;
    (reader.at == reader.tokens.len()).then_some(cfg)
}

/// Whether no build without `test` satisfies the cfg predicate `text`.
pub(super) fn requires_test(text: &str) -> bool {
    let Some(cfg) = parse(text) else {
        return false;
    };
    let mut named = Vec::new();
    cfg.named(&mut named);
    if named.len() > MAX_OPTIONS {
        return false;
    }
    let Some(assignments) = u32::try_from(named.len())
        .ok()
        .and_then(|width| 1_u32.checked_shl(width))
    else {
        return false;
    };
    !(0..assignments).any(|bits| cfg.holds(&named, bits))
}

/// Whether one attribute's text, `#[..]` as the grammar spans it, marks its item
/// as test code: `#[test]`, or a `cfg` whose predicate needs `test`.
fn marks_test(attribute: &str) -> bool {
    let Some(inner) = attribute
        .trim()
        .strip_prefix("#[")
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    let inner = inner.trim();
    if inner == "test" {
        return true;
    }
    let Some(arguments) = inner.strip_prefix("cfg") else {
        return false;
    };
    let Some(predicate) = arguments
        .trim_start()
        .strip_prefix('(')
        .and_then(|rest| rest.trim_end().strip_suffix(')'))
    else {
        return false;
    };
    // The Reference's grammar is `cfg ( ConfigurationPredicate ,? )`: one
    // trailing comma after the predicate is the same predicate.
    let predicate = predicate.trim_end();
    match predicate.strip_suffix(',') {
        Some(bare) => requires_test(bare),
        None => requires_test(predicate),
    }
}

/// The byte spans of one parsed file that are not shipped code.
struct Spans {
    /// Each test item, from its first test attribute to the item's end.
    test: Vec<Range<usize>>,
    /// Each line or block comment, doc comments included.
    comments: Vec<Range<usize>>,
}

/// The test and comment spans of `text`, from the grammar's tree. The parse is
/// bounded by `deadline`; a refusal or a timeout is the parser's typed error.
fn spans_within(text: &str, deadline: Duration) -> Result<Spans, ParseError> {
    let parsed = try_parse_within(text, Language::Rust, deadline)?;
    let mut spans = Spans {
        test: Vec::new(),
        comments: Vec::new(),
    };
    let mut pending = vec![parsed.root()];
    while let Some(node) = pending.pop() {
        // The start of the first test attribute in the run before the next item.
        let mut run: Option<usize> = None;
        for child in node.children() {
            let kind = child.kind();
            match kind.as_ref() {
                "attribute_item" => {
                    if run.is_none() && marks_test(&child.text()) {
                        run = Some(child.range().start);
                    }
                }
                "line_comment" | "block_comment" => spans.comments.push(child.range()),
                _ => match run.take() {
                    Some(start) => spans.test.push(start..child.range().end),
                    None => pending.push(child),
                },
            }
        }
    }
    Ok(spans)
}

/// One entry per line of `text`, as `str::lines` splits it: whether the line
/// holds test code. The grammar's refusal when `text` is not Rust.
pub(super) fn test_lines(text: &str) -> Result<Vec<bool>, ParseError> {
    let spans = spans_within(text, PARSE_DEADLINE)?;
    let newlines: Vec<usize> = text.match_indices('\n').map(|(at, _)| at).collect();
    let line_of = |byte: usize| newlines.partition_point(|newline| *newline < byte);
    let mut tested = vec![false; text.lines().count()];
    for span in &spans.test {
        let first = line_of(span.start);
        let last = line_of(span.end.saturating_sub(1));
        tested
            .iter_mut()
            .skip(first)
            .take(last.saturating_sub(first).saturating_add(1))
            .for_each(|line| *line = true);
    }
    Ok(tested)
}

/// `text` with every byte of `spans` but a newline turned to a space, so each
/// line keeps its number and the code beside a span on its line is still read.
fn blank(text: &str, spans: &Spans) -> String {
    let mut hidden = vec![false; text.len()];
    for span in spans.test.iter().chain(&spans.comments) {
        if let Some(bytes) = hidden.get_mut(span.clone()) {
            bytes.fill(true);
        }
    }
    text.char_indices()
        .map(|(at, ch)| {
            if ch != '\n' && hidden.get(at) == Some(&true) {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// How the scan reads one file: the shipped view it reads, and the parser's
/// refusal when it could not parse the file, in which case the view is the
/// whole file, comments and all.
pub(super) struct Reading {
    /// The file with test items and comments blanked, line for line.
    pub(super) shipped: String,
    /// The parser's refusal of the file, which the scan names as an edge.
    pub(super) refused: Option<ParseError>,
}

/// How the scan reads `text`.
pub(super) fn read(text: &str) -> Reading {
    read_within(text, PARSE_DEADLINE)
}

/// [`read`] under a parse deadline the caller names.
pub(super) fn read_within(text: &str, deadline: Duration) -> Reading {
    match spans_within(text, deadline) {
        Ok(spans) => Reading {
            shipped: blank(text, &spans),
            refused: None,
        },
        Err(refusal) => Reading {
            shipped: text.to_owned(),
            refused: Some(refusal),
        },
    }
}

/// The edge the scan names for a file the parser refused: a timeout says the
/// file was too costly to parse, any other refusal that it is not Rust.
pub(super) fn refusal_edge(relative: &str, refusal: &ParseError) -> String {
    match *refusal {
        ParseError::TimedOut { after, .. } => format!(
            "{relative}: the Rust parse ran past its {after:?} deadline; every line is read"
        ),
        _ => {
            format!("{relative}: the Rust grammar refused the file ({refusal}); every line is read")
        }
    }
}

#[test]
/// The scan blanks bytes, not lines: shipped code on the same line as a test
/// item, after a `//` inside a string, or beside a comment is read, and a
/// commented-out import is not code.
fn shipped_code_beside_a_test_item_or_in_a_string_is_read() {
    let before = "use crate::script::Scope; #[cfg(test)] mod tests {}\n";
    let after = "#[cfg(test)] mod tests { use crate::task::Hidden; } use crate::script::Scope;\n";
    for text in [before, after] {
        assert_eq!(
            upward_refusals("rt/sync.rs", text),
            vec![String::from("rt/sync.rs:1: rt module names crate::script")],
            "the import beside the test item is read: {text}"
        );
    }
    let slashes = "const U: &str = \"//\"; use crate::script::Scope;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", slashes),
        vec![String::from("rt/sync.rs:1: rt module names crate::script")],
    );
    let commented =
        "/* use crate::script::Old; */ use crate::task::New;\n// use crate::script::Old;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", commented),
        vec![String::from("rt/sync.rs:1: rt module names crate::task")],
    );
    let wide = "fn f() -> &'static str { \"é\" } #[cfg(test)] mod t { const C: char = 'ü'; }\nuse crate::task::Wide;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", wide),
        vec![String::from("rt/sync.rs:2: rt module names crate::task")],
        "multi-byte text keeps every line in place"
    );
}

#[test]
/// A parse past its deadline is named as a timeout, not as a syntax refusal,
/// and the file is read whole.
fn a_parse_past_its_deadline_is_named_and_read_whole() -> Result<(), String> {
    let depth = 4_000;
    let nested = format!(
        "use crate::task::Host;\nfn f() -> u8 {{ {}1{} }}\n",
        "(".repeat(depth),
        ")".repeat(depth)
    );
    let reading = read_within(&nested, Duration::ZERO);
    let refusal = reading
        .refused
        .as_ref()
        .ok_or_else(|| String::from("a zero deadline stops a nested parse"))?;
    assert!(
        matches!(*refusal, ParseError::TimedOut { .. }),
        "the refusal is the timeout: {refusal}"
    );
    assert!(
        refusal_edge("rt/sync.rs", refusal).contains("ran past its"),
        "a timeout is named as one"
    );
    assert_eq!(reading.shipped, nested, "a refused file is read whole");
    Ok(())
}

#[test]
/// A hostile predicate costs bounded work: past 12 options or 256 nodes it is
/// read as shipped, so its item is read.
fn a_hostile_predicate_is_read_as_shipped_within_its_bounds() {
    let options: Vec<String> = (0..12).map(|at| format!("feature = \"f{at}\"")).collect();
    let negations = vec!["not(test)"; 300].join(", ");
    let wide = format!("all(test, {}, any({negations}))", options.join(", "));
    assert!(
        !requires_test(&wide),
        "past the node bound reads as shipped"
    );
    let many: Vec<String> = (0..20_000)
        .map(|at| format!("feature = \"g{at}\""))
        .collect();
    assert!(
        !requires_test(&format!("all(test, {})", many.join(", "))),
        "past the option bound reads as shipped"
    );
    let within = format!("all(test, {})", options.join(", "));
    assert!(requires_test(&within), "12 options and test stay in bounds");
}

#[test]
/// #1096: a module under a predicate that no build without `test` satisfies is
/// test code, so the test imports of `rt/runtime.rs` are not shipped edges.
fn a_predicate_that_needs_test_is_test_code() -> Result<(), ParseError> {
    for predicate in [
        r#"all(test, feature = "script")"#,
        "test",
        " test ",
        "all(feature = \"script\", test)",
        "all(unix, all(test, not(windows)))",
        "any(test, all(test, unix))",
        "not(not(test))",
        "all(test, any())",
    ] {
        assert!(requires_test(predicate), "{predicate} needs test");
    }
    let runtime = "fn drive() {}\n#[cfg(all(test, feature = \"script\"))]\nmod one_driver_tests {\n    use crate::spec::Bot;\n}\n";
    assert!(upward_refusals("rt/runtime.rs", runtime).is_empty());
    assert_eq!(test_lines(runtime)?, vec![false, true, true, true, true]);
    let harness = "#[test]\nfn check() { crate::script::run(); }\nuse crate::script::Shown;\n";
    assert_eq!(
        upward_refusals("rt/runtime.rs", harness),
        vec![String::from(
            "rt/runtime.rs:3: rt module names crate::script"
        )],
        "a #[test] item exists only in test builds"
    );
    Ok(())
}

#[test]
/// A module that compiles outside tests ships, so its import is refused; a
/// predicate the reader cannot parse is read as shipped, never skipped.
fn a_module_that_compiles_outside_tests_is_read() {
    for predicate in [
        r#"any(test, feature = "x")"#,
        "not(test)",
        "unix",
        "all(unix, any(test, windows))",
        "test = \"x\"",
        "all()",
        "all(test",
        "test)",
        "",
        "all(test,,unix)",
        "feature = script",
        "not(test, unix)",
        "tset",
        "all(test) unix",
    ] {
        assert!(
            !requires_test(predicate),
            "{predicate} compiles outside tests"
        );
    }
    for attribute in ["#[cfg(any(test, feature = \"x\"))]", "#[cfg(not(test))]"] {
        let text = format!("{attribute}\nmod helpers {{\n    use crate::script::Scope;\n}}\n");
        assert_eq!(
            upward_refusals("rt/tenancy.rs", &text),
            vec![String::from(
                "rt/tenancy.rs:3: rt module names crate::script"
            )],
            "{attribute}"
        );
    }
}

#[test]
/// The scan skips only the test module: a shipped import, a driver name and a
/// panic catch after it are still read, whatever braces the module hides.
fn code_after_a_test_module_is_still_read() {
    let quoted =
        "const DOC: &str = r#\"\n#[cfg(test)]\nmod fake {\n\"#;\nuse crate::script::Hidden;\n";
    let commented = "/*\n#[cfg(test)]\nmod fake {\n*/\nuse crate::script::Hidden;\n";
    for text in [quoted, commented] {
        assert_eq!(
            upward_refusals("rt/sync.rs", text),
            vec![String::from("rt/sync.rs:5: rt module names crate::script")],
            "quoted or commented text opens no test item: {text}"
        );
    }
    let c_raw = "const C: &core::ffi::CStr = cr\"\\\";\n#[cfg(test)]\nmod tests {}\nuse crate::script::Scope;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", c_raw),
        vec![String::from("rt/sync.rs:4: rt module names crate::script")],
    );
    let shipped = "use crate::script::Scope;";
    let module = "fn run() {}\n#[cfg(test)]\nmod tests {\n    fn brace() -> char { '}' }\n    const OPEN: &str = \"{{\";\n    // }\n    /* } { */\n    const RAW: &str = r#\"}\"#;\n    mod inner {\n        fn quote() -> char { '\\'' }\n    }\n}\n";
    let text = &format!("{module}{shipped}\n");
    assert_eq!(
        upward_refusals("rt/sync.rs", text),
        vec![String::from("rt/sync.rs:13: rt module names crate::script")],
    );
    assert_eq!(
        edges(
            "clock.rs",
            "#[cfg(test)]\nmod tests {}\nuse crate::rt::time;\n"
        ),
        vec![String::from("clock.rs:3: core module names crate::rt")],
    );
    assert!(catches_a_panic(
        "#[cfg(test)]\nmod tests {\n}\nfn run() { let _ = std::panic::catch_unwind(work); }\n"
    ));
    let spread = "#[cfg(test)]\n#[allow(dead_code)]\n// the tests\npub(crate) mod tests\n{\n    use crate::script::Hidden;\n}\nuse crate::task::Host;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", spread),
        vec![String::from("rt/sync.rs:8: rt module names crate::task")],
        "attributes and comments between the cfg and the module"
    );
}

#[test]
/// A declared test module is its own lines only. A test module the file never
/// closes is not Rust: the grammar refuses the file, the scan names that
/// refusal, and every line of the file is read. Nothing panics.
fn an_open_or_declared_test_module_ends_where_rust_ends_it() -> Result<(), ParseError> {
    let declared = "#[cfg(test)]\nmod helpers;\nuse crate::script::Scope;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", declared),
        vec![String::from("rt/sync.rs:3: rt module names crate::script")],
    );
    assert_eq!(test_lines(declared)?, vec![true, true, false]);
    let pathed = "#[cfg(test)]\n#[path = \"x.rs\"]\npub mod helpers;\nuse crate::script::Scope;\n";
    assert_eq!(upward_refusals("rt/sync.rs", pathed).len(), 1);
    let truncated =
        "use crate::task::Host;\n#[cfg(test)]\nmod tests {\n    use crate::script::Scope;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", truncated),
        vec![
            String::from("rt/sync.rs:1: rt module names crate::task"),
            String::from("rt/sync.rs:4: rt module names crate::script"),
        ],
        "a file the grammar refuses is read whole"
    );
    assert!(test_lines(truncated).is_err());
    let named = edges("rt/sync.rs", truncated);
    assert!(
        named
            .iter()
            .any(|edge| edge.starts_with("rt/sync.rs: the Rust grammar refused the file")),
        "the refusal is named: {named:?}"
    );
    assert!(test_lines("")?.is_empty());
    assert_eq!(
        test_lines("#[cfg(test)]\nfn helper() {}\nuse crate::rt::time;\n")?,
        vec![true, true, false],
        "any item under a test cfg is test code, not only a module"
    );
    let gated_use = "#[cfg(test)]\nuse crate::script::Hidden;\nuse crate::script::Shown;\n";
    assert_eq!(
        upward_refusals("rt/sync.rs", gated_use),
        vec![String::from("rt/sync.rs:3: rt module names crate::script")],
    );
    Ok(())
}

#[test]
/// CRLF line endings give the same lines, the same mask and the same edges.
fn crlf_lines_keep_their_verdict() -> Result<(), ParseError> {
    let lf = "use crate::task::Host;\n#[cfg(test)]\nmod tests {\n    use crate::script::Hidden;\n}\nuse crate::script::Scope;\n";
    let crlf = lf.replace('\n', "\r\n");
    assert!(crlf.contains("\r\n"));
    assert_eq!(test_lines(&crlf)?, test_lines(lf)?);
    assert_eq!(test_lines(lf)?, vec![false, true, true, true, true, false]);
    assert_eq!(
        upward_refusals("rt/sync.rs", &crlf),
        vec![
            String::from("rt/sync.rs:1: rt module names crate::task"),
            String::from("rt/sync.rs:6: rt module names crate::script"),
        ],
    );
    Ok(())
}

#[test]
/// A 10,000 line file with a test module between two halves: both halves are
/// read, and the mask covers exactly the module.
fn a_large_file_is_read_in_one_pass() -> Result<(), ParseError> {
    let total = 10_000_usize;
    let half = 5_000_usize;
    let mut lines: Vec<String> = Vec::with_capacity(total);
    for at in 0..total {
        let line = match at {
            10 => String::from("use crate::script::Early;"),
            at if at == half => String::from("#[cfg(test)]"),
            at if at == half.saturating_add(1) => String::from("mod tests {"),
            at if at == half.saturating_add(2) => String::from("    use crate::script::Hidden;"),
            at if at == half.saturating_add(3) => String::from("}"),
            at if at == total.saturating_sub(10) => String::from("use crate::script::Late;"),
            _ => String::from("const FILLER: u8 = { 1 };"),
        };
        lines.push(line);
    }
    let text = lines.join("\n");
    assert_eq!(
        upward_refusals("rt/sync.rs", &text),
        vec![
            String::from("rt/sync.rs:11: rt module names crate::script"),
            format!(
                "rt/sync.rs:{}: rt module names crate::script",
                total.saturating_sub(9)
            ),
        ],
    );
    let mask = test_lines(&text)?;
    assert_eq!(mask.len(), total);
    assert_eq!(mask.iter().filter(|tested| **tested).count(), 4);
    Ok(())
}
