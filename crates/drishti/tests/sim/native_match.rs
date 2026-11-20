//! Matcher sims: kind globs, payload predicates and the governance glob are
//! each compared with a small recursive reference written from the documented
//! rules, over seeded corpora that include hostile characters.

use std::collections::BTreeMap;

use drishti::native_host::pattern::{glob_match, match_pattern, validate_pattern};
use drishti::native_host::protocol::{
    JsonValue, PayloadPredicate, SignalEnvelope, SignalPattern, SourceKind,
};

use crate::fixtures::session;
use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// A widget envelope over the given kind and payload.
fn envelope(kind: &str, payload: JsonValue) -> Result<SignalEnvelope, String> {
    Ok(SignalEnvelope::new(
        String::from("signal"),
        String::from(kind),
        SourceKind::Widget,
        String::from("source"),
        session("tenant-m")?,
        payload,
        String::from("2026-11-03T00:00:00Z"),
    ))
}

/// Seeded dotted string from the segment set.
fn dotted(random: &mut SimRng, segments: &[&str], count: usize) -> String {
    let mut parts = Vec::new();
    for _ in 0..count {
        if let Some(part) = random.pick_str(segments) {
            parts.push(part);
        }
    }
    parts.join(".")
}

/// Reference kind match: `*` is one segment, a trailing `**` is any tail,
/// everything else must be equal segment for segment.
fn reference_kind_match(glob: &[&str], kind: &[&str]) -> bool {
    let Some((head, rest)) = glob.split_first() else {
        return kind.is_empty();
    };
    if *head == "**" && rest.is_empty() {
        return true;
    }
    let Some((first, remaining)) = kind.split_first() else {
        return false;
    };
    (*head == "*" || head == first) && reference_kind_match(rest, remaining)
}

/// Reference validity: non-empty, at most 16 segments, `**` only last.
fn reference_glob_valid(glob: &str) -> bool {
    let segments: Vec<&str> = glob.split('.').collect();
    !glob.is_empty()
        && segments.len() <= 16
        && segments
            .iter()
            .take(segments.len().saturating_sub(1))
            .all(|segment| *segment != "**")
}

/// Bug caught: a kind glob that lets `*` cross a dot, drops the zero-tail
/// case of `**`, or honours a misplaced `**`, so a subscriber receives
/// signals outside its subtree.
#[test]
fn sim_match_kind_glob_agrees_with_the_reference() -> Result<(), String> {
    const GLOB_SEGMENTS: &[&str] = &["a", "b", "*", "**", ""];
    const KIND_SEGMENTS: &[&str] = &["a", "b", "", "*", "**"];
    let mut matched = 0_usize;
    let mut missed = 0_usize;
    for seed in 0..2_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(70_000));
        let glob_count = if seed.wrapping_rem(50) == 0 {
            random.below_len(3).saturating_add(15)
        } else {
            random.below_len(4).saturating_add(1)
        };
        let glob = dotted(&mut random, GLOB_SEGMENTS, glob_count);
        let kind_count = random.below_len(4).saturating_add(1);
        let kind = dotted(&mut random, KIND_SEGMENTS, kind_count);
        let pattern = SignalPattern::new(glob.clone(), None);
        let valid = reference_glob_valid(&glob);
        ensure(
            validate_pattern(&pattern) == valid,
            &format!("glob {glob:?}: validity disagrees with the reference ({valid})"),
        )?;
        let glob_parts: Vec<&str> = glob.split('.').collect();
        let kind_parts: Vec<&str> = kind.split('.').collect();
        let want = valid && reference_kind_match(&glob_parts, &kind_parts);
        let got = match_pattern(&envelope(&kind, JsonValue::Null)?, &pattern);
        ensure(
            got == want,
            &format!("glob {glob:?} against {kind:?}: got {got}, reference says {want}"),
        )?;
        if want {
            matched = matched.saturating_add(1);
        } else {
            missed = missed.saturating_add(1);
        }
    }
    ensure(
        matched > 100 && missed > 100,
        "the corpus must hold both verdicts",
    )
}

/// Scalar identity used by the reference: a tag plus exact text, or none for
/// containers (which the documented equality never equates).
fn scalar_key(value: &JsonValue) -> Option<String> {
    match *value {
        JsonValue::String(ref text) => Some(format!("s:{text}")),
        JsonValue::Number(number) => Some(format!("n:{:016x}", number.to_bits())),
        JsonValue::Bool(flag) => Some(format!("b:{flag}")),
        JsonValue::Null => Some(String::from("z")),
        _ => None,
    }
}

/// Reference equality: both scalars of one type with one value.
fn same(left: &JsonValue, right: &JsonValue) -> bool {
    match (scalar_key(left), scalar_key(right)) {
        (Some(first), Some(second)) => first == second,
        _ => false,
    }
}

/// Reference lookup: dotted keypath, empty segments skipped.
fn walk<'a>(payload: &'a JsonValue, keypath: &str) -> Option<&'a JsonValue> {
    let mut current = payload;
    for part in keypath.split('.') {
        if part.is_empty() {
            continue;
        }
        current = current.object_value()?.get(part)?;
    }
    Some(current)
}

