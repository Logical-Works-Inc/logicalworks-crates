//! Seeded deterministic replay of the #154 glob matcher contracts.
//!
//! One seed drives every pattern and path, a failure prints the seed, and the
//! same seed must produce the same trace hash. Matching results are compared
//! against an independent brute-force reference so the seeded sweep checks
//! semantics, not just self-consistency.
//!
//! The reference is deliberately naive: it enumerates every way the pattern's
//! tokens can consume the path, in both the strict and legacy dialects, and
//! computes the rolling reachability row for the whole path at each step. It is
//! `O(tokens * N²)` and shares no code with the shipped per-token scan, so a
//! match that both agree on is a real agreement.

use lgwks_std::glob::{GlobDialect, GlobPattern, GlobScratch, PatternError, matches};

use crate::seeded_sweep;

use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, fold, fold_usize,
    initial_trace, next_seed,
};

/// A table position, as the `usize` these sweeps index their own tables with.
///
/// Every table here holds a handful of entries, so a position's low four bytes
/// are the position; reading them is total where a checked conversion would carry
/// a refusal arm for a bound the table's own length states.
fn table_index(position: u64) -> usize {
    let bytes = position.to_le_bytes();
    usize::from(bytes[0]) | (usize::from(bytes[1]) << 8)
}

/// A table's length as the `u64` a seeded remainder is taken over.
///
/// The mirror of [`table_index`]: a length's low eight bytes are the length, so
/// the remainder below a table's own length is the same draw either way.
fn draw_bound(length: usize) -> u64 {
    let bytes = length.to_le_bytes();
    u64::from(bytes[0]) | (u64::from(bytes[1]) << 8)
}

/// The reference's token set, mirroring the documented dialect without sharing
/// the shipped code.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RefToken {
    /// One exact scalar.
    Literal(char),
    /// Exactly one non-separator scalar.
    Question,
    /// Zero or more non-separator scalars.
    Star,
    /// Zero or more scalars, including separators.
    DoubleStar,
    /// A class: inclusive ranges plus a negation flag.
    Class {
        /// Sorted inclusive ranges.
        ranges: Vec<(char, char)>,
        /// Whether a scalar outside the ranges is accepted.
        negated: bool,
    },
}

/// Tokenizes a pattern under the reference's own rules.
fn reference_tokens(pattern: &str, dialect: GlobDialect) -> Vec<RefToken> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while let Some(&current) = chars.get(index) {
        match current {
            '*' if chars.get(index.saturating_add(1)) == Some(&'*') => {
                let mut run = index;
                while chars.get(run) == Some(&'*') {
                    run = run.saturating_add(1);
                }
                if dialect == GlobDialect::Strict
                    && (run.saturating_sub(index) != 2
                        || (index != 0 && chars.get(index.saturating_sub(1)) != Some(&'/'))
                        || (run != chars.len() && chars.get(run) != Some(&'/')))
                {
                    return Vec::new();
                }
                tokens.push(RefToken::DoubleStar);
                index = run;
            }
            '*' => {
                tokens.push(RefToken::Star);
                index = index.saturating_add(1);
            }
            '?' => {
                tokens.push(RefToken::Question);
                index = index.saturating_add(1);
            }
            '[' => {
                let mut body = index.saturating_add(1);
                let mut negated = false;
                if matches!(chars.get(body), Some('!' | '^')) {
                    negated = true;
                    body = body.saturating_add(1);
                }
                let mut close = body;
                if chars.get(close) == Some(&']') {
                    close = close.saturating_add(1);
                }
                while matches!(chars.get(close), Some(&member) if member != ']') {
                    close = close.saturating_add(1);
                }
                if close >= chars.len() {
                    if dialect == GlobDialect::Strict {
                        return Vec::new();
                    }
                    tokens.push(RefToken::Literal('['));
                    index = index.saturating_add(1);
                    continue;
                }
                let mut ranges = Vec::new();
                // `close` was found at or after `body` inside `chars`, so the
                // span exists; an empty answer reads as a refusal and fails the
                // comparison against the shipped compiler loudly.
                let Some(inner) = chars.get(body..close) else {
                    return Vec::new();
                };
                let mut at = 0;
                while let Some(&member) = inner.get(at) {
                    if let Some(&[start, '-', end, ..]) = inner.get(at..) {
                        if start > end && dialect == GlobDialect::Strict {
                            return Vec::new();
                        }
                        if start <= end {
                            ranges.push((start, end));
                        }
                        at = at.saturating_add(3);
                    } else {
                        ranges.push((member, member));
                        at = at.saturating_add(1);
                    }
                }
                tokens.push(RefToken::Class { ranges, negated });
                index = close.saturating_add(1);
            }
            other => {
                tokens.push(RefToken::Literal(other));
                index = index.saturating_add(1);
            }
        }
    }
    tokens
}

