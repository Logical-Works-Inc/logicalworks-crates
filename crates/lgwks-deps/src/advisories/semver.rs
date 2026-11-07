//! SemVer parsing and requirement evaluation for RustSec advisories (#799).

use std::cmp::Ordering;

use super::types::{AdvisoryError, err_semver};

/// An identifier within a pre-release SemVer tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PrereleaseId {
    /// A numeric pre-release segment compared numerically.
    Numeric(u64),
    /// An alphanumeric pre-release segment compared ASCII lexicographically.
    Alpha(String),
}

impl Ord for PrereleaseId {
    fn cmp(&self, other: &Self) -> Ordering {
        match *self {
            Self::Numeric(lhs_num) => match *other {
                Self::Numeric(rhs_num) => lhs_num.cmp(&rhs_num),
                Self::Alpha(_) => Ordering::Less,
            },
            Self::Alpha(ref lhs_str) => match *other {
                Self::Numeric(_) => Ordering::Greater,
                Self::Alpha(ref rhs_str) => lhs_str.cmp(rhs_str),
            },
        }
    }
}

impl PartialOrd for PrereleaseId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A parsed SemVer version representing an installed package version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Version {
    /// Major component.
    pub(super) major: u64,
    /// Minor component.
    pub(super) minor: u64,
    /// Patch component.
    pub(super) patch: u64,
    /// Pre-release identifier chain.
    pub(super) pre: Vec<PrereleaseId>,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.major.cmp(&other.major) {
            Ordering::Equal => {}
            ord_res => return ord_res,
        }
        match self.minor.cmp(&other.minor) {
            Ordering::Equal => {}
            ord_res => return ord_res,
        }
        match self.patch.cmp(&other.patch) {
            Ordering::Equal => {}
            ord_res => return ord_res,
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                let mut iter_lhs = self.pre.iter();
                let mut iter_rhs = other.pre.iter();
                loop {
                    match (iter_lhs.next(), iter_rhs.next()) {
                        (Some(lhs_id), Some(rhs_id)) => match lhs_id.cmp(rhs_id) {
                            Ordering::Equal => continue,
                            ord_res => return ord_res,
                        },
                        (Some(_), None) => return Ordering::Greater,
                        (None, Some(_)) => return Ordering::Less,
                        (None, None) => return Ordering::Equal,
                    }
                }
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Splits version text into numeric segments and prerelease suffix.
fn split_version_core(text_input: &str) -> Result<(Vec<&str>, &str), AdvisoryError> {
    let trimmed = text_input.trim();
    let text = if let Some(stripped) = trimmed.strip_prefix('v') {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix('V') {
        stripped
    } else {
        trimmed
    };

    let without_build = match text.split('+').next() {
        Some(segment) => segment,
        None => text,
    };
    let (num_part, pre_part) = match without_build.split_once('-') {
        Some((lhs_num, rhs_pre)) => (lhs_num, rhs_pre),
        None => (without_build, ""),
    };

    let parts: Vec<&str> = num_part.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return err_semver(text_input);
    }
    Ok((parts, pre_part))
}

/// Parses a prerelease string into a vector of prerelease identifiers.
fn parse_prerelease(pre_part: &str, text_input: &str) -> Result<Vec<PrereleaseId>, AdvisoryError> {
    let mut pre = Vec::new();
    if pre_part.is_empty() {
        return Ok(pre);
    }
    for seg in pre_part.split('.') {
        if seg.is_empty() {
            return err_semver(text_input);
        }
        let is_all_digits = seg.chars().all(|ch| ch.is_ascii_digit());
        let no_leading_zero = seg == "0" || !seg.starts_with('0');
        if is_all_digits && no_leading_zero {
            match seg.parse::<u64>() {
                Ok(num) => pre.push(PrereleaseId::Numeric(num)),
                Err(_) => return err_semver(text_input),
            }
        } else {
            let valid_chars = seg
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
            if !valid_chars {
                return err_semver(text_input);
            }
            pre.push(PrereleaseId::Alpha(seg.to_owned()));
        }
    }
    Ok(pre)
}

/// Parses a version component or defaults to 0 if not present.
fn parse_component_or_zero(
    parts: &[&str],
    index: usize,
    text_input: &str,
) -> Result<u64, AdvisoryError> {
    match parts.get(index).copied() {
        Some(part_str) => match part_str.parse::<u64>() {
            Ok(num) => Ok(num),
            Err(_) => err_semver(text_input),
        },
        None => Ok(0),
    }
}

/// Parses an optional version component.
fn parse_component_opt(
    parts: &[&str],
    index: usize,
    text_input: &str,
) -> Result<Option<u64>, AdvisoryError> {
    match parts.get(index).copied() {
        Some(part_str) => match part_str.parse::<u64>() {
            Ok(num) => Ok(Some(num)),
            Err(_) => err_semver(text_input),
        },
        None => Ok(None),
    }
}

/// Parses a SemVer version string into a [`Version`].
pub(super) fn parse_version(text_input: &str) -> Result<Version, AdvisoryError> {
    let (parts, pre_part) = split_version_core(text_input)?;

    let major = match parse_component_opt(&parts, 0, text_input)? {
        Some(num) => num,
        None => return err_semver(text_input),
    };
    let minor = parse_component_or_zero(&parts, 1, text_input)?;
    let patch = parse_component_or_zero(&parts, 2, text_input)?;
    let pre = parse_prerelease(pre_part, text_input)?;

    Ok(Version {
        major,
        minor,
        patch,
        pre,
    })
}

/// A partially specified version used within requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReqVersion {
    /// Major component.
    pub(super) major: u64,
    /// Optional minor component.
    pub(super) minor: Option<u64>,
    /// Optional patch component.
    pub(super) patch: Option<u64>,
    /// Pre-release identifier chain.
    pub(super) pre: Vec<PrereleaseId>,
}

/// Parses a partially or fully specified version within a requirement comparator.
pub(super) fn parse_req_version(text_input: &str) -> Result<ReqVersion, AdvisoryError> {
    let (parts, pre_part) = split_version_core(text_input)?;

    let major = match parse_component_opt(&parts, 0, text_input)? {
        Some(num) => num,
        None => return err_semver(text_input),
    };
    let minor = parse_component_opt(&parts, 1, text_input)?;
    let patch = parse_component_opt(&parts, 2, text_input)?;
    let pre = parse_prerelease(pre_part, text_input)?;

    Ok(ReqVersion {
        major,
        minor,
        patch,
        pre,
    })
}

/// The comparison operator for a SemVer comparator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComparatorOp {
    /// Exact match (`=`).
    Exact,
    /// Strictly greater (`>`).
    Greater,
    /// Greater than or equal (`>=`).
    GreaterEq,
    /// Strictly less (`<`).
    Less,
    /// Less than or equal (`<=`).
    LessEq,
    /// Tilde requirement (`~`).
    Tilde,
    /// Caret requirement (`^`).
    Caret,
    /// Wildcard match (`*`).
    Wildcard,
}

