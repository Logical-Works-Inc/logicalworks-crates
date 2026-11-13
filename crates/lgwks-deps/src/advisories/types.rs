//! Domain types and refusals for RustSec advisory evaluation (#799, #942).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::lock::LockError;

/// The kind of an advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[doc(hidden)]
pub enum AdvisoryKind {
    /// A security vulnerability with potential exploitation impact.
    Vulnerability,
    /// An informational advisory (e.g. unmaintained, unsound, or notice).
    Informational(String),
}

impl AdvisoryKind {
    /// Returns the informational label if informational, or `None` if a vulnerability.
    #[must_use]
    #[doc(hidden)]
    pub fn informational_name(&self) -> Option<&str> {
        match *self {
            Self::Vulnerability => None,
            Self::Informational(ref name_str) => Some(name_str.as_str()),
        }
    }

    /// Whether this advisory represents an active vulnerability.
    #[must_use]
    #[doc(hidden)]
    pub const fn is_vulnerability(&self) -> bool {
        matches!(*self, Self::Vulnerability)
    }
}

/// The platforms an advisory's `[affected]` table restricts it to; empty
/// lists mean every platform.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Affected {
    /// `[affected] os`.
    pub(crate) os: Vec<String>,
    /// `[affected] arch`.
    pub(crate) arch: Vec<String>,
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
    /// The `[affected]` platform restriction.
    affected: Affected,
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
            affected: Affected::default(),
        }
    }

    /// Attaches an optional advisory title.
    #[must_use]
    pub(crate) fn with_title(mut self, title_val: impl Into<String>) -> Self {
        self.title = Some(title_val.into());
        self
    }

    /// Attaches the `[affected]` platform restriction.
    #[must_use]
    pub(crate) fn with_affected(mut self, affected: Affected) -> Self {
        self.affected = affected;
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
    #[must_use]
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

    /// The `[affected]` platform restriction.
    #[must_use]
    pub(crate) fn affected(&self) -> &Affected {
        &self.affected
    }
}

/// One lockfile to audit: the label a match names it by, and its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct Lockfile<'text> {
    /// What a match calls this lockfile, e.g. its path under the subject.
    label: &'text str,
    /// The `Cargo.lock` text.
    text: &'text str,
}

impl<'text> Lockfile<'text> {
    /// A lockfile named `label` holding `text`.
    #[must_use]
    #[doc(hidden)]
    pub const fn new(label: &'text str, text: &'text str) -> Self {
        Self { label, text }
    }

    /// The label a match names this lockfile by.
    #[must_use]
    pub(crate) const fn label(&self) -> &'text str {
        self.label
    }

    /// The `Cargo.lock` text.
    #[must_use]
    #[doc(hidden)]
    pub const fn text(&self) -> &'text str {
        self.text
    }
}

/// A resolved crates.io package an advisory applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[doc(hidden)]
pub struct AdvisoryMatch {
    /// Advisory identifier that matched.
    advisory_id: String,
    /// Package name.
    package: String,
    /// The advisory's title, its first `# ` heading.
    title: Option<String>,
    /// Version resolved in the lockfile.
    installed_version: String,
    /// Classification of the matched advisory.
    kind: AdvisoryKind,
    /// Patched version requirements from the advisory.
    patched: Vec<String>,
    /// The advisory's `[affected]` platform restriction.
    affected: Affected,
    /// The label of the lockfile that pins the package.
    lockfile: String,
}

impl AdvisoryMatch {
    /// Constructs a new advisory match of `advisory` against one pin.
    #[must_use]
    pub(crate) fn new(
        advisory: &Advisory,
        installed_version: impl Into<String>,
        lockfile: impl Into<String>,
    ) -> Self {
        Self {
            advisory_id: advisory.id().to_owned(),
            package: advisory.package().to_owned(),
            title: advisory.title().map(str::to_owned),
            installed_version: installed_version.into(),
            kind: advisory.kind().clone(),
            patched: advisory.patched().to_vec(),
            affected: advisory.affected().clone(),
            lockfile: lockfile.into(),
        }
    }

    /// The advisory identifier, e.g. `RUSTSEC-2026-0001`.
    #[must_use]
    #[doc(hidden)]
    pub fn advisory_id(&self) -> &str {
        &self.advisory_id
    }

    /// The package the advisory names.
    #[must_use]
    #[doc(hidden)]
    pub fn package(&self) -> &str {
        &self.package
    }

    /// The advisory's title, its first `# ` heading; `None` when it has none.
    #[must_use]
    #[doc(hidden)]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The version the lockfile resolved.
    #[must_use]
    #[doc(hidden)]
    pub fn installed_version(&self) -> &str {
        &self.installed_version
    }

