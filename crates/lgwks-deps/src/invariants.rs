//! Authored invariant register and its refusal rules.
//!
//! The register deliberately goes through `crate::contract::parse_register`
//! rather than acquiring a TOML dependency or copying the dependency reader.
//! Parsing is shared; this module owns only the invariant schema and the
//! repository-aware checks that give each claim a real enforcement boundary.
//!
//! ## Four separate questions, four separate answers
//!
//! An invariant register can be asked four different things, and conflating
//! them is how a metadata check came to print `enforced`:
//!
//! 1. **Registration validity** — is the register well formed? Answered by
//!    [`Register::parse`], which refuses an unknown enforcement kind, an
//!    unconfined `enforced_by`, or an evidence block that is not a complete
//!    revision/invocation/result triple.
//! 2. **Reference resolution** — does each claim name a mechanism that exists
//!    in *this* repository? Answered by [`audit`]: a lint must be declared and
//!    active in the workspace's own `[lints]` tables, a file must exist inside
//!    the repository and declare an active test item, and a module scope must
//!    resolve to a source file some `mod` item actually reaches.
//! 3. **Execution** — did the named mechanism run and fail on a broken tree?
//!    *This module never answers this.* It executes nothing: no authored
//!    command string is run, and no lint is invoked. [`SCOPE`] states exactly
//!    that limit wherever a verdict is printed, because a doctor that reports
//!    more than it did is the defect this module exists to repair.
//! 4. **Verified outcome** — was a run recorded against the reviewed revision?
//!    Answered only by [`Evidence`], which is what separates [`Status::Attested`]
//!    from [`Status::Resolved`].
//!
//! Human approval is a fifth thing again, and it stays a string: a reviewer's
//! name is a trust decision recorded in a diff, and this module does not
//! pretend that a name is a measurement.
//!
//! The module is split by responsibility: this file owns the schema and the
//! load-time checks, `verdict` the verdict ladder and the error types, and
//! `resolve` the repository-aware resolution and the entry point. Their public
//! items are re-exported here, so every path is `crate::invariants::…`.
//!
//! [`Register::parse`]: crate::invariants::Register::parse
//! [`audit`]: crate::invariants::audit
//! [`Evidence`]: crate::invariants::Evidence
//! [`Status::Attested`]: crate::invariants::Status::Attested
//! [`Status::Resolved`]: crate::invariants::Status::Resolved
//! [`SCOPE`]: crate::invariants::SCOPE

use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::contract;

/// Register location relative to the repository root.
pub const INVARIANTS_PATH: &str = "contract/INVARIANTS.toml";

/// Keys accepted by an `[[invariant]]` block.
const ENTRY_FIELDS: [&str; 12] = [
    "id",
    "statement",
    "scope",
    "owner",
    "enforcement",
    "enforced_by",
    "approved_by",
    "approved_on",
    "review",
    "evidence_revision",
    "evidence_invocation",
    "evidence_result",
];

/// The fields required for an invariant entry.
///
/// `enforced_by` is deliberately absent: its absence is a semantic refusal
/// ([`Refusal::MissingEnforcedBy`]) with its own diagnostic rather than a
/// generic parse failure, and the evidence fields are conditional on the
/// enforcement kind.
const REQUIRED_FIELDS: [&str; 8] = [
    "id",
    "statement",
    "scope",
    "owner",
    "enforcement",
    "approved_by",
    "approved_on",
    "review",
];

/// The one required field whose value is a member of a closed grammar.
///
/// `enforcement = ""` has supplied the field and supplied a value no run can
/// act on, which [`Enforcement::parse`] reports as a value outside the grammar.
/// Reporting it as absent would name a different defect with a different repair.
const ENFORCEMENT_FIELD: &str = "enforcement";

/// The three keys a recorded run is made of, in diagnostic order.
const EVIDENCE_FIELDS: [&str; 3] = [
    "evidence_revision",
    "evidence_invocation",
    "evidence_result",
];

/// The one observed result this gate admits as evidence.
const PASSING_RESULT: &str = "pass";

/// Shortest revision prefix accepted as naming a revision.
///
/// Seven is Git's own abbreviation floor, which is the shortest string a human
/// can be expected to copy out of `git log` without reaching for a tool.
const MIN_REVISION: usize = 7;

/// Longest revision string accepted, covering a full SHA-256 object name.
const MAX_REVISION: usize = 64;

mod binding;
mod parse;
mod resolve;
#[cfg(test)]
mod tests;
mod verdict;

pub use binding::EvidenceGap;
pub use resolve::{audit, check};
pub use verdict::{Audit, ErrorKind, InvariantError, Outcome, Refusal, SCOPE, Status};

// ── Enforcement kind ────────────────────────────────────────────────────────