/// A single SemVer comparator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Comparator {
    /// The comparator operator.
    pub(super) op: ComparatorOp,
    /// The target version boundary.
    pub(super) ver: ReqVersion,
}

impl Comparator {
    /// Tests whether an installed [`Version`] satisfies this comparator.
    pub(super) fn matches(&self, target_ver: &Version) -> bool {
        let mut minor_val = 0;
        if let Some(num) = self.ver.minor {
            minor_val = num;
        }
        let mut patch_val = 0;
        if let Some(num) = self.ver.patch {
            patch_val = num;
        }
        let concrete = Version {
            major: self.ver.major,
            minor: minor_val,
            patch: patch_val,
            pre: self.ver.pre.clone(),
        };

        match self.op {
            ComparatorOp::Wildcard => true,
            ComparatorOp::Exact => {
                if self.ver.minor.is_none() {
                    target_ver.major == self.ver.major && target_ver.pre.is_empty()
                } else if self.ver.patch.is_none() {
                    target_ver.major == self.ver.major
                        && target_ver.minor == minor_val
                        && target_ver.pre.is_empty()
                } else {
                    target_ver == &concrete
                }
            }
            ComparatorOp::Greater => target_ver > &concrete,
            ComparatorOp::GreaterEq => target_ver >= &concrete,
            ComparatorOp::Less => target_ver < &concrete,
            ComparatorOp::LessEq => {
                if self.ver.patch.is_none() && self.ver.minor.is_some() {
                    let upper = Version {
                        major: self.ver.major,
                        minor: minor_val.saturating_add(1),
                        patch: 0,
                        pre: Vec::new(),
                    };
                    target_ver < &upper
                } else if self.ver.minor.is_none() {
                    let upper = Version {
                        major: self.ver.major.saturating_add(1),
                        minor: 0,
                        patch: 0,
                        pre: Vec::new(),
                    };
                    target_ver < &upper
                } else {
                    target_ver <= &concrete
                }
            }
            ComparatorOp::Tilde => {
                let upper = if let Some(min_num) = self.ver.minor {
                    Version {
                        major: self.ver.major,
                        minor: min_num.saturating_add(1),
                        patch: 0,
                        pre: Vec::new(),
                    }
                } else {
                    Version {
                        major: self.ver.major.saturating_add(1),
                        minor: 0,
                        patch: 0,
                        pre: Vec::new(),
                    }
                };
                target_ver >= &concrete && target_ver < &upper
            }
            ComparatorOp::Caret => {
                let upper = if self.ver.major > 0 {
                    Version {
                        major: self.ver.major.saturating_add(1),
                        minor: 0,
                        patch: 0,
                        pre: Vec::new(),
                    }
                } else if let Some(min_num) = self.ver.minor {
                    if min_num > 0 {
                        Version {
                            major: 0,
                            minor: min_num.saturating_add(1),
                            patch: 0,
                            pre: Vec::new(),
                        }
                    } else if let Some(pat_num) = self.ver.patch {
                        Version {
                            major: 0,
                            minor: 0,
                            patch: pat_num.saturating_add(1),
                            pre: Vec::new(),
                        }
                    } else {
                        Version {
                            major: 0,
                            minor: 1,
                            patch: 0,
                            pre: Vec::new(),
                        }
                    }
                } else {
                    Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                        pre: Vec::new(),
                    }
                };
                target_ver >= &concrete && target_ver < &upper
            }
        }
    }
}