    /// Whether this match is a vulnerability or an informational notice: only a
    /// vulnerability fails `lgwks-deps advisories`.
    #[must_use]
    #[doc(hidden)]
    pub fn match_kind(&self) -> &AdvisoryKind {
        &self.kind
    }

    /// The patched version requirements, as the advisory writes them.
    #[must_use]
    #[doc(hidden)]
    pub fn match_patched(&self) -> &[String] {
        self.patched.as_slice()
    }

    /// The operating systems `[affected] os` restricts the advisory to.
    #[must_use]
    #[doc(hidden)]
    pub fn os(&self) -> &[String] {
        &self.affected.os
    }

    /// The architectures `[affected] arch` restricts the advisory to.
    #[must_use]
    #[doc(hidden)]
    pub fn arch(&self) -> &[String] {
        &self.affected.arch
    }

    /// Whether the advisory names an os or arch restriction. A restricted
    /// advisory still matches: the consumer decides what its targets are.
    #[must_use]
    #[doc(hidden)]
    pub fn is_restricted(&self) -> bool {
        !self.affected.os.is_empty() || !self.affected.arch.is_empty()
    }

    /// The label of the lockfile that pins the package.
    #[must_use]
    #[doc(hidden)]
    pub fn lockfile(&self) -> &str {
        &self.lockfile
    }
}

/// A pin the audit did not judge, because no crates.io index is its source: a
/// workspace member, a path or git pin, or another registry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
#[doc(hidden)]
pub struct SkippedPin {
    /// The package name.
    pub(crate) name: String,
    /// The version the lockfile pins.
    pub(crate) version: String,
    /// The label of the lockfile that pins it.
    pub(crate) lockfile: String,
    /// The `source` the lockfile writes, `None` for a workspace or path pin.
    pub(crate) source: Option<String>,
}

impl SkippedPin {
    /// What the lockfile's `[[package]]` block writes as `name`.
    #[must_use]
    #[doc(hidden)]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The version the lockfile pins.
    #[must_use]
    #[doc(hidden)]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The label of the lockfile that pins it.
    #[must_use]
    #[doc(hidden)]
    pub fn lockfile(&self) -> &str {
        &self.lockfile
    }

    /// The `source` the lockfile writes, `None` for a workspace or path pin.
    #[must_use]
    #[doc(hidden)]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

/// What the advisory database was when it was read: its git `HEAD`, when it is
/// a checkout, the advisories loaded and a digest over the files read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[doc(hidden)]
pub struct DatabaseIdentity {
    /// FNV-1a digest of the advisory files read: it names a checkout for a
    /// report, and is not a tamper seal.
    pub(crate) digest: String,
    /// The `HEAD` commit hash, when the database is a git checkout.
    pub(crate) commit: Option<String>,
    /// The `HEAD` committer date (`YYYY-MM-DD`), when it is a checkout.
    pub(crate) date: Option<String>,
    /// How many advisory files were loaded.
    pub(crate) advisories: usize,
}

impl DatabaseIdentity {
    /// The digest over every advisory file read, path and content.
    #[must_use]
    #[doc(hidden)]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The `HEAD` commit, `None` when the database is not a git checkout.
    #[must_use]
    #[doc(hidden)]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// The `HEAD` committer date, `None` when not a checkout: a consumer applies
    /// its own staleness rule to it.
    #[must_use]
    #[doc(hidden)]
    pub fn date(&self) -> Option<&str> {
        self.date.as_deref()
    }

    /// How many advisory files were loaded.
    #[must_use]
    #[doc(hidden)]
    pub const fn advisories(&self) -> usize {
        self.advisories
    }
}

/// What one audit decided, over every lockfile it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[doc(hidden)]
pub struct AdvisoryReport {
    /// The database the verdict binds to.
    pub(crate) database: DatabaseIdentity,
    /// How many lockfiles were read.
    pub(crate) lockfiles: usize,
    /// Pins from a crates.io index, the only ones matched.
    pub(crate) packages_audited: usize,
    /// Pins from anywhere else: workspace members, path, git and other
    /// registries, which no RustSec advisory names. Sorted, each named once
    /// per lockfile, and bounded with the audited pins by `MAX_PACKAGES`.
    pub(crate) skipped: Vec<SkippedPin>,
    /// Every match, vulnerability and informational, sorted.
    pub(crate) matches: Vec<AdvisoryMatch>,
}

impl AdvisoryReport {
    /// The database the verdict binds to.
    #[must_use]
    #[doc(hidden)]
    pub const fn database(&self) -> &DatabaseIdentity {
        &self.database
    }