/// Whether the class accepts a scalar.
fn class_accepts(ranges: &[(char, char)], negated: bool, value: char) -> bool {
    let inside = ranges
        .iter()
        .any(|&(low, high)| low <= value && value <= high);
    inside != negated
}

/// Computes the reference verdict with a whole-path reachability sweep.
///
/// This is the shape the shipped per-token scan replaced, kept here so the
/// seeded sweep compares results rather than re-running the same algorithm.
fn reference_matches(tokens: &[RefToken], path: &[char]) -> bool {
    let mut reachable = vec![false; path.len().saturating_add(1)];
    mark(&mut reachable, 0);
    for token in tokens {
        let mut next = vec![false; path.len().saturating_add(1)];
        match *token {
            RefToken::Literal(literal) => {
                for (offset, (&live, &value)) in reachable.iter().zip(path).enumerate() {
                    if live && value == literal {
                        mark(&mut next, offset.saturating_add(1));
                    }
                }
            }
            RefToken::Question => {
                for (offset, (&live, &value)) in reachable.iter().zip(path).enumerate() {
                    if live && value != '/' {
                        mark(&mut next, offset.saturating_add(1));
                    }
                }
            }
            RefToken::Class {
                ref ranges,
                negated,
            } => {
                for (offset, (&live, &value)) in reachable.iter().zip(path).enumerate() {
                    if live && value != '/' && class_accepts(ranges, negated, value) {
                        mark(&mut next, offset.saturating_add(1));
                    }
                }
            }
            RefToken::Star => {
                // `before` is whether the row's previous offset is reachable.
                let mut before = false;
                for (offset, &live) in reachable.iter().enumerate() {
                    let extend =
                        offset > 0 && before && path.get(offset.saturating_sub(1)) != Some(&'/');
                    before = live || extend;
                    if before {
                        mark(&mut next, offset);
                    }
                }
            }
            RefToken::DoubleStar => {
                let mut any = false;
                for (offset, &live) in reachable.iter().enumerate() {
                    any |= live;
                    if any {
                        mark(&mut next, offset);
                    }
                }
            }
        }
        reachable = next;
    }
    reachable.get(path.len()) == Some(&true)
}

/// Marks offset `at` of a reachability row.
fn mark(row: &mut [bool], at: usize) {
    if let Some(slot) = row.get_mut(at) {
        *slot = true;
    }
}

/// The entry of `table` a draw names, with `first` its first entry.
///
/// The draw is taken below the table's length, so the last of the first
/// `draw + 1` entries is the one it names; the lookup is total, with no index to
/// leave the table.
fn pick<T: Copy>(table: &[T], first: T, draw: u64) -> T {
    let position = table_index(draw.rem_euclid(draw_bound(table.len())));
    table
        .iter()
        .take(position.saturating_add(1))
        .fold(first, |_, &entry| entry)
}