/// How an invariant claims it is enforced.
///
/// A closed set parsed at load rather than a string matched at use, so a
/// register that writes `review` or `intent` is refused with a typed diagnostic
/// instead of being carried to a comparison that silently treats it as neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Enforcement {
    /// A repository check: a declared lint, or a file that declares a test.
    StaticCheck,
    /// A periodic monitor, whose only possible proof is a recorded run.
    Monitor,
}

impl Enforcement {
    /// Parses the authored spelling, or `None` when it is outside the grammar.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "static-check" => Some(Self::StaticCheck),
            "monitor" => Some(Self::Monitor),
            _ => None,
        }
    }

    /// Stable contract spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StaticCheck => "static-check",
            Self::Monitor => "monitor",
        }
    }

    /// Whether a recorded run is the only thing that can attest this kind.
    ///
    /// A monitor has no repository artifact to resolve against — it runs on a
    /// schedule — so its entry is incomplete without evidence. A static check
    /// may still carry evidence, and then it is attested rather than resolved.
    #[must_use]
    pub const fn requires_evidence(self) -> bool {
        matches!(self, Self::Monitor)
    }
}

impl fmt::Display for Enforcement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ── Typed enforcer references ───────────────────────────────────────────────

/// A lint namespace this gate knows how to look up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum LintNamespace {
    /// A Clippy lint, declared under `[lints.clippy]`.
    Clippy,
    /// A `rustc` lint, declared under `[lints.rust]`.
    Rustc,
    /// A `rustdoc` lint, declared under `[lints.rustdoc]`.
    Rustdoc,
}

impl LintNamespace {
    /// Parses the selector prefix a register writes.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "clippy" => Some(Self::Clippy),
            "rustc" => Some(Self::Rustc),
            "rustdoc" => Some(Self::Rustdoc),
            _ => None,
        }
    }

    /// Parses the `[lints]` table name Cargo declares this namespace under.
    ///
    /// The `rustc` selector is `rustc::` but its table is `[lints.rust]`, so
    /// this mapping is not the identity and is kept as its own function.
    fn parse_table(value: &str) -> Option<Self> {
        match value {
            "clippy" => Some(Self::Clippy),
            "rust" => Some(Self::Rustc),
            "rustdoc" => Some(Self::Rustdoc),
            _ => None,
        }
    }

    /// Stable selector prefix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clippy => "clippy",
            Self::Rustc => "rustc",
            Self::Rustdoc => "rustdoc",
        }
    }
}

impl fmt::Display for LintNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an authored `enforced_by` is not a reference this gate will resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathRefusal {
    /// The path is absolute, so it names a file outside the repository.
    Absolute,
    /// The path walks up out of the repository with `..`.
    ParentDirectory,
}

impl fmt::Display for PathRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Absolute => formatter.write_str("it is an absolute path"),
            Self::ParentDirectory => {
                formatter.write_str("it walks out of the repository with `..`")
            }
        }
    }
}

/// `error` as an `Err`, with the refusal recorded on the debug stream.
fn refused<T, E: fmt::Debug>(error: E) -> Result<T, E> {
    let refusal = Err(error);
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "refused: returning an error to the caller");
    refusal
}

/// A repository-relative path that cannot name a file outside the repository.
///
/// The constructor is the only way to build one, so confinement is a property
/// of the type rather than a check a later reader has to remember. It is
/// lexical: nothing here follows a symlink, which is why the audit still joins
/// the path against the root and tests the result before believing it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct RepoPath(PathBuf);

impl RepoPath {
    /// Parses an authored path, refusing anything that escapes the repository.
    fn parse(value: &str) -> Result<Self, PathRefusal> {
        let path = Path::new(value);
        if path.is_absolute() {
            return refused(PathRefusal::Absolute);
        }
        for component in path.components() {
            match component {
                Component::ParentDir => return refused(PathRefusal::ParentDirectory),
                Component::RootDir | Component::Prefix(_) => {
                    return refused(PathRefusal::Absolute);
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        Ok(Self(path.to_path_buf()))
    }

    /// The path as authored, relative to the repository root.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.display())
    }
}

/// The mechanism an invariant names as its enforcement.
///
/// Only two shapes resolve: a lint selector, and a file inside the repository.
/// An authored string that is neither is refused at load with
/// [`ErrorKind::EnforcerNotConfined`] rather than carried here, so every value
/// of this type is a reference the audit can actually go and look up.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnforcedBy {
    /// A `clippy::`/`rustc::`/`rustdoc::` lint selector.
    Lint {
        /// Namespace the lint is declared under.
        namespace: LintNamespace,
        /// Lint name, without the namespace.
        name: String,
    },
    /// A file inside the repository.
    File(RepoPath),
}

impl EnforcedBy {
    /// Parses an authored selector, or `None` when it is neither a lint nor a
    /// confined path.
    ///
    /// A lint namespace wins over the path reading: `clippy::x` is a valid
    /// relative path syntactically, and letting it fall through to the file
    /// branch would report "no such file" for a claim about a lint.
    fn parse(value: &str) -> Result<Self, PathRefusal> {
        if let Some((namespace, name)) = value.split_once("::")
            && let Some(namespace) = LintNamespace::parse(namespace)
            && valid_lint_name(name)
        {
            return Ok(Self::Lint {
                namespace,
                name: name.to_owned(),
            });
        }
        RepoPath::parse(value).map(Self::File)
    }

