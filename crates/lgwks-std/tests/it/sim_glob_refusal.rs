//! Deterministic simulation of the strict glob compiler's refusals: which
//! syntax is refused, and the UTF-8 byte offset the refusal names.
//!
//! The oracle is a reference model written from the documented contract of
//! `PatternError` ("the UTF-8 byte offset where the invalid syntax begins") and
//! the two-pass order of `GlobPattern::compile`: a `**` run that is not a whole
//! path component is refused first, then the first unclosed class or the first
//! class holding a descending range, at the byte where its `[` begins. It
//! shares no code with the shipped compiler.
//!
//! The fools: the weird-bytes fool puts multi-byte scalars before the bad
//! syntax (the regression: a descending range reported its scalar index, not
//! its byte offset), the giant-and-empty fool sends nothing and ten thousand
//! brackets, and the monkey sends seeded runs of every metacharacter.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `sim_strict_refusals_match_the_reference_model` | 4,096 seeded patterns agree on accept, kind and offset |
//! | `sim_legacy_never_refuses_what_strict_refuses` | the migration dialect compiles every seeded pattern |
//! | `sim_a_refusal_offset_is_a_boundary_at_the_bad_syntax` | the offset splits the text and lands on `[` or `*` |
//! | `sim_multibyte_prefixes_move_the_offset_by_their_bytes` | each refusal kind shifts by the prefix's byte length |
//! | `sim_empty_and_giant_patterns_refuse_at_their_first_byte` | the empty pattern compiles; 10,000 `[` refuse at 0 |
//! | `sim_an_inert_suffix_never_moves_the_verdict` | a component with no metacharacter changes no verdict |
//! | `sim_the_refusal_trace_replays_and_seeds_diverge` | same seed, same trace; distinct seeds, distinct traces |

use lgwks_std::glob::{GlobDialect, GlobPattern, PatternError, PatternErrorKind};
use lgwks_std::seeded::Seeded;

use crate::seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, fold, fold_usize,
    initial_trace, next_index, next_seed, seeded_stream,
};

/// Seeded patterns per sweep.
const CASES: usize = 4_096;

/// The scalars a seeded pattern is drawn from: the metacharacters, the class
/// negators and range dash, ASCII, and scalars of two, three and four bytes.
const ALPHABET: [char; 13] = [
    '*', '*', '[', ']', '-', '!', '/', 'a', 'z', 'é', 'ঈ', '😀', '^',
];

/// The scalars the weird-bytes fool puts in front of bad syntax.
const PREFIX: [char; 4] = ['é', 'ঈ', '😀', 'x'];

/// What the model expects of one strict compile: `None` accepts.
type Verdict = Option<(PatternErrorKind, usize)>;

/// The byte offset of scalar `index` in `chars`.
fn byte_at(chars: &[char], index: usize) -> usize {
    chars
        .iter()
        .take(index)
        .map(|scalar| scalar.len_utf8())
        .sum()
}

/// The first content scalar of a class opened at `open`, past its negator.
fn content_start(chars: &[char], open: usize) -> usize {
    let content = open.saturating_add(1);
    if matches!(chars.get(content), Some('!' | '^')) {
        content.saturating_add(1)
    } else {
        content
    }
}

/// The scalar index of the `]` that closes a class opened at `open`, or `None`.
fn class_close(chars: &[char], open: usize) -> Option<usize> {
    let content = content_start(chars, open);
    let search = if chars.get(content) == Some(&']') {
        content.saturating_add(1)
    } else {
        content
    };
    (search..chars.len()).find(|&at| chars.get(at) == Some(&']'))
}

/// Whether a class body holds a range whose first scalar sorts after its last.
fn descends(body: &[char]) -> bool {
    let mut at = 0;
    while at < body.len() {
        if let Some(&[start, '-', end]) = body.get(at..at.saturating_add(3)) {
            if start > end {
                return true;
            }
            at = at.saturating_add(3);
        } else {
            at = at.saturating_add(1);
        }
    }
    false
}

/// The first `**` run that is not a whole path component, as a verdict.
fn double_star_refusal(chars: &[char]) -> Verdict {
    let mut at = 0;
    while let Some(&scalar) = chars.get(at) {
        if scalar == '[' {
            let close = class_close(chars, at)?;
            at = close.saturating_add(1);
        } else if scalar == '*' && chars.get(at.saturating_add(1)) == Some(&'*') {
            let start = at;
            while chars.get(at) == Some(&'*') {
                at = at.saturating_add(1);
            }
            let left = start == 0 || chars.get(start.saturating_sub(1)) == Some(&'/');
            let right = at == chars.len() || chars.get(at) == Some(&'/');
            if at.saturating_sub(start) != 2 || !left || !right {
                return Some((PatternErrorKind::DoubleStarPlacement, byte_at(chars, start)));
            }
        } else {
            at = at.saturating_add(1);
        }
    }
    None
}

/// The reference verdict for a strict compile of `pattern`.
fn model(pattern: &str) -> Verdict {
    let chars: Vec<char> = pattern.chars().collect();
    if let Some(refused) = double_star_refusal(&chars) {
        return Some(refused);
    }
    let mut at = 0;
    while let Some(&scalar) = chars.get(at) {
        if scalar != '[' {
            at = at.saturating_add(1);
            continue;
        }
        let Some(close) = class_close(&chars, at) else {
            return Some((PatternErrorKind::UnclosedClass, byte_at(&chars, at)));
        };
        let body = chars.get(content_start(&chars, at)..close);
        if body.is_some_and(descends) {
            return Some((PatternErrorKind::DescendingRange, byte_at(&chars, at)));
        }
        at = close.saturating_add(1);
    }
    None
}

