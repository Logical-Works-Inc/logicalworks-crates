//! Signal pattern validation and matching.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasProtocol/
//! PatternMatcher.swift` (kernel#850 row 15), itself a port of the
//! TypeScript `matchPattern`. Bounds: 16 kind segments, keypath depth 8,
//! 64 predicate AST nodes. An invalid pattern never matches; it fails the
//! match instead of raising.

use super::protocol::{JsonValue, PayloadPredicate, SignalEnvelope, SignalPattern};

/// Maximum dot-segments in a kind glob.
const MAX_SEGMENTS: usize = 16;

/// Maximum keypath depth in a predicate leaf.
const MAX_KEYPATH_DEPTH: usize = 8;

/// Maximum predicate AST nodes.
const MAX_AST_NODES: usize = 64;

/// Validate a pattern without matching. Rejects empty globs, overlong
/// segment lists, non-trailing `**`, oversized predicates, and deep
/// keypaths.
///
/// # Examples
///
/// ```
/// use drishti::native_host::pattern::validate_pattern;
/// use drishti::native_host::protocol::SignalPattern;
///
/// assert!(validate_pattern(&SignalPattern::new(String::from("a.b.**"), None)));
/// // `**` is only legal as the last segment, and an empty glob is invalid.
/// assert!(!validate_pattern(&SignalPattern::new(String::from("a.**.b"), None)));
/// assert!(!validate_pattern(&SignalPattern::new(String::new(), None)));
/// ```
#[must_use]
pub fn validate_pattern(pattern: &SignalPattern) -> bool {
    if pattern.kind_glob().is_empty() {
        return false;
    }
    let segments: Vec<&str> = pattern.kind_glob().split('.').collect();
    if segments.len() > MAX_SEGMENTS {
        return false;
    }
    let last = segments.len().saturating_sub(1);
    for segment in segments.iter().take(last) {
        if *segment == "**" {
            return false;
        }
    }
    match pattern.payload_predicate() {
        None => true,
        Some(predicate) => {
            count_ast_nodes(predicate) <= MAX_AST_NODES && validate_predicate_structure(predicate)
        }
    }
}

/// Count predicate AST nodes: leaves count one, `and`/`or` add one for the
/// node itself plus their children.
fn count_ast_nodes(predicate: &PayloadPredicate) -> usize {
    match *predicate {
        PayloadPredicate::And(ref children) | PayloadPredicate::Or(ref children) => {
            children.iter().fold(1_usize, |sum, child| {
                sum.saturating_add(count_ast_nodes(child))
            })
        }
        _ => 1,
    }
}

/// Every leaf keypath must be at most [`MAX_KEYPATH_DEPTH`] segments deep.
/// Empty segments do not count (matching the kernel split semantics).
fn validate_predicate_structure(predicate: &PayloadPredicate) -> bool {
    match *predicate {
        PayloadPredicate::Eq { ref keypath, .. }
        | PayloadPredicate::Neq { ref keypath, .. }
        | PayloadPredicate::In { ref keypath, .. }
        | PayloadPredicate::Exists { ref keypath } => keypath_depth(keypath) <= MAX_KEYPATH_DEPTH,
        PayloadPredicate::And(ref children) | PayloadPredicate::Or(ref children) => {
            children.iter().all(validate_predicate_structure)
        }
    }
}

/// Keypath depth ignoring empty segments.
fn keypath_depth(keypath: &str) -> usize {
    keypath.split('.').filter(|part| !part.is_empty()).count()
}

/// Match an envelope against a pattern. An invalid pattern never matches.
///
/// # Examples
///
/// ```
/// use drishti::native_host::pattern::match_pattern;
/// use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SignalPattern, SourceKind};
/// # use drishti::canvas_protocol::SessionInvariant;
/// # use drishti::cli::drive::{POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT};
/// # let session = SessionInvariant::new(
/// #     WORK_OBJECT.into(), SESSION_ID.into(), POLICY_BUNDLE.into(), "tenant".into(), USER_SUB.into(),
/// # )?;
///
/// let envelope = SignalEnvelope::new(
///     String::from("s1"),
///     String::from("forms.submitted"),
///     SourceKind::Widget,
///     String::from("widget.forms"),
///     session,
///     JsonValue::Object(Default::default()),
///     String::from("2026-11-03T00:00:00Z"),
/// );
/// assert!(match_pattern(&envelope, &SignalPattern::new(String::from("forms.*"), None)));
/// assert!(!match_pattern(&envelope, &SignalPattern::new(String::from("other.**"), None)));
/// # Ok::<(), drishti::canvas_protocol::SessionInvariantError>(())
/// ```
#[must_use]
pub fn match_pattern(envelope: &SignalEnvelope, pattern: &SignalPattern) -> bool {
    if !validate_pattern(pattern) {
        return false;
    }
    if !match_kind_glob(pattern.kind_glob(), envelope.kind()) {
        return false;
    }
    match pattern.payload_predicate() {
        None => true,
        Some(predicate) => eval_predicate(predicate, envelope.payload()),
    }
}

