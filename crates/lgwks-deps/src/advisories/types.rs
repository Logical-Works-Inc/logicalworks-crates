//! Domain types and refusals for RustSec advisory evaluation (#799).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::lock::LockError;

/// The kind of an advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum AdvisoryKind {
    /// A security vulnerability with potential exploitation impact.
    Vulnerability,
    /// An informational advisory (e.g. unmaintained, unsound, or notice).
    Informational(String),
}

impl AdvisoryKind {
    /// Returns the informational label if informational, or `None` if a vulnerability.
    #[must_use]
    pub(crate) fn informational_name(&self) -> Option<&str> {
        match *self {
            Self::Vulnerability => None,
            Self::Informational(ref name_str) => Some(name_str.as_str()),
        }
    }

    /// Whether this advisory represents an active vulnerability.
    #[must_use]
    pub(crate) const fn is_vulnerability(&self) -> bool {
        matches!(*self, Self::Vulnerability)
    }
}

/// A parsed RustSec security advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Advisory {
    /// Advisory identifier, e.g. `"RUSTSEC-2026-0001"`.
    id: String,
    /// Name of the affected Cargo package.
    package: String,
    /// Optional advisory title.
    title: Option<String>,
    /// Optional publication date in `"YYYY-MM-DD"` format.
    date: Option<String>,
    /// Optional withdrawal date in `"YYYY-MM-DD"` format.
    withdrawn: Option<String>,
    /// Advisory classification.
    kind: AdvisoryKind,
    /// Patched version requirements.
    patched: Vec<String>,
    /// Unaffected version requirements.
    unaffected: Vec<String>,
}

impl Advisory {
    /// Constructs a new advisory from its parsed components.
    #[must_use]
    pub(crate) fn new(
        advisory_id: impl Into<String>,
        package_name: impl Into<String>,
        pub_date: Option<String>,
        withdrawn_date: Option<String>,
        advisory_kind: AdvisoryKind,
        patched_reqs: Vec<String>,
        unaffected_reqs: Vec<String>,
    ) -> Self {
        Self {
            id: advisory_id.into(),
            package: package_name.into(),
            title: None,
            date: pub_date,
            withdrawn: withdrawn_date,
            kind: advisory_kind,
            patched: patched_reqs,
            unaffected: unaffected_reqs,
        }
    }

    /// Attaches an optional advisory title.
    #[must_use]
    pub(crate) fn with_title(mut self, title_val: impl Into<String>) -> Self {
        self.title = Some(title_val.into());
        self
    }

    /// The advisory identifier, e.g. `"RUSTSEC-2026-0001"`.
    #[must_use]
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// Name of the affected package.
    #[must_use]
    pub(crate) fn package(&self) -> &str {
        &self.package
    }

    /// The advisory title, when it carries one.
    #[cfg(test)]
    pub(crate) fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The publication date, when it carries one.
    #[cfg(test)]
    pub(crate) fn date(&self) -> Option<&str> {
        self.date.as_deref()
    }

    /// Optional withdrawal date in `"YYYY-MM-DD"` format.
    #[must_use]
    pub(crate) fn withdrawn(&self) -> Option<&str> {
        self.withdrawn.as_deref()
    }

    /// Advisory kind: vulnerability or informational.
    #[must_use]
    pub(crate) fn kind(&self) -> &AdvisoryKind {
        &self.kind
    }

    /// Patched version requirement strings.
    #[must_use]
    pub(crate) fn patched(&self) -> &[String] {
        &self.patched
    }

    /// Unaffected version requirement strings.
    #[must_use]
    pub(crate) fn unaffected(&self) -> &[String] {
        &self.unaffected
    }
}

/// A recorded vulnerability match between a resolved package and an advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct AdvisoryMatch {
    /// Advisory identifier that matched.
    advisory_id: String,
    /// Package name.
    package: String,
    /// Version resolved in `Cargo.lock`.
    installed_version: String,
    /// Classification of the matched advisory.
    kind: AdvisoryKind,
    /// Patched version requirements from the advisory.
    patched: Vec<String>,
}

impl AdvisoryMatch {
    /// Constructs a new advisory match.
    #[must_use]
    pub(crate) fn new(
        advisory_id: impl Into<String>,
        package_name: impl Into<String>,
        installed_version: impl Into<String>,
        advisory_kind: AdvisoryKind,
        patched_reqs: Vec<String>,
    ) -> Self {
        Self {
            advisory_id: advisory_id.into(),
            package: package_name.into(),
            installed_version: installed_version.into(),
            kind: advisory_kind,
            patched: patched_reqs,
        }
    }

    /// Returns the security bulletin reference (e.g. `RUSTSEC-YYYY-NNNN`).
    #[must_use]
    pub(crate) fn advisory_id(&self) -> &str {
        &self.advisory_id
    }

    /// Returns the crate identifier affected by the security bulletin.
    #[must_use]
    pub(crate) fn package(&self) -> &str {
        &self.package
    }

    /// The installed package version from `Cargo.lock`.
    #[must_use]
    pub(crate) fn installed_version(&self) -> &str {
        &self.installed_version
    }