/// Reference evaluation of the documented operator table.
fn reference_eval(predicate: &PayloadPredicate, payload: &JsonValue) -> bool {
    match *predicate {
        PayloadPredicate::Eq {
            ref keypath,
            ref value,
        } => walk(payload, keypath).is_some_and(|found| same(found, value)),
        PayloadPredicate::Neq {
            ref keypath,
            ref value,
        } => walk(payload, keypath).is_some_and(|found| !same(found, value)),
        PayloadPredicate::In {
            ref keypath,
            ref values,
        } => walk(payload, keypath)
            .is_some_and(|found| values.iter().any(|candidate| same(found, candidate))),
        PayloadPredicate::Exists { ref keypath } => walk(payload, keypath).is_some(),
        PayloadPredicate::And(ref children) => {
            children.iter().all(|child| reference_eval(child, payload))
        }
        PayloadPredicate::Or(ref children) => {
            children.iter().any(|child| reference_eval(child, payload))
        }
        _ => false,
    }
}

/// Seeded scalar or empty container.
fn scalar(random: &mut SimRng) -> JsonValue {
    match random.below(8) {
        0 => JsonValue::String(String::from("x")),
        1 => JsonValue::String(String::from("y")),
        2 => JsonValue::Number(f64::from(random.below_u32(3))),
        3 => JsonValue::Bool(random.below(2) == 1),
        4 => JsonValue::Null,
        5 => JsonValue::Array(Vec::new()),
        6 => JsonValue::Number(-0.0),
        _ => JsonValue::Object(BTreeMap::new()),
    }
}

/// Seeded payload: an object of up to three keys, nested up to two levels.
fn payload(random: &mut SimRng, depth: usize) -> JsonValue {
    let mut map = BTreeMap::new();
    for key in ["a", "b", "c"] {
        if random.below(3) == 0 {
            continue;
        }
        let value = if depth < 2 && random.below(2) == 0 {
            payload(random, depth.saturating_add(1))
        } else {
            scalar(random)
        };
        map.insert(String::from(key), value);
    }
    JsonValue::Object(map)
}

/// Seeded predicate tree of bounded size.
fn predicate(random: &mut SimRng, depth: usize) -> Result<PayloadPredicate, String> {
    const KEYPATHS: &[&str] = &["a", "b", "c", "a.b", "a.b.c", "b.c", ".a", "a..b", "", "d"];
    let keypath = String::from(
        random
            .pick_str(KEYPATHS)
            .ok_or_else(|| String::from("keypaths must not be empty"))?,
    );
    let kinds = if depth >= 3 { 4 } else { 6 };
    Ok(match random.below(kinds) {
        0 => PayloadPredicate::eq(keypath, scalar(random)),
        1 => PayloadPredicate::neq(keypath, scalar(random)),
        2 => {
            let count = random.below_len(4);
            let mut values = Vec::new();
            for _ in 0..count {
                values.push(scalar(random));
            }
            PayloadPredicate::is_in(keypath, values)
        }
        3 => PayloadPredicate::exists(keypath),
        other => {
            let count = random.below_len(4);
            let mut children = Vec::new();
            for _ in 0..count {
                children.push(predicate(random, depth.saturating_add(1))?);
            }
            if other == 4 {
                PayloadPredicate::And(children)
            } else {
                PayloadPredicate::Or(children)
            }
        }
    })
}

/// Bug caught: an operator that treats a missing key as a mismatch for
/// `neq`, equates `true` with `1` or `[]` with `[]`, or mishandles an empty
/// `and` or `or`, so a filter passes or drops the wrong signals.
#[test]
fn sim_match_predicates_agree_with_the_reference() -> Result<(), String> {
    let mut held = 0_usize;
    let mut failed = 0_usize;
    for seed in 0..3_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(80_000));
        let body = payload(&mut random, 0);
        let tree = predicate(&mut random, 0)?;
        let pattern = SignalPattern::new(String::from("k.**"), Some(tree.clone()));
        ensure(
            validate_pattern(&pattern),
            &format!("seed {seed}: a small tree must validate: {tree:?}"),
        )?;
        let want = reference_eval(&tree, &body);
        let got = match_pattern(&envelope("k.x", body.clone())?, &pattern);
        ensure(
            got == want,
            &format!("seed {seed}: {tree:?} over {body:?}: got {got}, reference says {want}"),
        )?;
        if want {
            held = held.saturating_add(1);
        } else {
            failed = failed.saturating_add(1);
        }
    }
    ensure(
        held > 300 && failed > 300,
        "the corpus must hold both verdicts",
    )
}

/// Reference governance glob: `*` any run, `?` one scalar, recursive.
fn reference_glob(pattern: &[char], value: &[char]) -> bool {
    let Some((head, rest)) = pattern.split_first() else {
        return value.is_empty();
    };
    match *head {
        '*' => (0..=value.len()).any(|skip| reference_glob(rest, value.split_at(skip).1)),
        '?' => value
            .split_first()
            .is_some_and(|split| reference_glob(rest, split.1)),
        literal => value
            .split_first()
            .is_some_and(|split| *split.0 == literal && reference_glob(rest, split.1)),
    }
}

