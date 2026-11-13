//! The estate's SemVer against the reference it ports (#942): `semver` 1.0, the
//! crate Cargo and RustSec read versions with, as a dev-only oracle.
//!
//! 7.0.0's tests passed because nothing compared its answers to Cargo's. Here a
//! seed draws versions and requirements from an alphabet built so that every
//! rule meets its edge: pre-releases of one triple, numeric against
//! alphanumeric identifiers, leading zeros, a `u64` overflow, partial versions,
//! wildcards in every position, comma lists past `semver`'s 32 comparators and
//! malformed spellings. Any disagreement, in whether a text parses or in what a
//! parsed requirement matches, fails the seed and prints it.

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Write as _;

use lgwks_std::seeded::Seeded;

use super::{Prerelease, Requirement, Shape, Version, requirement, version};

/// Cases each sweep draws.
const CASES: u64 = 40_000;

/// Number components: zero, small, wide, a leading zero, `u64::MAX` and past it.
const NUMBERS: [&str; 9] = [
    "0",
    "1",
    "2",
    "3",
    "10",
    "01",
    "007",
    "18446744073709551615",
    "18446744073709551616",
];
/// Pre-release identifiers: numeric, a leading zero, alphanumeric, hyphenated,
/// and spellings that are not identifiers.
const IDS: [&str; 12] = [
    "0", "1", "2", "9", "10", "01", "alpha", "beta", "rc", "rc-1", "a_b", "",
];
/// The identifiers of [`IDS`] a parsed version can hold: the judged version
/// must parse on both sides, or the case would compare nothing.
const VALID_IDS: [&str; 9] = ["0", "1", "2", "9", "10", "alpha", "beta", "rc", "rc-1"];
/// Build metadata, leading zeros allowed.
const BUILDS: [&str; 4] = ["b7", "001", "sha.5114f85", ""];
/// Operators, two of them misspelt.
const OPS: [&str; 10] = ["", "=", ">", ">=", "<", "<=", "~", "^", "=>", "~>"];
/// Separators between comparators; a bare space is refused by `semver`.
const JOINS: [&str; 5] = [",", ", ", " , ", "  ,", " "];

/// Why an oracle case failed.
#[derive(Debug)]
enum OracleError {
    /// The seeded stream or a conversion refused a draw.
    Draw(String),
    /// A fixed case names a text the port does not read.
    Fixture(String),
    /// The port and the `semver` reference disagree.
    Disagree(String),
}

impl fmt::Display for OracleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Draw(ref why) => write!(formatter, "draw refused: {why}"),
            Self::Fixture(ref why) => write!(formatter, "fixture unread: {why}"),
            Self::Disagree(ref why) => write!(formatter, "{why}"),
        }
    }
}

impl std::error::Error for OracleError {}

impl From<String> for OracleError {
    fn from(why: String) -> Self {
        Self::Disagree(why)
    }
}

/// An oracle step's result.
type Outcome<T> = Result<T, OracleError>;

/// A seeded draw below `bound`.
fn below(stream: &mut Seeded, bound: u64) -> Outcome<u64> {
    stream
        .below(bound)
        .map_err(|error| OracleError::Draw(format!("{error:?}")))
}

/// One item of `pool`.
fn pick<'pool>(stream: &mut Seeded, pool: &[&'pool str]) -> Outcome<&'pool str> {
    let len = u64::try_from(pool.len()).map_err(|error| OracleError::Draw(error.to_string()))?;
    let at = usize::try_from(below(stream, len)?)
        .map_err(|error| OracleError::Draw(error.to_string()))?;
    pool.get(at)
        .copied()
        .ok_or_else(|| OracleError::Draw("pick past the pool".to_owned()))
}

/// A pre-release tag of one to three identifiers drawn from `pool`.
fn pre_tag(stream: &mut Seeded, pool: &[&str]) -> Outcome<String> {
    let count = below(stream, 3)?.saturating_add(1);
    let mut parts = Vec::new();
    for _ in 0..count {
        parts.push(pick(stream, pool)?);
    }
    Ok(parts.join("."))
}

