//! SemVer versions and Cargo requirements for RustSec advisories (#799, #942).
//!
//! The grammar is `semver` 1.0's `Version` and `VersionReq` parsers and the
//! rules are its `matches`, ported line for line so the audit agrees with Cargo
//! on every case `sim_advisories_semver_agrees_with_version_req` generates. The
//! rule 7.0.0 missed is the pre-release gate: a pre-release version satisfies a
//! requirement only when some comparator names its own `major.minor.patch` with
//! a pre-release, so `>= 1.2.3` never patches `2.0.0-alpha.1`.
//!
//! RustSec writes a narrower grammar than Cargo reads, and [`Requirement::rustsec_shape`]
//! holds an advisory to it the way `cargo audit` 0.22.2 loads one: one
//! comparator, or one lower and one upper bound; `^`, `~` and `=` alone; no
//! `1.x` or `1.*`.

use std::cmp::Ordering;

use super::types::{AdvisoryError, err_semver};

#[cfg(test)]
mod sim_oracle;

/// The most comparators one requirement holds, as `semver` caps them.
const MAX_COMPARATORS: usize = 32;

/// A pre-release tag, `""` for a release. Numeric identifiers carry no leading
/// zero, so equal text is equal precedence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Prerelease(String);

impl Prerelease {
    /// Whether this is a release rather than a pre-release.
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Whether an identifier is all digits, and so compares numerically.
fn is_numeric(identifier: &str) -> bool {
    identifier.bytes().all(|byte| byte.is_ascii_digit())
}

impl Ord for Prerelease {
    /// SemVer 2.0.0 section 11: a release outranks its pre-releases; numeric
    /// identifiers compare by width then digits, below alphanumeric ones; a
    /// shorter equal prefix ranks first.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.is_empty(), other.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        let mut theirs = other.0.split('.');
        for mine in self.0.split('.') {
            let Some(their) = theirs.next() else {
                return Ordering::Greater;
            };
            let ordering = match (is_numeric(mine), is_numeric(their)) {
                (true, true) => mine.len().cmp(&their.len()).then_with(|| mine.cmp(their)),
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (false, false) => mine.cmp(their),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        if theirs.next().is_none() {
            Ordering::Equal
        } else {
            Ordering::Less
        }
    }
}

impl PartialOrd for Prerelease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A resolved package version: a full triple, an optional pre-release, and
/// build metadata read and dropped, as matching never reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Version {
    /// Major component.
    major: u64,
    /// Minor component.
    minor: u64,
    /// Patch component.
    patch: u64,
    /// Pre-release tag.
    pre: Prerelease,
}

/// One digit run with no leading zero that fits a `u64`, and the text after it.
fn numeric(input: &str) -> Option<(u64, &str)> {
    let mut len = 0_usize;
    let mut value = 0_u64;
    while let Some(&digit) = input.as_bytes().get(len) {
        if !digit.is_ascii_digit() {
            break;
        }
        if value == 0 && len > 0 {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(u64::from(digit.saturating_sub(b'0')))?;
        len = len.saturating_add(1);
    }
    if len == 0 {
        return None;
    }
    Some((value, input.get(len..)?))
}

/// A dot-separated identifier run (`[0-9A-Za-z-]+` segments) and the text after
/// it; `("", input)` when none starts here. A pre-release refuses a numeric
/// segment with a leading zero; build metadata keeps one.
fn identifier(input: &str, pre: bool) -> Option<(&str, &str)> {
    let bytes = input.as_bytes();
    let mut accumulated = 0_usize;
    let mut segment = 0_usize;
    let mut nondigit = false;
    loop {
        match bytes.get(accumulated.saturating_add(segment)).copied() {
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'-') => {
                segment = segment.saturating_add(1);
                nondigit = true;
            }
            Some(b'0'..=b'9') => segment = segment.saturating_add(1),
            boundary => {
                if segment == 0 {
                    if accumulated == 0 && boundary != Some(b'.') {
                        return Some(("", input));
                    }
                    return None;
                }
                if pre && segment > 1 && !nondigit && bytes.get(accumulated) == Some(&b'0') {
                    return None;
                }
                accumulated = accumulated.saturating_add(segment);
                if boundary != Some(b'.') {
                    return Some((input.get(..accumulated)?, input.get(accumulated..)?));
                }
                accumulated = accumulated.saturating_add(1);
                segment = 0;
                nondigit = false;
            }
        }
    }
}

/// A non-empty identifier run after `marker`, or `text` whole when it does not
/// start with `marker`; `None` when the marker opens an empty or broken run.
fn tagged(text: &str, marker: char, pre: bool) -> Option<(&str, &str)> {
    let Some(after) = text.strip_prefix(marker) else {
        return Some(("", text));
    };
    let (tag, rest) = identifier(after, pre)?;
    if tag.is_empty() {
        return None;
    }
    Some((tag, rest))
}

/// `Version::parse`: `major.minor.patch`, then `-pre`, then `+build`, nothing
/// else and no whitespace.
fn version(text: &str) -> Option<Version> {
    let (major, rest) = numeric(text)?;
    let (minor, rest) = numeric(rest.strip_prefix('.')?)?;
    let (patch, rest) = numeric(rest.strip_prefix('.')?)?;
    let (pre, rest) = tagged(rest, '-', true)?;
    let (_, rest) = tagged(rest, '+', false)?;
    rest.is_empty().then(|| Version {
        major,
        minor,
        patch,
        pre: Prerelease(pre.to_owned()),
    })
}

/// Parses a resolved version as `semver::Version::parse` does.
pub(super) fn parse_version(text: &str) -> Result<Version, AdvisoryError> {
    match version(text) {
        Some(parsed) => Ok(parsed),
        None => err_semver(text),
    }
}

/// A comparator's operator; [`Op::Wildcard`] is a bare `1.*` or `1.2.x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `=`.
    Exact,
    /// `>`.
    Greater,
    /// `>=`.
    GreaterEq,
    /// `<`.
    Less,
    /// `<=`.
    LessEq,
    /// `~`.
    Tilde,
    /// `^`, and a bare version.
    Caret,
    /// A bare version with a wildcard component.
    Wildcard,
}