/// Match a kind against a glob. `*` matches exactly one segment; a trailing
/// `**` matches the prefix plus any (including zero) trailing segments.
/// Empty segments match literally on both sides.
fn match_kind_glob(glob: &str, kind: &str) -> bool {
    let glob_segments: Vec<&str> = glob.split('.').collect();
    let kind_segments: Vec<&str> = kind.split('.').collect();
    if glob_segments.last() == Some(&"**") {
        let prefix_len = glob_segments.len().saturating_sub(1);
        if kind_segments.len() < prefix_len {
            return false;
        }
        for (index, segment) in glob_segments.iter().take(prefix_len).enumerate() {
            let Some(kind_segment) = kind_segments.get(index) else {
                return false;
            };
            if *segment != "*" && segment != kind_segment {
                return false;
            }
        }
        return true;
    }
    if glob_segments.len() != kind_segments.len() {
        return false;
    }
    glob_segments
        .iter()
        .zip(kind_segments.iter())
        .all(|(glob_segment, kind_segment)| *glob_segment == "*" || glob_segment == kind_segment)
}

/// Read a dotted keypath from a payload. Empty segments are skipped, so
/// `a..b` traverses `a` then `b` exactly like the kernel.
fn get_at_keypath<'a>(payload: &'a JsonValue, keypath: &str) -> Option<&'a JsonValue> {
    let mut current = payload;
    for part in keypath.split('.').filter(|part| !part.is_empty()) {
        let map = current.object_value()?;
        current = map.get(part)?;
    }
    Some(current)
}

/// Evaluate one predicate against a payload. A missing key is false for
/// every operator, including `neq`.
fn eval_predicate(predicate: &PayloadPredicate, payload: &JsonValue) -> bool {
    match *predicate {
        PayloadPredicate::Eq {
            ref keypath,
            ref value,
        } => match get_at_keypath(payload, keypath) {
            Some(found) => json_value_equals(found, value),
            None => false,
        },
        PayloadPredicate::Neq {
            ref keypath,
            ref value,
        } => match get_at_keypath(payload, keypath) {
            Some(found) => !json_value_equals(found, value),
            None => false,
        },
        PayloadPredicate::In {
            ref keypath,
            ref values,
        } => match get_at_keypath(payload, keypath) {
            Some(found) => values
                .iter()
                .any(|candidate| json_value_equals(found, candidate)),
            None => false,
        },
        PayloadPredicate::Exists { ref keypath } => get_at_keypath(payload, keypath).is_some(),
        PayloadPredicate::And(ref clauses) => {
            clauses.iter().all(|clause| eval_predicate(clause, payload))
        }
        PayloadPredicate::Or(ref clauses) => {
            clauses.iter().any(|clause| eval_predicate(clause, payload))
        }
    }
}

/// Scalar equality with strict type discipline: booleans never equal
/// numbers, and only null equals null. Numbers compare bit-exactly (JSON
/// payloads cannot express NaN, and the single `-0.0` versus `0.0` edge is
/// documented rather than silently merged).
fn json_value_equals(actual: &JsonValue, expected: &JsonValue) -> bool {
    match *actual {
        JsonValue::Null => matches!(*expected, JsonValue::Null),
        JsonValue::Bool(left) => match *expected {
            JsonValue::Bool(right) => left == right,
            _ => false,
        },
        JsonValue::Number(left) => match *expected {
            JsonValue::Number(right) => left.to_bits() == right.to_bits(),
            _ => false,
        },
        JsonValue::String(ref left) => match *expected {
            JsonValue::String(ref right) => left == right,
            _ => false,
        },
        JsonValue::Array(_) | JsonValue::Object(_) => false,
    }
}

