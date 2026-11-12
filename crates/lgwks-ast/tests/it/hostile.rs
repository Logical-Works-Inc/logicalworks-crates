//! Hostile and malformed input: every answer is a typed refusal, or it is not
//! an answer (#277).
//!
//! Two bodies of evidence. The first is the whole fixture table the crate
//! already declares, plus adversarial generators that produce the shapes a
//! hostile PR actually carries -- nested delimiters with and without their
//! closers, one line of megabytes, a source at the byte ceiling, invalid UTF-8
//! boundaries, and each grammar's own nesting fragment taken to a depth past
//! [`MAX_AST_DEPTH`]. Every one of them must come back as a [`ParseError`].
//! Not "no panic" as a matter of luck: each generator is paired with the shape
//! of answer it is supposed to get, so a refusal that arrives for the wrong
//! reason fails.
//!
//! The second body is the markdown guard. tree-sitter's markdown external
//! scanner serializes its open block containers into a fixed 1 024-byte buffer
//! and asserts when they do not fit, and an assertion in a C parser is
//! `abort()`: the process dies, the caller with it, and `try_parse` returns
//! nothing at all. `"- "` repeated 255 times — 510 bytes — does it. The crate
//! cannot patch the scanner, but it can refuse the source before the scanner
//! sees it, and `MAX_MARKDOWN_CONTAINERS_PER_LINE` is that refusal. The
//! seventeen shapes, their measured abort depths and the thousand-seed proof are
//! in the section at the foot of this file.
//!
//! # What the guard does not cover
//!
//! It runs on the **checked** parse. `parse` and `parse_with` return a `Parsed`
//! rather than a `Result`, so a refusal has nowhere to go in them and this change
//! moved neither signature; the unchecked path carries the exposure and says so
//! on its own documentation. A caller accepting untrusted input uses
//! [`try_parse`].
//!
//! # Why a hard per-test timeout
//!
//! A test that hangs is a test that hides a defect, and the shapes here are the
//! ones that would hang. `.config/nextest.toml` gives every test in this module
//! a `slow-timeout` and a `terminate-after`, so a walk that stops making
//! progress fails the run instead of holding it open.

#![cfg(feature = "lang-rust")]

use std::error::Error;

#[cfg(feature = "lang-md")]
use std::io::Write;
#[cfg(feature = "lang-md")]
use std::time::Duration;

#[cfg(feature = "lang-md")]
use crate::seed::Rng;
use lgwks_ast::{
    Language, MAX_AST_DEPTH, MAX_AST_NODES, MAX_SOURCE_BYTES, ParseError, inspect_ast, parse,
    try_parse, try_parse_within,
};

/// What every family returns.
type TestResult = Result<(), Box<dyn Error>>;

/// One shape of hostile input, and what it is supposed to earn.
struct Adversarial {
    /// What the generator is called in an assertion message.
    name: &'static str,
    /// The source, or `None` when this build cannot build the shape's grammar.
    source: Option<String>,
    /// Whether the source is the grammar's own valid source.
    ///
    /// Not "must be accepted": a full-ceiling tiling of a grammar that emits
    /// eight nodes per twelve source bytes is past [`MAX_AST_NODES`], and being
    /// refused for that is the bound working. What a valid source must never be
    /// refused for is *syntax* — that would mean the tiling broke the grammar's
    /// own input rather than testing a bound.
    valid: bool,
}

/// The nesting fragment each grammar is fed, where it has one.
///
/// `None` where the grammar is not compiled in this build, or where the
/// generator has no delimiter to nest. The table is the one the measurement
/// example uses, kept here deliberately separate: that one describes shapes to
/// time, this one describes shapes to refuse, and a shape allowed to be invalid
/// there is not the same thing as one allowed to be invalid here.
fn nesting_fragment(language: Language) -> Option<(&'static str, &'static str)> {
    match language.name() {
        "bash" | "c" | "cpp" | "csharp" | "css" | "dart" | "hcl" | "java" | "kotlin" | "nix"
        | "scala" | "solidity" | "swift" => Some(("{", "}")),
        "go" => Some(("{", "}")),
        "typescript" | "tsx" | "javascript" | "python" | "ruby" | "php" | "lua" | "haskell"
        | "rust" => Some(("(", ")")),
        "json" => Some(("[", "]")),
        "html" => Some(("<div>", "</div>")),
        "elixir" => Some(("defmodule A do\n", "end\n")),
        "markdown" => Some(("- ", "")),
        "yaml" => Some(("  ", "")),
        _ => None,
    }
}