/// One comparator: an operator over a possibly partial version.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Comparator {
    /// The operator.
    op: Op,
    /// Major component.
    major: u64,
    /// Minor component, `None` when omitted or a wildcard.
    minor: Option<u64>,
    /// Patch component, `None` when omitted or a wildcard.
    patch: Option<u64>,
    /// Pre-release tag; only a full triple carries one.
    pre: Prerelease,
}

/// The operator `input` opens with, and the text after it.
fn op(input: &str) -> (Op, &str) {
    const OPS: [(&str, Op); 7] = [
        (">=", Op::GreaterEq),
        ("<=", Op::LessEq),
        ("=", Op::Exact),
        (">", Op::Greater),
        ("<", Op::Less),
        ("~", Op::Tilde),
        ("^", Op::Caret),
    ];
    for (spelling, found) in OPS {
        if let Some(rest) = input.strip_prefix(spelling) {
            return (found, rest);
        }
    }
    (Op::Caret, input)
}

/// `*`, `x` or `X` at the start of `input`, and the text after it.
fn wildcard(input: &str) -> Option<&str> {
    input
        .strip_prefix('*')
        .or_else(|| input.strip_prefix('x'))
        .or_else(|| input.strip_prefix('X'))
}

/// One comparator, and the text after it with its trailing spaces skipped.
fn comparator(input: &str) -> Option<(Comparator, &str)> {
    let (mut op, text) = op(input);
    let bare = input.len() == text.len();
    let (major, text) = numeric(text.trim_start_matches(' '))?;
    let mut widened = false;
    let (minor, text) = match text.strip_prefix('.') {
        None => (None, text),
        Some(after) => match wildcard(after) {
            Some(rest) => {
                widened = true;
                if bare {
                    op = Op::Wildcard;
                }
                (None, rest)
            }
            None => {
                let (minor, rest) = numeric(after)?;
                (Some(minor), rest)
            }
        },
    };
    let (patch, text) = match text.strip_prefix('.') {
        None => (None, text),
        Some(after) => match wildcard(after) {
            Some(rest) => {
                if bare {
                    op = Op::Wildcard;
                }
                (None, rest)
            }
            None if widened => return None,
            None => {
                let (patch, rest) = numeric(after)?;
                (Some(patch), rest)
            }
        },
    };
    let (pre, text) = match patch {
        Some(_) => tagged(text, '-', true)?,
        None => ("", text),
    };
    let text = match patch {
        Some(_) => tagged(text, '+', false)?.1,
        None => text,
    };
    let found = Comparator {
        op,
        major,
        minor,
        patch,
        pre: Prerelease(pre.to_owned()),
    };
    Some((found, text.trim_start_matches(' ')))
}