impl Comparator {
    /// Constructs a wildcard comparator matching any version.
    fn wildcard() -> Self {
        Self {
            op: ComparatorOp::Wildcard,
            ver: ReqVersion {
                major: 0,
                minor: None,
                patch: None,
                pre: Vec::new(),
            },
        }
    }
}

/// Parses a comparator expression string.
pub(super) fn parse_comparator(text_input: &str) -> Result<Comparator, AdvisoryError> {
    let trimmed = text_input.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return Ok(Comparator::wildcard());
    }

    let (op, rest) = if let Some(rest_str) = trimmed.strip_prefix(">=") {
        (ComparatorOp::GreaterEq, rest_str.trim())
    } else if let Some(rest_str) = trimmed.strip_prefix("<=") {
        (ComparatorOp::LessEq, rest_str.trim())
    } else if let Some(rest_str) = trimmed.strip_prefix('>') {
        (ComparatorOp::Greater, rest_str.trim())
    } else if let Some(rest_str) = trimmed.strip_prefix('<') {
        (ComparatorOp::Less, rest_str.trim())
    } else if let Some(rest_str) = trimmed.strip_prefix('=') {
        (ComparatorOp::Exact, rest_str.trim())
    } else if let Some(rest_str) = trimmed.strip_prefix('~') {
        (ComparatorOp::Tilde, rest_str.trim())
    } else if let Some(rest_str) = trimmed.strip_prefix('^') {
        (ComparatorOp::Caret, rest_str.trim())
    } else {
        (ComparatorOp::Caret, trimmed)
    };

    if rest == "*" {
        return Ok(Comparator::wildcard());
    }

    let ver = parse_req_version(rest)?;
    Ok(Comparator { op, ver })
}

/// A SemVer requirement composed of comma-separated comparators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Requirement {
    /// Comparators that must all be satisfied.
    pub(super) comparators: Vec<Comparator>,
}

impl Requirement {
    /// Evaluates whether `version` satisfies every comparator in this requirement.
    pub(super) fn matches(&self, version: &Version) -> bool {
        self.comparators
            .iter()
            .all(|comparator| comparator.matches(version))
    }
}

/// Parses a comma-separated SemVer requirement string.
pub(super) fn parse_requirement(text_input: &str) -> Result<Requirement, AdvisoryError> {
    let trimmed = text_input.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return Ok(Requirement {
            comparators: vec![Comparator {
                op: ComparatorOp::Wildcard,
                ver: ReqVersion {
                    major: 0,
                    minor: None,
                    patch: None,
                    pre: Vec::new(),
                },
            }],
        });
    }

    let mut comparators = Vec::new();
    for part in trimmed.split(',') {
        let part_trimmed = part.trim();
        if !part_trimmed.is_empty() {
            comparators.push(parse_comparator(part_trimmed)?);
        }
    }

    if comparators.is_empty() {
        comparators.push(Comparator {
            op: ComparatorOp::Wildcard,
            ver: ReqVersion {
                major: 0,
                minor: None,
                patch: None,
                pre: Vec::new(),
            },
        });
    }

    Ok(Requirement { comparators })
}