/// A version text: mostly well formed, sometimes cut, padded or prefixed.
fn version_text(stream: &mut Seeded) -> Outcome<String> {
    let mut text = format!(
        "{}.{}.{}",
        pick(stream, &NUMBERS)?,
        pick(stream, &NUMBERS)?,
        pick(stream, &NUMBERS)?
    );
    if below(stream, 2)? == 0 {
        text = format!("{text}-{}", pre_tag(stream, &IDS)?);
    }
    if below(stream, 6)? == 0 {
        text = format!("{text}+{}", pick(stream, &BUILDS)?);
    }
    match below(stream, 12)? {
        0 => text = format!("v{text}"),
        1 => text = format!(" {text}"),
        2 => text.push('.'),
        _ => {}
    }
    Ok(text)
}

/// One comparator text: an operator, one to three components with wildcards
/// anywhere, and a tag or build only some of the time.
fn comparator_text(stream: &mut Seeded) -> Outcome<String> {
    let parts = below(stream, 3)?.saturating_add(1);
    let mut text = pick(stream, &OPS)?.to_owned();
    text.push_str(pick(stream, &["", " ", "  "])?);
    for part in 0..parts {
        if part > 0 {
            text.push('.');
        }
        if part > 0 && below(stream, 6)? == 0 {
            text.push_str(pick(stream, &["*", "x", "X"])?);
        } else {
            text.push_str(pick(stream, &NUMBERS[..7])?);
        }
    }
    if below(stream, 3)? == 0 {
        text = format!("{text}-{}", pre_tag(stream, &IDS)?);
    }
    if below(stream, 10)? == 0 {
        text = format!("{text}+{}", pick(stream, &BUILDS)?);
    }
    Ok(text)
}

/// A requirement text: `*` alone, or one to four comparators (rarely 33).
fn requirement_text(stream: &mut Seeded) -> Outcome<String> {
    match below(stream, 40)? {
        0 => return Ok(pick(stream, &["*", " * ", "x", "*, 1", "* 1", ""])?.to_owned()),
        1 => {
            let mut text = String::from(">= 1");
            for _ in 0..32 {
                text.push_str(", < 9");
            }
            return Ok(text);
        }
        _ => {}
    }
    let count = below(stream, 4)?.saturating_add(1);
    let mut text = pick(stream, &["", " "])?.to_owned();
    for at in 0..count {
        if at > 0 {
            text.push_str(pick(stream, &JOINS)?);
        }
        text.push_str(&comparator_text(stream)?);
    }
    Ok(text)
}

/// The version the matching case judges, drawn near the requirement's triple.
fn judged(stream: &mut Seeded) -> Outcome<String> {
    let mut text = format!(
        "{}.{}.{}",
        pick(stream, &NUMBERS[..5])?,
        pick(stream, &NUMBERS[..5])?,
        pick(stream, &NUMBERS[..5])?
    );
    if below(stream, 2)? == 0 {
        text = format!("{text}-{}", pre_tag(stream, &VALID_IDS)?);
    }
    Ok(text)
}

/// A disagreement with the reference, logged before it is returned so a
/// failing seed shows in the trace log as well as in the test result.
fn disagree<T>(why: String) -> Outcome<T> {
    lgwks_std::trace::debug!(why = why.as_str(), "advisories: the semver port disagrees");
    Err(OracleError::Disagree(why))
}

/// One seed's case, checked against `semver`, rendered for the trace.
fn case(stream: &mut Seeded, trace: &mut String) -> Outcome<()> {
    let shown = version_text(stream)?;
    let ours = version(&shown).is_some();
    let theirs = ::semver::Version::parse(&shown).is_ok();
    if ours != theirs {
        return disagree(format!("version {shown:?}: estate {ours}, semver {theirs}"));
    }
    let wanted = requirement_text(stream)?;
    let target = judged(stream)?;
    let reference = ::semver::VersionReq::parse(&wanted);
    let estate = requirement(&wanted);
    match (estate.as_ref(), reference.as_ref()) {
        (Some(_), Err(error)) => {
            return disagree(format!(
                "requirement {wanted:?}: estate reads it, semver says {error}"
            ));
        }
        (None, Ok(_)) => {
            return disagree(format!(
                "requirement {wanted:?}: semver reads it, estate does not"
            ));
        }
        _ => {}
    }
    let (Some(estate), Ok(reference)) = (estate, reference) else {
        writeln!(trace, "{shown}|{wanted}|refused").map_err(|error| error.to_string())?;
        return Ok(());
    };
    let (Some(parsed), Ok(oracle)) = (version(&target), ::semver::Version::parse(&target)) else {
        return disagree(format!("the judged version {target:?} did not parse"));
    };
    let ours = estate.matches(&parsed);
    let theirs = reference.matches(&oracle);
    if ours != theirs {
        return disagree(format!(
            "{wanted:?} on {target}: estate {ours}, semver {theirs}"
        ));
    }
    writeln!(trace, "{shown}|{wanted}|{target}|{ours}").map_err(|error| error.to_string())?;
    Ok(())
}