impl Comparator {
    /// The triple and tag equal, each omitted component matching any.
    fn exact(&self, version: &Version) -> bool {
        version.major == self.major
            && self.minor.is_none_or(|minor| version.minor == minor)
            && self.patch.is_none_or(|patch| version.patch == patch)
            && version.pre == self.pre
    }

    /// The first named component that differs decides, as `ordering` reads it;
    /// an omitted component is never strictly passed.
    fn passes(&self, version: &Version, ordering: Ordering) -> bool {
        if version.major != self.major {
            return version.major.cmp(&self.major) == ordering;
        }
        let Some(minor) = self.minor else {
            return false;
        };
        if version.minor != minor {
            return version.minor.cmp(&minor) == ordering;
        }
        let Some(patch) = self.patch else {
            return false;
        };
        if version.patch != patch {
            return version.patch.cmp(&patch) == ordering;
        }
        version.pre.cmp(&self.pre) == ordering
    }

    /// `~`: the major, and the minor when named, stay; then at or above.
    fn tilde(&self, version: &Version) -> bool {
        if version.major != self.major || self.minor.is_some_and(|minor| version.minor != minor) {
            return false;
        }
        if let Some(patch) = self.patch
            && version.patch != patch
        {
            return version.patch > patch;
        }
        version.pre >= self.pre
    }

    /// `^`: the leftmost named non-zero component stays; then at or above.
    fn caret(&self, version: &Version) -> bool {
        if version.major != self.major {
            return false;
        }
        let Some(minor) = self.minor else {
            return true;
        };
        let Some(patch) = self.patch else {
            return if self.major > 0 {
                version.minor >= minor
            } else {
                version.minor == minor
            };
        };
        if self.major > 0 {
            if version.minor != minor {
                return version.minor > minor;
            }
            if version.patch != patch {
                return version.patch > patch;
            }
        } else if minor > 0 {
            if version.minor != minor {
                return false;
            }
            if version.patch != patch {
                return version.patch > patch;
            }
        } else if version.minor != minor || version.patch != patch {
            return false;
        }
        version.pre >= self.pre
    }

    /// Whether `version` satisfies this comparator alone, before the
    /// pre-release gate.
    fn holds(&self, version: &Version) -> bool {
        match self.op {
            Op::Exact | Op::Wildcard => self.exact(version),
            Op::Greater => self.passes(version, Ordering::Greater),
            Op::GreaterEq => self.exact(version) || self.passes(version, Ordering::Greater),
            Op::Less => self.passes(version, Ordering::Less),
            Op::LessEq => self.exact(version) || self.passes(version, Ordering::Less),
            Op::Tilde => self.tilde(version),
            Op::Caret => self.caret(version),
        }
    }

    /// Whether this comparator opens the gate for a pre-release `version`: it
    /// names the same full triple with a pre-release of its own.
    fn admits_pre(&self, version: &Version) -> bool {
        self.major == version.major
            && self.minor == Some(version.minor)
            && self.patch == Some(version.patch)
            && !self.pre.is_empty()
    }

    /// Whether this comparator sets a lower bound alone (`>`, `>=`).
    const fn is_lower(&self) -> bool {
        matches!(self.op, Op::Greater | Op::GreaterEq)
    }