/// Runs the seeded sweep and returns its deterministic trace.
fn run_seeded_sweep(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();

    let alphabets = ["ab/", "ab/?", "ab/éঈ😀", ".-_", "xyz/[]"];
    let path_fragments = ["a", "b", "x/y", "", "é", "😀", ".", "/", "a/b/c", "-"];
    let meta = ['*', '?', '[', ']', '/'];
    let [first_alphabet, ..] = alphabets;
    let [first_fragment, ..] = path_fragments;
    let [first_meta, ..] = meta;

    for _ in 0..1_500 {
        let alphabet: Vec<char> = pick(&alphabets, first_alphabet, next_seed(&mut state))
            .chars()
            .collect();
        // The pattern is a seeded run of alphabet scalars.
        let pattern_length = table_index(next_seed(&mut state).rem_euclid(8)).saturating_add(1);
        let mut pattern = String::new();
        // Every alphabet above holds scalars; an empty one would leave the
        // pattern empty, which the reference and the compiler then agree on.
        if let Some(&first_scalar) = alphabet.first() {
            for _ in 0..pattern_length {
                pattern.push(pick(&alphabet, first_scalar, next_seed(&mut state)));
            }
        }
        // Inject the wildcard and class metacharacters at a seeded position.
        let inject_at = table_index(
            next_seed(&mut state).rem_euclid(draw_bound(pattern.chars().count().saturating_add(1))),
        );
        let inject = pick(&meta, first_meta, next_seed(&mut state));
        let pattern: String = {
            let mut chars: Vec<char> = pattern.chars().collect();
            chars.insert(inject_at.min(chars.len()), inject);
            chars.into_iter().collect()
        };

        let dialect = if next_seed(&mut state).rem_euclid(2) == 0 {
            GlobDialect::Strict
        } else {
            GlobDialect::Legacy
        };
        let path: String = {
            let first = pick(&path_fragments, first_fragment, next_seed(&mut state));
            let second = pick(&path_fragments, first_fragment, next_seed(&mut state));
            format!("{first}{second}")
        };

        let compiled = GlobPattern::compile_with_dialect(&pattern, dialect);
        let reference_tokens = reference_tokens(&pattern, dialect);
        if reference_tokens.is_empty() {
            // The reference tokenized to nothing, which it also uses for a
            // strict refusal. Distinguish the two with the real compiler.
            assert!(
                compiled.is_err(),
                "seed {seed}: pattern {pattern:?} refused by the reference must \
                 also be refused by the strict compiler"
            );
            fold(&mut trace, u64::MAX);
            continue;
        }

        let compiled = match compiled {
            Ok(value) => value,
            Err(refusal) => {
                // A refusal the reference tokenized is folded with its text's
                // length, so two seeds that refuse differently diverge.
                fold(&mut trace, u64::MAX);
                fold_usize(&mut trace, refusal.to_string().len());
                continue;
            }
        };

        let mut scratch = GlobScratch::new();
        let compiled_chars: Vec<char> = path.chars().collect();
        let expected = reference_matches(&reference_tokens, &compiled_chars);
        let observed = compiled.is_match_with(&path, &mut scratch);
        fold(&mut trace, u64::from(observed));
        assert_eq!(
            observed, expected,
            "seed {seed}: {pattern:?} against {path:?} under {dialect:?} must \
             agree with the independent reference"
        );

        // The one-call legacy entry and the compiled path must agree.
        if dialect == GlobDialect::Legacy {
            assert_eq!(
                matches(&pattern, &path),
                observed,
                "seed {seed}: the one-call entry must agree with the compiled path"
            );
        }
    }
    trace
}

/// Whether a single wildcard or class component consumes the separator.
///
/// A pattern that is exactly one `?`, one `*`, or one class must never match
/// the bare `/`. A pattern that spells `/` literally may, and a multi-token
/// pattern is not this property at all — `a/b` matches the separator by
/// matching the literal.
fn single_component_matches_slash(pattern: &str, dialect: GlobDialect) -> bool {
    let Ok(compiled) = GlobPattern::compile_with_dialect(pattern, dialect) else {
        return false;
    };
    // Only a single wildcard or class token, with no literal around it.
    let is_single_component =
        matches!(pattern, "?" | "*" | "**") || (pattern.starts_with('[') && pattern.ends_with(']'));
    is_single_component && compiled.is_match("/")
}

#[test]
fn the_same_seed_replays_to_the_same_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(run_seeded_sweep, seed);
    }
}