/// Every case of one sweep, and the trace they leave.
fn sweep(seed: u64) -> Outcome<String> {
    let mut stream = Seeded::from_seed(seed);
    let mut trace = String::new();
    for index in 0..CASES {
        case(&mut stream, &mut trace)
            .map_err(|why| OracleError::Disagree(format!("seed {seed:#x} case {index}: {why}")))?;
    }
    Ok(trace)
}

#[test]
fn sim_advisories_semver_agrees_with_version_req() -> Outcome<()> {
    for seed in [
        0x5EED_0942_0000_0001_u64,
        0x5EED_0942_0000_0002,
        0xDEAD_BEEF_0942_0003,
    ] {
        let first = sweep(seed)?;
        if sweep(seed)? != first {
            return disagree(format!("seed {seed:#x}: one seed, two traces"));
        }
    }
    Ok(())
}

/// Parses a version a fixed case names.
fn fixed(text: &str) -> Outcome<Version> {
    version(text).ok_or_else(|| OracleError::Fixture(format!("{text:?} is no version")))
}

/// Parses a requirement a fixed case names.
fn wanted(text: &str) -> Outcome<Requirement> {
    requirement(text).ok_or_else(|| OracleError::Fixture(format!("{text:?} is no requirement")))
}

/// Ported from logical_ci `sim_advisories_pre_releases_gate_like_version_req`:
/// a plain comparator never patches a pre-release, and a same-triple
/// pre-release bound still does. The answers are `semver`'s.
#[test]
fn sim_advisories_pre_releases_gate_like_version_req() -> Outcome<()> {
    const REQUIREMENTS: [&str; 7] = [
        ">= 1.2.3",
        ">= 0.103.16, < 0.104.0-alpha.1",
        ">= 2.0.0-alpha.1, < 2.0.0",
        "^1.2.3",
        "< 2.0.0",
        "= 0.104.0-alpha.8",
        "*",
    ];
    const VERSIONS: [&str; 8] = [
        "1.2.3",
        "1.9.9",
        "2.0.0",
        "2.0.0-alpha.1",
        "0.104.0-alpha.7",
        "0.104.0-alpha.8",
        "0.103.15",
        "0.103.99",
    ];
    if wanted(">= 1.2.3")?.matches(&fixed("2.0.0-alpha.1")?) {
        return disagree(
            "patched >= 1.2.3 holds for 2.0.0-alpha.1, hiding a vulnerability".to_owned(),
        );
    }
    for text in REQUIREMENTS {
        let reference = ::semver::VersionReq::parse(text)
            .map_err(|error| OracleError::Fixture(error.to_string()))?;
        for target in VERSIONS {
            let oracle = ::semver::Version::parse(target)
                .map_err(|error| OracleError::Fixture(error.to_string()))?;
            let ours = wanted(text)?.matches(&fixed(target)?);
            if ours != reference.matches(&oracle) {
                return disagree(format!(
                    "{text} on {target}: estate {ours}, semver disagrees"
                ));
            }
        }
    }
    Ok(())
}