    /// Whether this comparator sets an upper bound alone (`<`, `<=`).
    const fn is_upper(&self) -> bool {
        matches!(self.op, Op::Less | Op::LessEq)
    }
}

/// A comma-separated requirement: every comparator holds, and a pre-release
/// passes only through a comparator of its own triple. No comparator is `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Requirement {
    /// The comparators, in the order written.
    comparators: Vec<Comparator>,
}

/// Why RustSec would not load a requirement Cargo can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// `1.*` or `1.2.x`.
    Wildcard,
    /// More than two comparators.
    TooMany,
    /// `^`, `~` or `=` beside another comparator.
    NotAlone,
    /// Two comparators that are not one lower and one upper bound.
    NotARange,
}

impl Shape {
    /// The rule the requirement breaks, as a refusal names it.
    pub(super) const fn reason(self) -> &'static str {
        match self {
            Self::Wildcard => "a wildcard comparator, which RustSec does not load",
            Self::TooMany => "more than two comparators",
            Self::NotAlone => "a `^`, `~` or `=` comparator beside another",
            Self::NotARange => "two comparators that are not one lower and one upper bound",
        }
    }
}

impl Requirement {
    /// Whether `version` satisfies the requirement, as `VersionReq::matches`.
    pub(super) fn matches(&self, version: &Version) -> bool {
        if !self
            .comparators
            .iter()
            .all(|comparator| comparator.holds(version))
        {
            return false;
        }
        version.pre.is_empty()
            || self
                .comparators
                .iter()
                .any(|comparator| comparator.admits_pre(version))
    }

    /// Holds the requirement to the grammar RustSec loads advisories with.
    ///
    /// # Errors
    ///
    /// The [`Shape`] `cargo audit` refuses the advisory for.
    pub(super) fn rustsec_shape(&self) -> Result<(), Shape> {
        let shape = self.unloadable();
        match shape {
            None => Ok(()),
            Some(shape) => {
                lgwks_std::trace::debug!(
                    reason = shape.reason(),
                    "advisories: a requirement RustSec will not load"
                );
                Err(shape)
            }
        }
    }

    /// The rule of RustSec's grammar this requirement breaks, if any.
    fn unloadable(&self) -> Option<Shape> {
        if self
            .comparators
            .iter()
            .any(|comparator| comparator.op == Op::Wildcard)
        {
            return Some(Shape::Wildcard);
        }
        match *self.comparators.as_slice() {
            [] | [_] => None,
            [ref first, ref second] => {
                let ranged =
                    |comparator: &Comparator| comparator.is_lower() || comparator.is_upper();
                if !ranged(first) || !ranged(second) {
                    Some(Shape::NotAlone)
                } else if first.is_lower() == second.is_lower() {
                    Some(Shape::NotARange)
                } else {
                    None
                }
            }
            _ => Some(Shape::TooMany),
        }
    }
}

/// `VersionReq::from_str`: `*` alone, or comparators joined by commas, with
/// spaces (only spaces) around them.
fn requirement(text: &str) -> Option<Requirement> {
    let text = text.trim_start_matches(' ');
    if let Some(rest) = wildcard(text) {
        return rest
            .trim_start_matches(' ')
            .is_empty()
            .then(|| Requirement {
                comparators: Vec::new(),
            });
    }
    let mut comparators = Vec::new();
    let mut rest = text;
    loop {
        let (found, after) = comparator(rest)?;
        comparators.push(found);
        if after.is_empty() {
            return Some(Requirement { comparators });
        }
        let next = after.strip_prefix(',')?;
        if comparators.len() >= MAX_COMPARATORS {
            return None;
        }
        rest = next.trim_start_matches(' ');
    }
}

/// Parses a requirement as `semver::VersionReq::parse` does.
pub(super) fn parse_requirement(text: &str) -> Result<Requirement, AdvisoryError> {
    match requirement(text) {
        Some(parsed) => Ok(parsed),
        None => err_semver(text),
    }
}