    /// The exact spelling an execution record must name to be about this
    /// reference.
    ///
    /// `None` only for a value this type cannot hold; it exists so callers bind
    /// evidence without re-deriving the spelling from a second source.
    #[must_use]
    pub fn selector(&self) -> String {
        match *self {
            Self::Lint {
                namespace,
                ref name,
            } => format!("{namespace}::{name}"),
            Self::File(ref path) => path.to_string(),
        }
    }
}

impl fmt::Display for EnforcedBy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.selector())
    }
}

/// Whether `name` uses the alphabet a lint name is written in.
fn valid_lint_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

// ── Evidence ────────────────────────────────────────────────────────────────

/// A recorded run, bound to a revision, an invocation, and a result.
///
/// Constructed only by `build_evidence`, which refuses an incomplete triple,
/// a revision that is not a Git-style object name, a result that is not
/// `pass`, or an invocation that does not name the entry's own enforcer. That
/// fixes the *selector*. It does not fix the *revision*, which at parse time is
/// only a well-shaped hex string: [`audit`] binds it, by asking Git whether it is
/// a commit in the history of `HEAD` and whether the enforcer is unchanged since,
/// and refuses [`Status::Attested`] with [`Refusal::EvidenceNotBound`] when not.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Evidence {
    /// Revision the run was observed at.
    revision: String,
    /// Command that produced the observed result.
    invocation: String,
}

impl Evidence {
    /// Revision the recorded run was observed at.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Command that produced the observed result.
    #[must_use]
    pub fn invocation(&self) -> &str {
        &self.invocation
    }
}

/// Whether a recorded revision names a revision at all.
fn valid_revision(revision: &str) -> bool {
    (MIN_REVISION..=MAX_REVISION).contains(&revision.len())
        && revision
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

// ── Entry and register ──────────────────────────────────────────────────────

/// One declared, reviewed invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    /// Stable `INV-<SCOPE>-<SLUG>` identifier.
    pub(crate) id: String,
    /// Human-readable claim the repository is meant to preserve.
    pub(crate) statement: String,
    /// Workspace crate or Rust module path bound by the claim.
    pub(crate) scope: String,
    /// Workspace crate responsible for preserving the claim.
    pub(crate) owner: String,
    /// Enforcement kind, parsed from the closed two-word grammar.
    pub(crate) enforcement: Enforcement,
    /// The mechanism named as enforcement, or `None` when the entry names one
    /// and it is absent. Resolution against the repository happens in
    /// [`audit`], not here.
    pub(crate) enforced_by: Option<EnforcedBy>,
    /// Recorded run attesting the claim, when the entry carries one.
    pub(crate) evidence: Option<Evidence>,
    /// Human who approved the claim.
    pub(crate) approved_by: String,
    /// ISO date of the review.
    pub(crate) approved_on: String,
    /// Path or URL to the review evidence.
    pub(crate) review: String,
    /// One-based line where the entry opened.
    pub(crate) line: usize,
}

/// The parsed invariant register.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Register {
    /// Whether invariant refusals are enforcement failures.
    pub(crate) enforce: bool,
    /// Invariants in source order.
    pub(crate) entries: Vec<Entry>,
}

impl Register {
    /// Number of invariant declarations in this register.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Whether invariant refusals are enforcement failures.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let text = std::fs::read_to_string("contract/INVARIANTS.toml")?;
    /// let register = lgwks_deps::invariants::Register::parse(&text).map_err(|kind| format!("{kind:?}"))?;
    /// let fatal = register.enforce();
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn enforce(&self) -> bool {
        self.enforce
    }

    /// Parses an invariant register through the dependency register's shared
    /// fail-closed line reader.
    pub fn parse(text: &str) -> Result<Self, ErrorKind> {
        let raw = contract::parse_register(text, "[[invariant]]", &ENTRY_FIELDS)
            .map_err(parse::map_contract_error)?;
        // The repository-policy keys are the dependency register's; the shared
        // line reader collects them, and this register has no meaning for them.
        if let Some(field) = raw.policy_fields.first() {
            return refused(ErrorKind::UnknownKey {
                line: field.line,
                key: field.key.to_owned(),
            });
        }
        let mut entries = Vec::with_capacity(raw.entries.len());
        for raw_entry in &raw.entries {
            entries.push(parse::build(raw_entry)?);
        }
        Ok(Self {
            enforce: raw.enforce,
            entries,
        })
    }
}
