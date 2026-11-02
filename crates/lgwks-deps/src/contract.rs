//! `contract` owns the human-approved dependency register and enforces
//! INV-APPROVAL-IS-SEMANTIC: an entry is not an approval unless it says *what
//! the standard library cannot do*. A name on a list is a whitelist; a name
//! with a reason, a pin, an approver, a date, and a link to the evidence is a
//! contract, and only the second one is admissible here.
//!
//! The register lives at `contract/APPROVED.toml` in the repo being gated. It
//! is valid TOML so an editor or a human can read it, but it is parsed by a
//! line-oriented reader in this module rather than by a TOML crate: taking a
//! dependency in order to police dependencies would be self-refuting. The
//! reader refuses any line it does not recognise instead of skipping it, so a
//! typo cannot quietly become an unenforced entry.
//!
//! Approval is a diff. There is no command that adds an entry: a human writes
//! the block and commits it, which is what makes the approval reviewable and
//! attributable. `lgwks-deps request` only prints the block to be filled in.

use std::fmt;

use crate::retire::Retire;

mod build;
mod error;
mod read;
#[cfg(test)]
mod tests;

pub use error::ContractError;

pub(crate) use build::{check_field_present, is_identifier, is_iso_date};
pub(crate) use read::{RawEntry, RawField, decode_string, parse_register};

use build::{build, build_policy, fingerprint};
use read::{ENTRY_KEYS, check_aliases, check_duplicate_entry};

// ── The register ────────────────────────────────────────────────────────────

/// Where an approved crate sits in the ladder. Only two tiers are admissible:
/// ELIMINATE and CONSOLIDATE crates do not get entries, they get an
/// `lgwks_std` module, and an entry claiming either tier is a category error.
///
/// Public so a surface freeze can be expressed in the register's own vocabulary
/// rather than as a second list of crate names maintained beside it; see
/// [`crate::audit_direct`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tier {
    /// Out of scope for reimplementation; kept as a direct dependency.
    Boundary,
    /// Kept as audited upstream source under `vendor/`, not as a registry edge.
    Vendor,
}

impl Tier {
    /// Decodes the register spelling of a tier.
    ///
    /// Rejects everything else, including the `eliminate` and `consolidate`
    /// tiers: those are answered by an `lgwks_std` module rather than by an
    /// approval, so a register naming one has made a category error and must
    /// fail rather than fall back to a default tier. Matching is exact and
    /// case-sensitive, so `Boundary` is not accepted for `boundary`.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "boundary" => Some(Self::Boundary),
            "vendor" => Some(Self::Vendor),
            _ => None,
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `Tier` is `Copy` and both variants are unit variants, so matching on
        // the dereferenced value moves nothing.
        formatter.write_str(match *self {
            Self::Boundary => "boundary",
            Self::Vendor => "vendor",
        })
    }
}

