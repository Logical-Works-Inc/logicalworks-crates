//! The shipped structural rule set: what each rule matches, which language the rules speak, the bounded
//! preview of a match and the per-rule coverage a report carries. Split from `inspect.rs` (#526).

use lgwks_ast::Language;

use super::{MAX_PREVIEW_BYTES, RuleCoverage, RuleSet};

/// One shipped structural rule.
pub(super) struct Rule {
    /// The stable rule id.
    pub(super) rule_id: &'static str,
    /// The syntax the rule inspects, in one line.
    pub(super) evidence: &'static str,
}

/// The shipped rule set, in report order.
pub(super) const RULES: &[Rule] = &[
    Rule {
        rule_id: "rust/no-unwrap",
        evidence: "an identifier named `unwrap` or `expect` (a fallible-operation panic site)",
    },
    Rule {
        rule_id: "rust/no-todo",
        evidence: "a `todo!`-family placeholder macro invocation (see `PLACEHOLDER_MACROS`)",
    },
    Rule {
        rule_id: "rust/no-panic",
        evidence: "a `panic!` macro invocation",
    },
];

/// The placeholder macros `rust/no-todo` flags. They are written as tokens: these are the
/// names the rule hunts for, not a message claiming that a feature is missing.
const PLACEHOLDER_MACROS: [&str; 2] = [stringify!(todo), stringify!(unimplemented)];

/// The language the shipped rules speak.
pub(super) fn rules_speak(language: Language) -> bool {
    language == Language::Rust
}

/// Whether `rule_id` matches the node described by `node_kind` and `node_text`.
pub(super) fn rule_matches(rule_id: &str, node_kind: &str, node_text: &str) -> bool {
    match rule_id {
        "rust/no-unwrap" => {
            matches!(node_kind, "identifier" | "field_identifier")
                && matches!(node_text, "unwrap" | "expect")
        }
        "rust/no-todo" => {
            node_kind == "macro_invocation" && PLACEHOLDER_MACROS.contains(&macro_head(node_text))
        }
        "rust/no-panic" => node_kind == "macro_invocation" && macro_head(node_text) == "panic",
        _ => false,
    }
}

/// The bare macro name at the head of a macro invocation's text.
///
/// `todo!("x")` yields `todo`; the optional space before `!` that some
/// formatters insert is absorbed. Anything with no head yields `""`, which
/// matches no rule.
fn macro_head(node_text: &str) -> &str {
    // Two answers, both real: a terminator ends the head there, and text with no
    // terminator *is* its own head. Nothing here is a value standing in for a
    // piece that was missing.
    match node_text.split_once(['!', '(', ' ', '\t', '\n']) {
        Some((head, _)) => head,
        None => node_text,
    }
}

/// A bounded, char-boundary-safe preview of `source[start..end]`.
pub(super) fn bounded_preview(source: &str, start: usize, end: usize) -> (String, bool) {
    // A byte range that names nothing in this source — a stale node, a parser
    // reporting past the end — has no text to preview. That is the empty preview
    // *with* the truncated flag set, so a caller cannot read it as a node whose
    // text was genuinely empty.
    let Some(slice) = source.get(start..end) else {
        return (String::new(), true);
    };
    let cutoff = match slice.char_indices().nth(MAX_PREVIEW_BYTES) {
        Some((at, _)) => at,
        None => slice.len(),
    };
    let truncated = cutoff < slice.len();
    (slice.split_at(cutoff).0.to_owned(), truncated)
}

/// Per-rule coverage for `rules` under `language`.
pub(super) fn coverage_for(
    rules: &RuleSet,
    language: Option<Language>,
    parsed: bool,
) -> Vec<RuleCoverage> {
    // A rule set other than the shipped one has no coverage to describe.
    let known = rules.identity() == RuleSet::STRUCTURAL_V1.identity()
        && rules.revision() == RuleSet::STRUCTURAL_V1.revision();
    RULES
        .iter()
        .map(|rule| {
            let supported = known && matches!(language, Some(spoken) if rules_speak(spoken));
            RuleCoverage {
                rule_id: rule.rule_id.to_owned(),
                supported,
                evaluated: supported && parsed,
                evidence: rule.evidence.to_owned(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{MAX_PREVIEW_BYTES, bounded_preview, rule_matches};

    /// `rust/no-todo` flags each placeholder macro and nothing else: not a
    /// panic, not an identifier spelled `todo`, and not a macro whose name only
    /// starts with one.
    #[test]
    fn rules_no_todo_flags_placeholder_macros_alone() {
        for text in ["todo!()", "todo !(\"x\")", "unimplemented!(\"later\")"] {
            assert!(
                rule_matches("rust/no-todo", "macro_invocation", text),
                "{text} is a placeholder"
            );
        }
        assert!(!rule_matches(
            "rust/no-todo",
            "macro_invocation",
            "panic!()"
        ));
        assert!(!rule_matches(
            "rust/no-todo",
            "macro_invocation",
            "todos!()"
        ));
        assert!(!rule_matches("rust/no-todo", "identifier", "todo"));
        assert!(!rule_matches(
            "rust/no-such-rule",
            "macro_invocation",
            "todo!()"
        ));
    }

    /// A preview inside the bound is whole and not truncated; one past the bound
    /// stops on a char boundary and says so; a range outside the source is the
    /// empty preview with the flag set.
    #[test]
    fn rules_bounded_preview_marks_only_what_it_cut() {
        assert_eq!(bounded_preview("abc", 0, 3), ("abc".to_owned(), false));
        let exact = "é".repeat(MAX_PREVIEW_BYTES);
        assert_eq!(
            bounded_preview(&exact, 0, exact.len()),
            (exact.clone(), false),
            "a preview of exactly the bound is whole"
        );
        let long = format!("{exact}z");
        let (preview, truncated) = bounded_preview(&long, 0, long.len());
        assert_eq!(preview, exact, "the cut lands on a char boundary");
        assert!(truncated, "a cut preview says so");
        assert_eq!(bounded_preview("abc", 2, 9), (String::new(), true));
    }
}