/// Seeded string from the piece set.
fn pieced(random: &mut SimRng, pieces: &[&str], count: usize) -> String {
    let mut out = String::new();
    for _ in 0..count {
        if let Some(piece) = random.pick_str(pieces) {
            out.push_str(piece);
        }
    }
    out
}

/// Bug caught: a governance glob that takes a literal `*` or `?` in the
/// subject for the wildcard of the pattern (kinds may contain both), so a
/// filter of `*` misses a kind such as `a*b`.
#[test]
fn sim_match_governance_glob_agrees_with_the_reference() -> Result<(), String> {
    const SUBJECT: &[&str] = &["a", "b", "\u{e9}", "\u{1f600}", "*", "?", "."];
    const PATTERN: &[&str] = &["a", "b", "\u{e9}", "*", "?", "*", ".", "\u{1f600}"];
    let mut hits = 0_usize;
    let mut misses = 0_usize;
    for seed in 0..4_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(90_000));
        let pattern_count = random.below_len(6);
        let subject_count = random.below_len(7);
        let pattern = pieced(&mut random, PATTERN, pattern_count);
        let subject = pieced(&mut random, SUBJECT, subject_count);
        let pattern_chars: Vec<char> = pattern.chars().collect();
        let subject_chars: Vec<char> = subject.chars().collect();
        let want = reference_glob(&pattern_chars, &subject_chars);
        let got = glob_match(&pattern, &subject);
        ensure(
            got == want,
            &format!("glob {pattern:?} over {subject:?}: got {got}, reference says {want}"),
        )?;
        if want {
            hits = hits.saturating_add(1);
        } else {
            misses = misses.saturating_add(1);
        }
    }
    ensure(
        hits > 300 && misses > 300,
        "the corpus must hold both verdicts",
    )
}

/// Bug caught: a glob matcher with exponential backtracking (a hostile
/// filter of repeated stars stalls the governance reader) or recursion that
/// overflows the stack on a long pattern.
#[test]
fn sim_match_governance_glob_stays_linear_under_hostile_input() -> Result<(), String> {
    let stars = "a*".repeat(1_500);
    let subject = "a".repeat(3_000);
    ensure(
        !glob_match(&format!("{stars}b"), &subject),
        "a trailing literal absent from the subject must fail",
    )?;
    ensure(
        glob_match(&format!("{stars}b"), &format!("{subject}b")),
        "a trailing literal present in the subject must match",
    )?;
    ensure(
        glob_match(&"*".repeat(100_000), "anything at all"),
        "a run of stars matches any subject",
    )?;
    ensure(
        glob_match(&"*".repeat(100_000), ""),
        "a run of stars matches the empty subject",
    )?;
    ensure(
        !glob_match(&"?".repeat(100_000), &"x".repeat(99_999)),
        "one question mark too many must fail",
    )?;
    ensure(
        glob_match(&"?".repeat(100_000), &"\u{e9}".repeat(100_000)),
        "a question mark matches one scalar, not one byte",
    )
}

/// The JSON tree of a pattern whose predicate nests `depth` operators (`and`
/// and `or` alternating, chosen by `flip`) over a single `exists` leaf. Built
/// as a tree, not text: the JSON text reader stops at 128 levels, which is
/// below the depth this sim must reach.
fn nested_pattern(depth: usize, flip: bool) -> lgwks_std::json::Value {
    let mut predicate = lgwks_std::json::json!({"exists": "k"});
    for level in 0..depth {
        let operator = if (level.wrapping_rem(2) == 0) == flip {
            "and"
        } else {
            "or"
        };
        predicate = lgwks_std::json::json!({ (operator): [predicate] });
    }
    lgwks_std::json::json!({"kind_glob": "a.b", "payload_predicate": predicate})
}

/// Bug caught: a decoder whose nesting cap disagrees with the matcher's
/// 64-node budget. One level too many decodes a saved pattern that can never
/// validate (it is stored, then silently never matches); one level too few
/// refuses a pattern that would have matched.
#[test]
fn sim_match_decode_depth_cap_equals_the_validation_budget() -> Result<(), String> {
    for depth in 0..=70_usize {
        for flip in [false, true] {
            let tree = nested_pattern(depth, flip);
            let nodes = depth.saturating_add(1);
            let decoded = lgwks_std::json::from_value::<SignalPattern>(tree);
            ensure(
                decoded.is_ok() == (nodes <= 64),
                &format!(
                    "{nodes} nodes: decode verdict {} but the 64-node budget says {}",
                    decoded.is_ok(),
                    nodes <= 64
                ),
            )?;
            if let Ok(pattern) = decoded {
                ensure(
                    validate_pattern(&pattern),
                    &format!("{nodes} nodes decoded but do not validate"),
                )?;
                let again = lgwks_std::json::to_value(&pattern).map_err(describe)?;
                let reread =
                    lgwks_std::json::from_value::<SignalPattern>(again).map_err(describe)?;
                ensure(reread == pattern, "a saved pattern must reload as it was")?;
            }
        }
    }
    Ok(())
}