/// One approved dependency, with the evidence that justified it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Entry {
    /// Package name as `Cargo.lock` spells it.
    pub(crate) krate: String,
    /// Which tier the approval sits in.
    pub(crate) tier: Tier,
    /// Approved Cargo manifest requirement, exactly as metadata reports it.
    pub(crate) version: String,
    /// Workspace crate responsible for this external capability.
    pub(crate) owner: String,
    /// Stable semantic capability supplied by the dependency.
    pub(crate) capability: String,
    /// SPDX expression the approval was granted under, verbatim as Cargo
    /// reports it for the package.
    ///
    /// Required rather than optional: an approval that does not say which
    /// licence was reviewed cannot be re-checked when the licence changes
    /// upstream, which is the drift this field exists to catch (#208).
    pub(crate) license: String,
    /// Admitted Cargo source class: `registry`, `git`, or `path`.
    pub(crate) source: String,
    /// Admitted origin identity for the source class: a complete Cargo
    /// registry source, a Git repository plus its admitted revision/reference
    /// policy, or an external path authority.
    ///
    /// `None` is a legacy class-only approval. It grants no exact-origin
    /// assurance beyond the one deterministic registry this estate uses, and a
    /// git or path edge requires an authored origin before it can be admitted.
    pub(crate) origin: Option<String>,
    /// Workspace crates permitted to declare this edge directly.
    pub(crate) allowed_consumers: Vec<String>,
    /// Permitted edge kinds: `normal`, `build`, and/or `dev`.
    pub(crate) allowed_kinds: Vec<String>,
    /// The complete set of upstream features this edge may enable, when the
    /// entry constrains them. `None` grandfathered the dimension: the entry
    /// predates feature policy and does not refuse a feature it never named.
    pub(crate) features: Option<Vec<String>>,
    /// Features the edge must enable, a subset of [`features`](Self::features)
    /// when that is also authored. `None` requires nothing.
    pub(crate) required_features: Option<Vec<String>>,
    /// The exact authored `default-features` bit this edge must carry, when the
    /// entry constrains it.
    pub(crate) uses_default_features: Option<bool>,
    /// The exact authored optionality this edge must carry, when constrained.
    pub(crate) optional: Option<bool>,
    /// The exact target `cfg(…)` this edge must be scoped to, when
    /// constrained; `""` requires an unconditional declaration.
    pub(crate) target: Option<String>,
    /// Additional accepted spellings of this crate's Cargo package name.
    ///
    /// An explicit, collision-checked compatibility alias: the entry admits an
    /// observed package whose name is `krate` or one of these. It exists because
    /// Cargo folds `-`/`_` when it decides two packages collide, but that fold
    /// is not package identity; an approval written in one spelling must say so
    /// rather than have the fold silently exempt a different package.
    pub(crate) aliases: Vec<String>,
    /// One sentence naming what the standard library cannot do.
    pub(crate) reason: String,
    /// The human who approved it.
    pub(crate) approved_by: String,
    /// ISO date of approval.
    pub(crate) approved_on: String,
    /// Path or URL to the evidence behind the approval.
    pub(crate) review: String,
    /// How this edge ends, when the register mortgages it (schema 3, #788).
    ///
    /// `None` is a register that predates the mortgage table: the audit
    /// refuses it as [`crate::Refusal::RetireMissing`] rather than the reader
    /// substituting an exit path the reviewer never wrote.
    pub(crate) retire: Option<Retire>,
    /// Line where the entry opened, for diagnosis.
    pub(crate) line: usize,
}

impl Entry {
    /// The ladder tier this approval sits in.
    ///
    /// Public for the same reason [`Tier`] is: the gate reads a tier off
    /// an entry to decide whether a surface may be frozen, and an audit that had
    /// to reach into `pub(crate)` fields could not be written at all.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Workspace crate that is responsible for this capability.
    ///
    /// Public because the surface freeze in [`crate::audit_direct`] decides who
    /// a re-tiered approval belongs to by owner, and a decision a caller cannot
    /// reach is a decision it cannot make.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Package name as `Cargo.lock` spells it.
    ///
    /// The companion of [`Entry::owner`] for the same reason: a refusal that
    /// reports a violated freeze has to say which edge it is about.
    #[must_use]
    pub fn krate(&self) -> &str {
        &self.krate
    }

    /// SPDX expression this approval was granted under.
    ///
    /// Public because the licence audit is a decision about a *comparison*: it
    /// asks whether this string still says what the package's own manifest
    /// says. A caller holding only an opaque `Entry` could report that some
    /// approval drifted without being able to say which expression was
    /// expected.
    #[must_use]
    pub fn license(&self) -> &str {
        &self.license
    }

    /// Whether this approval admits the observed Cargo package name `name`.
    ///
    /// Byte-exact against the Cargo-authored identity, plus any explicitly
    /// authored [`aliases`](Self::aliases). There is no implicit `-`/`_` fold:
    /// a fold would let two distinct packages share one authority.
    #[must_use]
    pub(crate) fn admits(&self, name: &str) -> bool {
        self.krate == name || self.aliases.iter().any(|alias| alias == name)
    }

    /// How this edge ends, when the register mortgages it (schema 3, #788).
    #[must_use]
    pub(crate) fn retire(&self) -> Option<&Retire> {
        self.retire.as_ref()
    }
}

/// The parsed register.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Contract {
    /// When false, the register declares itself to be in adoption mode.
    ///
    /// This is a reviewable posture, not an off switch: flipping it is a diff in
    /// the register itself, never an environment variable a process can set for
    /// itself. It does **not** make refusals non-fatal. `audit_direct` refuses a
    /// stand-down outright when the tree carries violations, so the recorded
    /// value reports which posture the register was read under while the verdict
    /// stays a function of the refusals (#204).
    pub enforce: bool,
    /// Canonical repository URL whose workspace members are local authority.
    pub repository: Option<String>,
    /// Register schema version. Absent means version 1 (a class-only register);
    /// the committed register authors version 3, which adds the exact-origin and
    /// capability-policy keys, the explicit alias list, and every edge's
    /// `[approved.retire]` mortgage (#788).
    pub(crate) schema: u32,
    /// A stable fingerprint of the register text this `Contract` was parsed
    /// from, so a CLI receipt can bind a run to the exact contract revision it
    /// read. See [`Contract::digest`] for what this does and does not claim.
    pub(crate) digest: String,
    /// The repository's own policy, read from `[policy]`. Nothing in it has a
    /// built-in default: this crate is published, so any value compiled into it
    /// would bind every repository that runs the gate to one repository's
    /// choices, with no way to change them short of a new release.
    pub(crate) policy: Policy,
    /// Every approved dependency.
    pub(crate) entries: Vec<Entry>,
}