    /// How many lockfiles were read.
    #[must_use]
    #[doc(hidden)]
    pub const fn lockfiles(&self) -> usize {
        self.lockfiles
    }

    /// How many crates.io pins were judged.
    #[must_use]
    #[doc(hidden)]
    pub const fn packages_audited(&self) -> usize {
        self.packages_audited
    }

    /// How many pins were skipped because no crates.io index is their source.
    #[must_use]
    #[doc(hidden)]
    pub const fn packages_skipped(&self) -> usize {
        self.skipped.len()
    }

    /// Every pin skipped because no crates.io index is its source, sorted by
    /// name, version, lockfile and source.
    #[must_use]
    #[doc(hidden)]
    pub fn skipped(&self) -> &[SkippedPin] {
        &self.skipped
    }

    /// Every match, sorted by advisory, package, version and lockfile.
    #[must_use]
    #[doc(hidden)]
    pub fn matches(&self) -> &[AdvisoryMatch] {
        &self.matches
    }
}

/// Why an audit refused to report: never a clean report it could not make.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[doc(hidden)]
pub enum AdvisoryError {
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
    /// An advisory requirement Cargo reads but RustSec does not load.
    UnsupportedRequirement {
        /// The advisory file.
        path: PathBuf,
        /// The requirement as written.
        text: String,
        /// The rule it breaks.
        reason: &'static str,
    },
    /// A database entry whose name is not UTF-8, so it can be neither matched
    /// nor digested.
    NotUtf8 {
        /// The entry, shown lossily.
        path: PathBuf,
    },
    /// A database with no advisory file under `crates` (or `rust`).
    EmptyDatabase {
        /// The database root.
        path: PathBuf,
    },
    /// An input past a bound the audit holds.
    Bound {
        /// What was counted.
        what: &'static str,
        /// The most allowed.
        limit: usize,
    },
    /// A git checkout whose `HEAD` could not be read.
    DatabaseHead {
        /// The database root.
        path: PathBuf,
        /// What git said.
        cause: String,
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
            } => write!(fmt_out, "I/O error at {}: {cause}", path.display()),
            Self::MalformedFrontmatter {
                ref path,
                ref detail,
            } => write!(
                fmt_out,
                "malformed frontmatter in {}: {detail}",
                path.display()
            ),
            Self::InvalidSemver { ref text } => write!(fmt_out, "invalid semver string: {text:?}"),
            Self::UnsupportedRequirement {
                ref path,
                ref text,
                reason,
            } => write!(fmt_out, "{} writes {text:?}, {reason}", path.display()),
            Self::NotUtf8 { ref path } => {
                write!(fmt_out, "{} is not a UTF-8 name", path.display())
            }
            Self::EmptyDatabase { ref path } => write!(
                fmt_out,
                "{} holds no advisory files under crates (nor rust)",
                path.display()
            ),
            Self::Bound { what, limit } => write!(fmt_out, "more than {limit} {what}"),
            Self::DatabaseHead {
                ref path,
                ref cause,
            } => write!(
                fmt_out,
                "the HEAD of {} is unreadable: {cause}",
                path.display()
            ),
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

/// Logs and returns `error`.
pub(super) fn refuse<T>(error: AdvisoryError) -> Result<T, AdvisoryError> {
    lgwks_std::trace::debug!(error = ?error, "advisories refusal");
    Err(error)
}

/// Logs and returns an I/O error.
pub(super) fn err_io<T>(
    target_path: &Path,
    cause_val: impl fmt::Display,
) -> Result<T, AdvisoryError> {
    refuse(AdvisoryError::Io {
        path: target_path.to_path_buf(),
        cause: cause_val.to_string(),
    })
}

/// Logs and returns a malformed frontmatter error.
pub(super) fn err_frontmatter<T>(
    target_path: &Path,
    detail_str: impl Into<String>,
) -> Result<T, AdvisoryError> {
    refuse(AdvisoryError::MalformedFrontmatter {
        path: target_path.to_path_buf(),
        detail: detail_str.into(),
    })
}

/// Logs and returns an invalid SemVer error.
pub(super) fn err_semver<T>(text_val: impl Into<String>) -> Result<T, AdvisoryError> {
    refuse(AdvisoryError::InvalidSemver {
        text: text_val.into(),
    })
}

/// Logs and returns a bound error.
pub(super) fn err_bound<T>(what: &'static str, limit: usize) -> Result<T, AdvisoryError> {
    refuse(AdvisoryError::Bound { what, limit })
}