    /// Whether this match is a vulnerability or an informational notice: only a
    /// vulnerability fails `lgwks-deps advisories`.
    #[inline]
    #[must_use]
    pub(crate) fn match_kind(&self) -> &AdvisoryKind {
        &self.kind
    }

    /// Returns the patched version specifications.
    #[inline]
    #[must_use]
    pub(crate) fn match_patched(&self) -> &[String] {
        self.patched.as_slice()
    }
}

/// The report produced by auditing a lockfile against an advisory database.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct AdvisoryReport {
    /// Deterministic FNV-1a digest of the advisory database: it names a checkout
    /// for a report, and is not a tamper seal.
    db_digest: String,
    /// Total number of advisory files loaded.
    advisories_loaded: usize,
    /// Total number of non-local packages audited.
    packages_audited: usize,
    /// Vulnerability matches detected.
    matches: Vec<AdvisoryMatch>,
}

impl AdvisoryReport {
    /// Constructs a new advisory report.
    #[must_use]
    pub(crate) fn new(
        db_digest: String,
        advisories_loaded: usize,
        packages_audited: usize,
        matches: Vec<AdvisoryMatch>,
    ) -> Self {
        Self {
            db_digest,
            advisories_loaded,
            packages_audited,
            matches,
        }
    }

    /// Deterministic FNV-1a digest of the advisory database.
    #[must_use]
    pub(crate) fn db_digest(&self) -> &str {
        &self.db_digest
    }

    /// Number of advisory files loaded from the database.
    #[must_use]
    pub(crate) fn advisories_loaded(&self) -> usize {
        self.advisories_loaded
    }

    /// Number of non-local packages evaluated against the database.
    #[must_use]
    pub(crate) fn packages_audited(&self) -> usize {
        self.packages_audited
    }

    /// Every matched advisory vulnerability.
    #[must_use]
    pub(crate) fn matches(&self) -> &[AdvisoryMatch] {
        &self.matches
    }
}

/// Errors that can occur during advisory parsing or lockfile auditing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum AdvisoryError {
    /// An I/O error occurred while reading from `path`.
    Io {
        /// Path where the I/O failure occurred.
        path: PathBuf,
        /// Description of the error.
        cause: String,
    },
    /// Frontmatter in the advisory Markdown file at `path` was malformed or missing required keys.
    MalformedFrontmatter {
        /// Path to the malformed advisory file.
        path: PathBuf,
        /// Detail of the malformed frontmatter.
        detail: String,
    },
    /// A SemVer version or requirement string could not be parsed.
    InvalidSemver {
        /// The invalid text.
        text: String,
    },
    /// A lockfile error occurred.
    Lock(LockError),
}

impl fmt::Display for AdvisoryError {
    fn fmt(&self, fmt_out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io {
                ref path,
                ref cause,
            } => {
                write!(fmt_out, "I/O error at {}: {}", path.display(), cause)
            }
            Self::MalformedFrontmatter {
                ref path,
                ref detail,
            } => {
                write!(
                    fmt_out,
                    "malformed frontmatter in {}: {}",
                    path.display(),
                    detail
                )
            }
            Self::InvalidSemver { ref text } => {
                write!(fmt_out, "invalid semver string: {text:?}")
            }
            Self::Lock(ref lock_err) => write!(fmt_out, "lockfile error: {lock_err}"),
        }
    }
}

impl std::error::Error for AdvisoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Lock(ref lock_err) => Some(lock_err),
            _ => None,
        }
    }
}

impl From<LockError> for AdvisoryError {
    fn from(lock_err: LockError) -> Self {
        Self::Lock(lock_err)
    }
}

/// Logs and returns an I/O error.
pub(super) fn err_io<T>(
    target_path: &Path,
    cause_val: impl fmt::Display,
) -> Result<T, AdvisoryError> {
    let error = AdvisoryError::Io {
        path: target_path.to_path_buf(),
        cause: cause_val.to_string(),
    };
    lgwks_std::trace::debug!(error = ?error, "advisories I/O failure");
    Err(error)
}

/// Logs and returns a malformed frontmatter error.
pub(super) fn err_frontmatter<T>(
    target_path: &Path,
    detail_str: impl Into<String>,
) -> Result<T, AdvisoryError> {
    let error = AdvisoryError::MalformedFrontmatter {
        path: target_path.to_path_buf(),
        detail: detail_str.into(),
    };
    lgwks_std::trace::debug!(error = ?error, "advisories malformed frontmatter");
    Err(error)
}

/// Logs and returns an invalid SemVer error.
pub(super) fn err_semver<T>(text_val: impl Into<String>) -> Result<T, AdvisoryError> {
    let error = AdvisoryError::InvalidSemver {
        text: text_val.into(),
    };
    lgwks_std::trace::debug!(error = ?error, "advisories invalid semver");
    Err(error)
}

/// Logs and returns a lock error.
pub(super) fn err_lock<T>(cause_val: LockError) -> Result<T, AdvisoryError> {
    let error = AdvisoryError::Lock(cause_val);
    lgwks_std::trace::debug!(error = ?error, "advisories lock failure");
    Err(error)
}