#[test]
fn different_seeds_produce_different_traces() {
    assert_distinct_seeds_diverge(run_seeded_sweep, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}

#[test]
fn the_separator_is_never_consumed_by_any_single_component() {
    // `/` is excluded from `?`, `*`, and every class including a negated one, in
    // both dialects and however the class is spelled. `**` is deliberately
    // absent: crossing the separator is what it exists to do.
    for pattern in [
        "?", "*", "[/]", "[!-#]", "[^/]", "[a-z]", "[!a-z]", "[--/]", "[!]", "*?", "?*",
    ] {
        assert!(
            !single_component_matches_slash(pattern, GlobDialect::Strict),
            "{pattern:?} must not match the bare separator under strict"
        );
        assert!(
            !single_component_matches_slash(pattern, GlobDialect::Legacy),
            "{pattern:?} must not match the bare separator under legacy"
        );
    }
    // A pattern that spells the separator literally does match it, which is
    // what keeps `a/**/b` against `a/b` working.
    assert!(
        !matches_slash("a/**/b", '/', GlobDialect::Strict),
        "a/**/b needs at least an `a` before the separator"
    );
    assert!(matches_slash("/", '/', GlobDialect::Strict));
}

/// Whether a pattern matches the bare separator as its whole path.
fn matches_slash(pattern: &str, path: char, dialect: GlobDialect) -> bool {
    // A refused pattern matches nothing, which is what this question asks.
    let Ok(compiled) = GlobPattern::compile_with_dialect(pattern, dialect) else {
        return false;
    };
    compiled.is_match(&path.to_string())
}

#[test]
fn the_documented_directory_reach_cases_keep_their_exact_results() -> Result<(), PatternError> {
    // The migration-sensitive forms #154 names, retained exactly.
    for (pattern, path, expected) in [
        ("a/**/b", "a/b", true),
        ("a/**/b", "a/x/y/b", true),
        ("a/**/b", "a/b/c", false),
        ("**/b", "b", true),
        ("**/b", "a/b", true),
        ("a/**", "a", true),
        ("a/**", "a/x/y", true),
        ("a**b", "a/x/y/b", true),
        ("", "", true),
        ("", "a", false),
        ("[", "[", true),
        ("[abc", "[abc", true),
        ("*", "a/b", false),
        ("**", "a/b", true),
    ] {
        assert_eq!(
            matches(pattern, path),
            expected,
            "legacy dialect: {pattern:?} against {path:?}"
        );
        let compiled = GlobPattern::compile_with_dialect(pattern, GlobDialect::Legacy)?;
        assert_eq!(
            compiled.is_match(path),
            expected,
            "compiled legacy dialect: {pattern:?} against {path:?}"
        );
    }
    // The same cases under the strict dialect, where `a**b` is a typed refusal
    // rather than a silent no-match.
    assert!(GlobPattern::compile("a**b").is_err());
    assert!(GlobPattern::compile("[abc").is_err());
    assert!(GlobPattern::compile("[z-a]").is_err());
    Ok(())
}

#[test]
fn unicode_scalar_semantics_hold_across_the_declared_cases() -> Result<(), PatternError> {
    // One scalar matches `?`, whether it is one UTF-8 byte or four: the unit is
    // the scalar, not the byte. Two `?` do not match one scalar.
    assert!(GlobPattern::compile("?")?.is_match("é"));
    assert!(GlobPattern::compile("?")?.is_match("😀"));
    assert!(!GlobPattern::compile("??")?.is_match("é"));
    assert!(!GlobPattern::compile("??")?.is_match("😀"));
    // A decomposed sequence is two scalars and matches `??` without
    // normalization.
    assert!(GlobPattern::compile("??")?.is_match("e\u{301}"));
    assert!(!GlobPattern::compile("?")?.is_match("e\u{301}"));
    // Classes and ranges follow scalar ordering.
    assert!(GlobPattern::compile("[é]")?.is_match("é"));
    assert!(GlobPattern::compile("[অ-ঊ]")?.is_match("ঈ"));
    assert!(GlobPattern::compile("[😀-🙏]")?.is_match("😃"));
    assert!(GlobPattern::compile("[ঊ-অ]").is_err());
    Ok(())
}