/// The nesting fragment, nested past [`MAX_AST_DEPTH`] and paired.
fn deeply_nested(language: Language) -> Option<String> {
    let (open, close) = nesting_fragment(language)?;
    if close.is_empty() {
        // An indentation-run grammar closes by dedent, which this generator
        // cannot express without also producing valid source; the paired case
        // below covers it.
        return None;
    }
    let levels = MAX_AST_DEPTH.saturating_mul(8);
    Some(format!(
        "{}{}{}",
        open.repeat(levels),
        "x",
        close.repeat(levels)
    ))
}

/// The nesting fragment with no closers at all.
fn unbalanced(language: Language) -> Option<String> {
    let (open, _) = nesting_fragment(language)?;
    Some(open.repeat(MAX_AST_DEPTH.saturating_mul(8)))
}

/// Every adversarial shape this build can construct for `language`.
///
/// Four families: the grammar's own source at the byte ceiling, one line of
/// megabytes, a nesting run past the depth ceiling with its closers, and the
/// same run without them. The last two are the shapes the depth bound exists
/// for; the first two are the shapes a byte bound exists for.
fn adversarial_shapes(language: Language) -> Vec<Adversarial> {
    let mut shapes = Vec::new();
    let Some(valid) = grammar_sample(language) else {
        return shapes;
    };
    let one_line = "x".repeat(64);
    shapes.push(Adversarial {
        name: "tiled-to-the-byte-ceiling",
        source: Some(tile_to(valid, MAX_SOURCE_BYTES)),
        valid: true,
    });
    shapes.push(Adversarial {
        name: "one-line-of-megabytes",
        source: Some(tile_to(&one_line, MAX_SOURCE_BYTES)),
        valid: false,
    });
    if let Some(nested) = deeply_nested(language) {
        shapes.push(Adversarial {
            name: "nested-past-the-depth-ceiling",
            source: Some(nested),
            valid: false,
        });
    }
    if let Some(open) = unbalanced(language) {
        shapes.push(Adversarial {
            name: "unbalanced-openers",
            source: Some(open),
            valid: false,
        });
    }
    shapes
}

/// `fragment` repeated into `bytes`, in whole copies and never past it.
///
/// Whole copies only, and never a byte over: a shape that overshot the ceiling
/// would be refused by the byte bound and would measure nothing about the
/// grammar, and truncating mid-copy could split a multi-byte character. A
/// fragment wider than the budget yields an empty string rather than a partial
/// one, which no sample in this table is.
fn tile_to(fragment: &str, bytes: usize) -> String {
    let mut source = String::with_capacity(bytes);
    while source.len().saturating_add(fragment.len()) <= bytes {
        source.push_str(fragment);
    }
    source
}

/// Source this build knows `language` accepts, or `None` where the build has no
/// fixture for it.
///
/// The crate's own `FIXTURES` table is `#[cfg(test)]` inside the library and is
/// not reachable from an integration binary, so the one sample each shape needs
/// is written here. Where a grammar's sample is not known, no shape is built:
/// inventing source for a grammar nobody here can validate would make the test
/// assert something about the wrong thing.
fn grammar_sample(language: Language) -> Option<&'static str> {
    Some(match language.name() {
        "bash" => "echo hello\n",
        "c" => "int main(void) { return 0; }\n",
        "cpp" => "int main() { return 0; }\n",
        "csharp" => "class A { }\n",
        "css" => "a { color: red; }\n",
        "dart" => "void main() {}\n",
        "elixir" => "defmodule A do\n  def f, do: 1\nend\n",
        "go" => "package main\n\nfunc main() {}\n",
        "haskell" => "main = putStrLn \"hi\"\n",
        "hcl" => "resource \"a\" \"b\" {\n}\n",
        "html" => "<!DOCTYPE html>\n<html><body><p>hi</p></body></html>\n",
        "java" => "class A {}\n",
        "javascript" => "const a = 1;\n",
        "json" => "{\"a\": 1}\n",
        "kotlin" => "fun main() {}\n",
        "lua" => "local a = 1\n",
        "nix" => "{ pkgs }: pkgs.hello\n",
        "php" => "<?php echo \"hi\";\n",
        "python" => "def f():\n    return 1\n",
        "ruby" => "def f\n  1\nend\n",
        "rust" => "fn main() {}\n",
        "scala" => "object A { def f = 1 }\n",
        "solidity" => "contract A {}\n",
        "swift" => "func f() {}\n",
        "tsx" | "typescript" => "const a: number = 1;\n",
        "yaml" => "a: 1\n",
        _ => return None,
    })
}

/// The name of a refusal, for an assertion that reads as the variant itself.
fn refusal_name(error: &ParseError) -> String {
    format!("{error}")
}

// ── Typed refusals, in process ──────────────────────────────────────────────

