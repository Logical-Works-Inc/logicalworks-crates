//! The glob matcher against a regular-expression oracle, with shrinking.

use super::{Outcome, check, runner, shrunk};
use proptest::collection::vec;
use proptest::prelude::{Just, Strategy, prop_oneof};
use proptest::test_runner::TestCaseError;

/// One token of the strict glob grammar, as the oracle reads it.
#[derive(Debug, Clone)]
enum Atom {
    /// A literal scalar.
    Literal(char),
    /// `?`.
    Question,
    /// `*`.
    Star,
    /// `[ab]`, `[!ab]` or `[a-b]`, as written.
    Class(&'static str),
}

/// A strict-dialect pattern: components joined by `/`, each either `**` or a
/// run of atoms.
fn glob_patterns() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        3 => proptest::sample::select(&['a', 'b', '.', 'é'][..]).prop_map(Atom::Literal),
        1 => Just(Atom::Question),
        2 => Just(Atom::Star),
        1 => proptest::sample::select(&["[ab]", "[!a]", "[a-b]", "[^b]"][..]).prop_map(Atom::Class),
    ];
    let component = prop_oneof![
        1 => Just(String::from("**")),
        4 => vec(atom, 0..4).prop_map(|atoms| {
            atoms
                .iter()
                .map(|atom| match *atom {
                    Atom::Literal(literal) => String::from(literal),
                    Atom::Question => String::from("?"),
                    Atom::Star => String::from("*"),
                    Atom::Class(class) => String::from(class),
                })
                .collect::<String>()
        }),
    ];
    vec(component, 1..5)
        .prop_map(|components| components.join("/"))
        .prop_filter("a strict pattern", |pattern| {
            lgwks_std::glob::GlobPattern::compile(pattern).is_ok()
        })
}

/// Paths over the same scalars, `/` included.
fn glob_paths() -> impl Strategy<Value = String> {
    vec(
        proptest::sample::select(&['a', 'b', '.', 'é', '/'][..]),
        0..12,
    )
    .prop_map(|scalars| scalars.into_iter().collect())
}

/// The glob as a regular expression for the `pattern` engine, which shares no
/// code with the glob automaton. The token split follows the grammar (the
/// longest `/**/`-family token first); the matching is the regex engine's.
fn glob_as_regex(pattern: &str) -> String {
    let mut out = String::from(r"(?s)\A");
    let mut rest = pattern;
    while !rest.is_empty() {
        let (piece, used): (String, usize) = if rest.starts_with("/**/") {
            (String::from("/(?:.*/)?"), 4)
        } else if rest.starts_with("/**") {
            (String::from("(?:/.*)?"), 3)
        } else if rest.starts_with("**/") {
            (String::from("(?:.*/)?"), 3)
        } else if rest.starts_with("**") {
            (String::from(".*"), 2)
        } else if rest.starts_with('*') {
            (String::from("[^/]*"), 1)
        } else if rest.starts_with('?') {
            (String::from("[^/]"), 1)
        } else if rest.starts_with('[') {
            // A close bracket at or after the opener ends the class; a token
            // with none runs to its end, which is how a glob spells one.
            let close = match rest.find(']') {
                Some(close) => close,
                None => rest.len(),
            };
            // The opener is at index 0 and a `]` is at index 1 or later, so
            // `close` is at least 1 and the body is the span between them: `[]`
            // and a bare `[` both spell the empty class, which this is.
            let Some(body) = rest.get(1..close) else {
                break;
            };
            let (negated, members) = match body.strip_prefix(['!', '^']) {
                Some(members) => (true, members),
                None => (false, body),
            };
            let members: String = members
                .chars()
                .map(|member| match member {
                    '-' => String::from("-"),
                    other => format!(r"\x{{{:x}}}", u32::from(other)),
                })
                .collect();
            let class = if negated {
                format!("[^/{members}]")
            } else {
                format!("[{members}&&[^/]]")
            };
            (class, close.saturating_add(1))
        } else {
            // The loop condition leaves a non-empty remainder, so the token has
            // a first scalar to escape; an empty remainder ends the pattern.
            let Some(literal) = rest.chars().next() else {
                break;
            };
            (
                format!(r"\x{{{:x}}}", u32::from(literal)),
                literal.len_utf8(),
            )
        };
        out.push_str(&piece);
        // A translator that consumed past the end of the token leaves no
        // remainder, and the pattern ends there rather than restarting.
        let Some(tail) = rest.get(used..) else {
            break;
        };
        rest = tail;
    }
    out.push_str(r"\z");
    out
}

/// A glob matcher under test: `None` when it refuses the pattern.
type Matcher = fn(&str, &str) -> Option<bool>;

/// The shipped strict matcher, through a reused scratch.
fn strict_glob(pattern: &str, path: &str) -> Option<bool> {
    let compiled = lgwks_std::glob::GlobPattern::compile(pattern).ok()?;
    let mut scratch = lgwks_std::glob::GlobScratch::new();
    Some(compiled.is_match_with(path, &mut scratch))
}

/// The matcher agrees with the regex oracle on every generated pair, and a
/// match's scratch stays linear in the path whatever the pattern.
fn glob_property(matcher: Matcher, pattern: &str, path: &str) -> Result<(), TestCaseError> {
    let oracle = lgwks_std::pattern::Regex::new(&glob_as_regex(pattern))
        .map_err(|error| TestCaseError::fail(format!("oracle for {pattern:?}: {error}")))?;
    let expected = oracle.is_match(path);
    let answer = matcher(pattern, path);
    check(answer == Some(expected), || {
        format!("{pattern:?} on {path:?}: {answer:?}, oracle {expected}")
    })?;
    if let Ok(compiled) = lgwks_std::glob::GlobPattern::compile(pattern) {
        let mut scratch = lgwks_std::glob::GlobScratch::new();
        let rerun = compiled.is_match_with(path, &mut scratch);
        check(rerun == expected, || {
            format!("{pattern:?} on {path:?}: a reused scratch answered {rerun}")
        })?;
        let scalars = path.chars().count().saturating_add(1);
        check(
            scratch.row_capacity() <= scalars.saturating_mul(2).saturating_add(8),
            || {
                format!(
                    "{pattern:?} on {path:?}: rows grew to {}",
                    scratch.row_capacity()
                )
            },
        )?;
    }
    Ok(())
}

#[test]
fn glob_agrees_with_a_regex_oracle_on_generated_patterns() -> Outcome {
    runner().run(&(glob_patterns(), glob_paths()), |(pattern, path)| {
        glob_property(strict_glob, &pattern, &path)
    })?;
    Ok(())
}

#[test]
fn glob_property_catches_a_matcher_that_lets_star_cross_a_slash() -> Outcome {
    /// Compiles every lone `*` as `**`'s reach: the classic glob bug.
    fn crossing(pattern: &str, path: &str) -> Option<bool> {
        let star_free = pattern.replace("**", "\u{0}").replace('*', "\u{1}");
        let regex = glob_as_regex(&star_free.replace('\u{0}', "**")).replace(r"\x{1}", ".*");
        let oracle = lgwks_std::pattern::Regex::new(&regex).ok()?;
        Some(oracle.is_match(path))
    }
    let minimal = shrunk(&(glob_patterns(), glob_paths()), |(pattern, path)| {
        glob_property(crossing, &pattern, &path)
    })?;
    assert_eq!(
        minimal,
        (String::from("*"), String::from("/")),
        "the smallest crossing"
    );
    Ok(())
}