/// What a register declares about its own repository, beyond the approvals.
///
/// Each field is the register's decision, and an absent field is a decision
/// not made — never a fallback to some other repository's answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Policy {
    /// SPDX licence identifiers an external dependency may be under
    /// (`accepted_licenses`). `None` means the register has not said, and the
    /// licence audit refuses rather than guesses (`LicensePolicyUndeclared`).
    pub(crate) accepted_licenses: Option<Vec<String>>,
    /// The closed set of workspace members and approval owners this repository
    /// has (`surfaces`). `None` means the repository keeps no closed set, and no
    /// member or owner is refused for its name.
    pub(crate) surfaces: Option<Vec<String>>,
    /// Members whose approved edge set is closed (`frozen_surfaces`), and the
    /// tier every approval they own must keep claiming (`frozen_tier`). The two
    /// are declared together or not at all.
    pub(crate) frozen: Option<Frozen>,
}

/// The frozen-surface half of [`Policy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frozen {
    /// Members whose edges may not grow or be re-tiered.
    pub(crate) surfaces: Vec<String>,
    /// The tier their approvals must keep.
    pub(crate) tier: Tier,
}

impl Contract {
    /// Number of reviewed dependency approvals in this register.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Parses a register. Fail-closed: an unrecognised line is an error, not a
    /// line to skip.
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        let raw = parse_register(text, "[[approved]]", &ENTRY_KEYS)?;
        let mut entries: Vec<Entry> = Vec::new();
        for draft in &raw.entries {
            let entry = build(draft)?;
            check_duplicate_entry(&entries, &entry, draft.line)?;
            entries.push(entry);
        }
        check_aliases(&entries)?;
        let policy = build_policy(&raw.policy_fields)?;
        Ok(Self {
            enforce: raw.enforce,
            repository: raw.repository,
            schema: raw.schema,
            digest: fingerprint(text),
            policy,
            entries,
        })
    }

    /// The SPDX licence identifiers this register accepts, or `None` when it
    /// has not declared any.
    #[must_use]
    pub fn accepted_licenses(&self) -> Option<&[String]> {
        self.policy.accepted_licenses.as_deref()
    }

    /// The register schema version this contract was written under.
    #[must_use]
    pub const fn schema(&self) -> u32 {
        self.schema
    }

    /// A stable fingerprint of the register text this contract was parsed from.
    ///
    /// It binds a receipt to the exact contract revision a run read: two
    /// different registers fingerprint differently, so a receipt naming a
    /// digest that does not match the committed register is visibly stale.
    ///
    /// This is an identity fingerprint, not an adversarial integrity claim.
    /// `lgwks_std::hash` (BLAKE3) is the estate's cryptographic primitive and is
    /// deliberately not used here: it sits behind a `lgwks_std` feature that
    /// enabling would add a `blake3` edge to the gate's own dependency graph,
    /// which INV-DEP-EDGE-OWNED refuses. A register is human-authored and
    /// reviewable, and this digest detects accidental drift, not a forger who
    /// can already rewrite the validator.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Finds the approval for a resolved Cargo package name.
    ///
    /// Matching is byte-exact against the Cargo-authored package name, plus any
    /// explicitly authored alias. Returns the first match only; a package that
    /// backs several capabilities has several entries, so callers that need all
    /// of them use `approvals_for`.
    #[must_use]
    pub fn approval_for(&self, krate: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.admits(krate))
    }

    /// Returns every semantic approval for an upstream package. A package may
    /// legitimately back distinct capabilities with different owners.
    ///
    /// Order follows the register, so for a given register the sequence is
    /// deterministic and diffable.
    pub fn approvals_for<'a>(&'a self, krate: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.entries.iter().filter(move |entry| entry.admits(krate))
    }

    /// Every approval in the register, in the order it was written.
    ///
    /// The register-wide counterpart of [`Contract::approvals_for`], for a check
    /// that is about the register's whole posture rather than one package's
    /// edges: the surface freeze reads every entry once.
    pub fn approvals(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }
}