#[test]
/// Every adversarial shape of every compiled grammar answers with a `ParseError`
/// or a tree, and never with a panic, a hang or a silent empty answer.
fn every_adversarial_shape_answers_typed() -> TestResult {
    let mut checked = 0_usize;
    let mut tilings_refused_for_syntax = 0_usize;
    let mut tilings_accepted = 0_usize;
    for &language in Language::ALL {
        for shape in adversarial_shapes(language) {
            let Some(source) = shape.source.as_deref() else {
                continue;
            };
            // The byte-ceiling tiling asks whether a 2 MiB source *can* be
            // parsed, which is a question about the grammar and the bounds,
            // not about how fast the host parses. On the host clock's 10 s
            // deadline a loaded CI runner answered it with timeouts (run
            // 37651284179: 1 accepted, 6 "refused for syntax"), and a timeout
            // counted as a syntax refusal. `Duration::MAX` is a deadline past
            // what the clock can represent, which never arrives, so the tiling
            // runs to completion however loaded the host is while the byte,
            // node, depth and markdown guards stay enforced. Every other shape
            // keeps the production deadline: their timeouts are meaningful
            // typed answers, asserted only as typed, never counted.
            let answer = if shape.name == "tiled-to-the-byte-ceiling" {
                try_parse_within(source, language, std::time::Duration::MAX)
            } else {
                try_parse(source, language)
            };
            match answer {
                Ok(tree) => {
                    // An adversarial *shape* is not the same thing as invalid
                    // source: two megabytes of `x` is one enormous valid Bash
                    // command, and the one-line shape is adversarial because of
                    // its length rather than its content. What must hold either
                    // way is that a tree this crate handed back is inside the
                    // bounds it claims.
                    if shape.valid {
                        tilings_accepted = tilings_accepted.saturating_add(1);
                    }
                    let metrics = inspect_ast(&tree.root(), None);
                    assert!(
                        metrics.nodes <= MAX_AST_NODES,
                        "{} {}: a checked parse returned a tree of {} nodes",
                        language.name(),
                        shape.name,
                        metrics.nodes
                    );
                    assert!(
                        !metrics.has_syntax_issues,
                        "{} {}: a checked parse returned a tree carrying a recovery node",
                        language.name(),
                        shape.name
                    );
                }
                Err(error) => {
                    // The refusal must name itself: a caller reporting findings
                    // cannot use a blank message.
                    assert!(
                        !refusal_name(&error).is_empty(),
                        "{} {}: the refusal rendered as nothing",
                        language.name(),
                        shape.name
                    );
                    if shape.valid {
                        // Tiling is only sound for a grammar whose unit of source
                        // concatenates: three Nix expressions in one file are
                        // three expressions and a syntax error, which says
                        // nothing about a bound. Counted rather than asserted,
                        // so the assertion at the end can read how many grammars
                        // tile cleanly rather than this one guessing. A timeout
                        // here is not a syntax refusal at all: the tiling runs
                        // under a deadline that never arrives, so one means the
                        // clock contract broke rather than the grammar, and it
                        // fails distinctly instead of joining the count.
                        assert!(
                            !matches!(error, ParseError::TimedOut { .. }),
                            "{} {}: the byte-ceiling tiling timed out under a deadline that never \
                             arrives; the clock contract broke, not the grammar",
                            language.name(),
                            shape.name
                        );
                        tilings_refused_for_syntax = tilings_refused_for_syntax.saturating_add(1);
                    }
                }
            }
            checked = checked.saturating_add(1);
        }
    }
    assert!(
        checked >= 4 * 3,
        "only {checked} shapes were checked; the default build compiles 7 grammars \
         and four shapes each"
    );
    // The byte-ceiling shape must be able to succeed, or the family would only
    // ever be measuring refusals and a bound that refused everything would pass.
    // Tiling is not sound for every grammar, so this is a count over the
    // grammars whose unit of source concatenates, not over all of them.
    assert!(
        tilings_accepted >= 3,
        "only {tilings_accepted} grammars accepted a full-ceiling tiling of their own valid \
         source, and {tilings_refused_for_syntax} refused it for syntax; the byte-ceiling shape \
         is not proving that a 2 MiB source can be parsed"
    );
    Ok(())
}