/// Ported from logical_ci `sim_advisories_pre_releases_order_numerically_like_rustsec`:
/// SemVer section 11 precedence, both ways round, so `rc.10` never reads below
/// `rc.9`.
#[test]
fn sim_advisories_pre_releases_order_numerically_like_rustsec() -> Outcome<()> {
    const PRECEDENCE: [(&str, &str, Ordering); 12] = [
        ("1.0.0-rc.9", "1.0.0-rc.10", Ordering::Less),
        ("1.0.0-alpha.2", "1.0.0-alpha.10", Ordering::Less),
        ("1.0.0-2", "1.0.0-10", Ordering::Less),
        ("0.104.0-alpha.7", "0.104.0-alpha.8", Ordering::Less),
        ("1.0.0-alpha", "1.0.0-alpha.1", Ordering::Less),
        ("1.0.0-alpha.1", "1.0.0-alpha.beta", Ordering::Less),
        ("1.0.0-alpha.beta", "1.0.0-beta", Ordering::Less),
        ("1.0.0-beta.2", "1.0.0-beta.11", Ordering::Less),
        ("1.0.0-rc.1", "1.0.0", Ordering::Less),
        ("2.0.0-alpha.1", "2.0.0", Ordering::Less),
        ("2.0.0", "2.0.0-alpha.1", Ordering::Greater),
        ("0.103.15", "0.103.15", Ordering::Equal),
    ];
    let order = |left: &Version, right: &Version| {
        (left.major, left.minor, left.patch)
            .cmp(&(right.major, right.minor, right.patch))
            .then_with(|| left.pre.cmp(&right.pre))
    };
    for (left, right, want) in PRECEDENCE {
        let (low, high) = (fixed(left)?, fixed(right)?);
        let reference = ::semver::Version::parse(left)
            .and_then(|one| Ok(one.cmp(&::semver::Version::parse(right)?)))
            .map_err(|error| error.to_string())?;
        if order(&low, &high) != want || order(&high, &low) != want.reverse() || reference != want {
            return disagree(format!("{left} against {right} is not {want:?}"));
        }
    }
    let mut stream = Seeded::from_seed(0x5EED_0942_00C0_0001);
    for _ in 0..2_000 {
        let (low, high) = (below(&mut stream, 40)?, below(&mut stream, 40)?);
        let left = Prerelease(format!("rc.{low}"));
        let right = Prerelease(format!("rc.{high}"));
        if left.cmp(&right) != low.cmp(&high) {
            return disagree(format!(
                "rc.{low} against rc.{high} does not order as the integers"
            ));
        }
    }
    Ok(())
}

/// RustSec's advisory grammar, recorded from `cargo audit` 0.22.2 loading one
/// advisory per spelling (#942): what it loads, and why it refuses the rest.
#[test]
fn rustsec_loads_one_comparator_or_one_range_and_refuses_the_rest() -> Outcome<()> {
    const LOADED: [&str; 14] = [
        "^0.3.2",
        "~1.4",
        "=3.1.0-beta.2",
        "0.3.2",
        "> 1.0.0",
        "<= 1.0.0",
        "*",
        ">= 1.0.0 , < 2",
        ">=1.0.0,<2.0.0",
        "<2, >=1",
        ">=1.*",
        "^1.*",
        "=1.2",
        "~1",
    ];
    const REFUSED: [(&str, Shape); 8] = [
        ("2.*", Shape::Wildcard),
        ("1.x", Shape::Wildcard),
        (">=1, <2, >=1.5", Shape::TooMany),
        (">=1, >=2", Shape::NotARange),
        ("<1, <2", Shape::NotARange),
        ("=1.0.0, <2", Shape::NotAlone),
        ("~1.2, <1.2.5", Shape::NotAlone),
        (">=1, ^1.5", Shape::NotAlone),
    ];
    for text in LOADED {
        wanted(text)?
            .rustsec_shape()
            .map_err(|shape| format!("{text:?} is refused as {}", shape.reason()))?;
    }
    for (text, shape) in REFUSED {
        if wanted(text)?.rustsec_shape() != Err(shape) {
            return disagree(format!("{text:?} is not refused as {}", shape.reason()));
        }
    }
    for text in ["", ">= 1.0.0-01", ">= v1.0.0", "1.2.3.4"] {
        if requirement(text).is_some() {
            return disagree(format!("{text:?} reads, and cargo audit refuses it"));
        }
    }
    Ok(())
}