/// The shipped compiler's verdict for a strict compile of `pattern`.
fn shipped(pattern: &str) -> Verdict {
    GlobPattern::compile(pattern).err().map(
        |PatternError {
             kind, byte_offset, ..
         }| (kind, byte_offset),
    )
}

/// Up to `limit` seeded scalars drawn from `table`.
fn seeded_text(state: &mut Seeded, table: &[char], limit: usize) -> String {
    let mut text = String::new();
    let Some(length) = next_index(state).checked_rem(limit.saturating_add(1)) else {
        return text;
    };
    for _ in 0..length {
        let drawn = next_index(state).checked_rem(table.len());
        if let Some(&scalar) = drawn.and_then(|at| table.get(at)) {
            text.push(scalar);
        }
    }
    text
}

/// Folds a verdict into a trace: the kind's position and the offset.
fn fold_verdict(trace: &mut u64, verdict: Verdict) {
    let Some((kind, offset)) = verdict else {
        fold(trace, 0);
        return;
    };
    let code = match kind {
        PatternErrorKind::UnclosedClass => 1,
        PatternErrorKind::DescendingRange => 2,
        PatternErrorKind::DoubleStarPlacement => 3,
        _ => 4,
    };
    fold(trace, code);
    fold_usize(trace, offset);
}

/// Runs the seeded comparison and returns its trace.
fn run(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..CASES {
        let pattern = seeded_text(&mut state, &ALPHABET, 12);
        let observed = shipped(&pattern);
        assert_eq!(
            observed,
            model(&pattern),
            "seed {seed}: {pattern:?} must be judged as the reference judges it"
        );
        fold_verdict(&mut trace, observed);
    }
    fold(&mut trace, next_seed(&mut state));
    trace
}

#[test]
fn sim_strict_refusals_match_the_reference_model() {
    for seed in SWEEP_SEEDS {
        run(seed);
    }
}

#[test]
fn sim_legacy_never_refuses_what_strict_refuses() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..CASES {
            let pattern = seeded_text(&mut state, &ALPHABET, 12);
            assert!(
                GlobPattern::compile_with_dialect(&pattern, GlobDialect::Legacy).is_ok(),
                "seed {seed}: the legacy dialect refuses nothing, yet refused {pattern:?}"
            );
        }
    }
}

#[test]
fn sim_a_refusal_offset_is_a_boundary_at_the_bad_syntax() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..CASES {
            let pattern = seeded_text(&mut state, &ALPHABET, 12);
            let Some((kind, offset)) = shipped(&pattern) else {
                continue;
            };
            assert!(
                pattern.is_char_boundary(offset),
                "seed {seed}: {pattern:?} names byte {offset}, inside a scalar"
            );
            let begins = pattern.get(offset..).and_then(|rest| rest.chars().next());
            let expected = match kind {
                PatternErrorKind::DoubleStarPlacement => Some('*'),
                _ => Some('['),
            };
            assert_eq!(
                begins, expected,
                "seed {seed}: {pattern:?} {kind:?} at byte {offset}"
            );
        }
    }
}

#[test]
fn sim_multibyte_prefixes_move_the_offset_by_their_bytes() {
    let mut state = seeded_stream(0x00C0_FFEE);
    let bad = [
        ("[z-a]", PatternErrorKind::DescendingRange, 0),
        ("[abc", PatternErrorKind::UnclosedClass, 0),
        ("a**b", PatternErrorKind::DoubleStarPlacement, 1),
    ];
    for _ in 0..1_000 {
        let prefix = seeded_text(&mut state, &PREFIX, 8);
        for (tail, kind, shift) in bad {
            let pattern = format!("{prefix}{tail}");
            let offset = prefix.len().saturating_add(shift);
            assert_eq!(
                shipped(&pattern),
                Some((kind, offset)),
                "{pattern:?}: the refusal sits {offset} bytes in, past the prefix"
            );
        }
    }
}

#[test]
fn sim_empty_and_giant_patterns_refuse_at_their_first_byte() {
    assert_eq!(shipped(""), None, "the empty pattern compiles");
    let giant = "[".repeat(10_000);
    assert_eq!(shipped(&giant), Some((PatternErrorKind::UnclosedClass, 0)));
    let stars = "*".repeat(10_000);
    assert_eq!(
        shipped(&stars),
        Some((PatternErrorKind::DoubleStarPlacement, 0))
    );
    let wide = format!("{}[z-a]", "😀".repeat(2_500));
    assert_eq!(
        shipped(&wide),
        Some((PatternErrorKind::DescendingRange, 10_000))
    );
}

/// Path components with no metacharacter, so none can close a class or extend a
/// `**` run: appending one never moves an earlier verdict.
const INERT_SUFFIX: [&str; 3] = ["/ab", "/é", "/😀x"];

#[test]
fn sim_an_inert_suffix_never_moves_the_verdict() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..CASES {
            let pattern = seeded_text(&mut state, &ALPHABET, 12);
            let before = shipped(&pattern);
            for suffix in INERT_SUFFIX {
                let longer = format!("{pattern}{suffix}");
                assert_eq!(
                    shipped(&longer),
                    before,
                    "seed {seed}: {suffix:?} after {pattern:?} must leave its verdict alone"
                );
            }
        }
    }
}

#[test]
fn sim_the_refusal_trace_replays_and_seeds_diverge() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(run, seed);
    }
    assert_distinct_seeds_diverge(run, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