#[test]
/// A source at the byte ceiling is refused *before* the parser runs, so the
/// refusal names the byte bound and not a tree it built first.
fn the_byte_ceiling_is_refused_before_any_tree_exists() -> TestResult {
    let over = "x".repeat(MAX_SOURCE_BYTES.saturating_add(1));
    let refusal = match try_parse(&over, Language::Rust) {
        Err(refusal) => refusal,
        Ok(_) => return Err("a source one byte over the ceiling was accepted".into()),
    };
    match refusal {
        ParseError::SourceTooLarge { actual, limit } => {
            assert_eq!(
                limit, MAX_SOURCE_BYTES,
                "the refusal names the applied bound"
            );
            assert_eq!(actual, over.len(), "and the observed length");
        }
        other => {
            return Err(format!("expected SourceTooLarge, got {}", refusal_name(&other)).into());
        }
    }
    // A source of exactly the ceiling is inside the byte bound, so what refuses
    // it next is a recovery node rather than the byte check. That ordering is
    // the claim: the byte check runs first, and only for a source past it.
    let at = "x".repeat(MAX_SOURCE_BYTES);
    assert!(
        matches!(
            try_parse(&at, Language::Rust),
            Err(ParseError::InvalidSyntax { .. })
        ),
        "a source exactly at the ceiling is past the byte bound but not the node bound"
    );
    Ok(())
}

#[test]
/// Every refusal renders as a located diagnostic rather than as text, including
/// the ones the byte, depth and deadline bounds produce.
fn every_refusal_renders_as_a_located_diagnostic() -> TestResult {
    let source = "fn main() {}\n";
    let refusals = [
        ParseError::SourceTooLarge {
            actual: MAX_SOURCE_BYTES.saturating_add(1),
            limit: MAX_SOURCE_BYTES,
        },
        ParseError::ParserUnavailable {
            language: "rust",
            detail: "injected".to_owned(),
        },
        ParseError::AstTooLarge {
            language: "rust",
            observed: 3,
            limit: 2,
        },
        ParseError::AstTooDeep {
            language: "rust",
            observed: MAX_AST_DEPTH.saturating_add(1),
            limit: MAX_AST_DEPTH,
        },
        ParseError::TimedOut {
            language: "rust",
            after: lgwks_ast::DEFAULT_PARSE_DEADLINE,
        },
    ];
    for refusal in refusals {
        let reported = refusal.to_diagnostic("hostile.rs", source);
        assert!(
            !reported.render().is_empty(),
            "{refusal} rendered as an empty report"
        );
        assert!(
            reported.span().start.byte <= source.len(),
            "{refusal} rendered a span past the end of its own source"
        );
    }
    Ok(())
}

#[test]
/// An unchecked parse of the same hostile shapes does not hang or panic, which
/// is what `parse` exists for: inspecting a malformed tree on purpose.
fn the_unchecked_parse_of_the_same_shapes_still_answers() -> TestResult {
    for source in [
        "(".repeat(MAX_AST_DEPTH.saturating_mul(8)),
        ")".repeat(MAX_AST_DEPTH.saturating_mul(8)),
        format!("{}{}", "(".repeat(4_096), ")".repeat(4_096)),
        "\u{fffd}".repeat(1_024),
        "\u{1f600}".repeat(1_024),
    ] {
        let tree = parse(&source, Language::Rust);
        let metrics = inspect_ast(&tree.root(), None);
        assert!(
            metrics.nodes >= 1,
            "a parse of {} bytes reported {} nodes",
            source.len(),
            metrics.nodes
        );
        // The depth of a hostile tree is a measurement, never a refusal: this is
        // the unchecked walk, and capping it would make the number a constant.
        assert!(
            metrics.max_depth >= 1,
            "the root counts as depth 1 for a {} byte source",
            source.len()
        );
    }
    Ok(())
}

#[test]
/// Multi-byte and truncated UTF-8 boundaries are answered, not panicked on.
fn multibyte_and_truncated_sources_are_answered() -> TestResult {
    for source in [
        // A multibyte character cut in half is not a `&str`; the truncated form
        // is the one a caller reading bytes can actually hand over.
        String::from_utf8_lossy(&[0xf0, 0x9f, 0x98]).into_owned(),
        String::from_utf8_lossy(&[0x80]).into_owned(),
        String::from("fn f() { /* \u{e9}\u{65e5}\u{672c} */ }\n"),
        format!("{}\u{2028}{}", "fn f() {}".repeat(64), "//".repeat(1_000)),
    ] {
        // The only assertion available in process: whatever came back is a
        // `Result` the caller can match on, never a panic on the way to it.
        if let Err(error) = try_parse(&source, Language::Rust) {
            assert!(
                !refusal_name(&error).is_empty(),
                "a refusal rendered as nothing"
            );
        }
    }
    Ok(())
}

// ── The markdown guard ──────────────────────────────────────────────────────
//
// Compiled only where the markdown grammar is. Every case below is a case about
// *that* grammar's external scanner, and a build without it has no scanner to
// overflow; the sibling test outside this module asserts that rather than
// letting the coverage read as wider than it is.
#[cfg(feature = "lang-md")]
mod markdown;