/// Minimal glob matcher supporting `*` (any substring) and `?` (any single
/// character), used for governance kind filters. Iterative two-pointer
/// matching with backtracking: no recursion, so adversarial patterns cannot
/// overflow the stack.
///
/// # Examples
///
/// ```
/// use drishti::native_host::pattern::glob_match;
///
/// assert!(glob_match("intent.*", "intent.find"));
/// assert!(glob_match("forms.?", "forms.x"));
/// assert!(!glob_match("forms.?", "forms.xy"));
/// ```
#[must_use]
pub fn glob_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == value;
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let value_chars: Vec<char> = value.chars().collect();
    let mut pattern_index = 0_usize;
    let mut value_index = 0_usize;
    let mut star_index: Option<usize> = None;
    let mut match_index = 0_usize;
    while value_index < value_chars.len() {
        let in_pattern = pattern_chars.get(pattern_index).copied();
        // The star is tested first: a literal `*` in the subject must not be
        // consumed as if it were matched by the pattern's own `*`.
        if in_pattern == Some('*') {
            star_index = Some(pattern_index);
            match_index = value_index;
            pattern_index = pattern_index.saturating_add(1);
        } else if in_pattern == Some('?') || in_pattern == Some(value_chars[value_index]) {
            pattern_index = pattern_index.saturating_add(1);
            value_index = value_index.saturating_add(1);
        } else if let Some(star) = star_index {
            pattern_index = star.saturating_add(1);
            match_index = match_index.saturating_add(1);
            value_index = match_index;
        } else {
            return false;
        }
    }
    while pattern_chars.get(pattern_index).copied() == Some('*') {
        pattern_index = pattern_index.saturating_add(1);
    }
    pattern_index >= pattern_chars.len()
}

#[cfg(test)]
mod tests {
    use super::{glob_match, validate_pattern};
    use crate::native_host::protocol::{PayloadPredicate, SignalPattern};

    /// Build a glob-only pattern.
    fn glob_only(glob: &str) -> SignalPattern {
        SignalPattern::new(String::from(glob), None)
    }

    /// Pattern validation rejects overlong and misplaced wildcards.
    #[test]
    fn validation_rejects_bad_shapes() -> Result<(), String> {
        if validate_pattern(&glob_only("")) {
            return Err(String::from("empty glob must be invalid"));
        }
        if validate_pattern(&glob_only("a.**.b")) {
            return Err(String::from("non-trailing ** must be invalid"));
        }
        let seventeen = (0..17)
            .map(|index| format!("s{index}"))
            .collect::<Vec<_>>()
            .join(".");
        if validate_pattern(&glob_only(&seventeen)) {
            return Err(String::from("17 segments must be invalid"));
        }
        // 63 leaves plus one `and` node is exactly 64: valid.
        let leaves: Vec<PayloadPredicate> = (0..63)
            .map(|index| PayloadPredicate::exists(format!("k{index}")))
            .collect();
        if !validate_pattern(&SignalPattern::new(
            String::from("a"),
            Some(PayloadPredicate::And(leaves)),
        )) {
            return Err(String::from("64 nodes must be valid"));
        }
        // 64 leaves plus one `and` node is 65: invalid.
        let leaves: Vec<PayloadPredicate> = (0..64)
            .map(|index| PayloadPredicate::exists(format!("k{index}")))
            .collect();
        if validate_pattern(&SignalPattern::new(
            String::from("a"),
            Some(PayloadPredicate::And(leaves)),
        )) {
            return Err(String::from("65 nodes must be invalid"));
        }
        // Nine-deep keypaths are invalid; eight-deep are valid.
        let deep = (0..9)
            .map(|index| format!("k{index}"))
            .collect::<Vec<_>>()
            .join(".");
        if validate_pattern(&SignalPattern::new(
            String::from("a"),
            Some(PayloadPredicate::exists(deep)),
        )) {
            return Err(String::from("depth 9 must be invalid"));
        }
        let shallow = (0..8)
            .map(|index| format!("k{index}"))
            .collect::<Vec<_>>()
            .join(".");
        if !validate_pattern(&SignalPattern::new(
            String::from("a"),
            Some(PayloadPredicate::exists(shallow)),
        )) {
            return Err(String::from("depth 8 must be valid"));
        }
        Ok(())
    }

    /// Assert one glob vector.
    fn assert_glob(pattern: &str, value: &str, want: bool) -> Result<(), String> {
        if glob_match(pattern, value) != want {
            return Err(format!("glob_match({pattern:?}, {value:?}) != {want}"));
        }
        Ok(())
    }

    /// Governance glob vectors, including star runs and question marks.
    #[test]
    fn governance_globs_match() -> Result<(), String> {
        assert_glob("intent.*", "intent.find", true)?;
        assert_glob("intent.*", "intent.find.extra", true)?;
        assert_glob("intent.*", "other.find", false)?;
        assert_glob("*.find", "intent.find", true)?;
        assert_glob("forms.?", "forms.x", true)?;
        assert_glob("forms.?", "forms.xy", false)?;
        assert_glob("*", "anything.at.all", true)?;
        assert_glob("a.**.b", "a.x.b", true)?;
        assert_glob("a.**.b", "a.b", false)?;
        assert_glob("exact", "exact", true)?;
        assert_glob("exact", "other", false)?;
        Ok(())
    }
}
