//! The records behind a parked approval: the pending bytes, the decision that
//! answers them, and the listing that names what waits.
//!
//! The store holds two shapes under two keys: the pending record under the
//! parking step's key, and the decision under the `<path>/decision` child
//! key, so the payload and the verdict never overwrite one another. Counting
//! lives with the store's index (see `task::store`): pending appends
//! increment their tenant, decisions decrement, so the park ceiling is one
//! probe however large the store is.

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use lgwks_std::hash::Digest;
use lgwks_std::wire::{Archive, Deserialize, Serialize};

use crate::effect::RunId;
#[cfg(feature = "edge")]
use crate::task::{DefinitionIdentity, RunStore};

#[cfg(feature = "edge")]
use super::approval::approval_digest;
use super::approval::{MAX_AUDIT_ENTRIES, nanos_of};
use super::run_store::Durable;
#[cfg(feature = "edge")]
use super::run_store::RunRecords;
use super::scope::step_key;
use super::{FlowError, StepKey};

/// A parked approval, as the run store holds it.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) struct PendingRecord {
    /// The tenant that parked it, and the only one that may decide it.
    pub(crate) tenant: String,
    /// The run that parked it.
    pub(crate) run: RunId,
    /// The step path that parked it, for attribution.
    pub(crate) path: String,
    /// The written label naming the approval.
    pub(crate) name: String,
    /// The digest the decision must match, over the archived payload.
    pub(crate) digest: Digest,
    /// The payload's bytes, as the block produced them.
    pub(crate) payload: Vec<u8>,
    /// When the approval parked, in nanoseconds on the host's clock.
    pub(crate) parked_at_nanos: u64,
    /// How long the decision is owed, in nanoseconds.
    pub(crate) ttl_nanos: u64,
}

/// A decision on a parked approval, as the run store holds it.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) struct DecisionRecord {
    /// The pending step key this decision answers, in hex.
    pub(crate) pending_key_hex: String,
    /// Whether the payload is released.
    pub(crate) approved: bool,
    /// Whether the approval had lapsed when it was decided.
    pub(crate) expired: bool,
    /// The digest the decision was made against.
    pub(crate) digest: Digest,
    /// Who decided, as the decision named them.
    pub(crate) by: String,
}

/// One approval record, either side of the decision.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) enum ApprovalRecord {
    /// A parked approval, awaiting its decision.
    Pending(PendingRecord),
    /// The decision a park received.
    Decision(DecisionRecord),
}

/// What the bytes under an approval step key hold.
pub(crate) enum RecordClass {
    /// A parked approval, awaiting its decision.
    Pending(PendingRecord),
    /// The decision a park received.
    Decision(DecisionRecord),
    /// Anything else: not an approval record.
    Other,
}

/// Read approval bytes as what they are: pending, decided, or neither.
///
/// A refusal to decode is `Other` rather than an error, so a step key that
/// holds another record entirely — a `remember` under the same path — reads
/// as foreign rather than as a corrupt approval.
pub(crate) fn classify_record(bytes: &[u8]) -> RecordClass {
    match ApprovalRecord::from_record(bytes) {
        Ok(ApprovalRecord::Pending(pending)) => RecordClass::Pending(pending),
        Ok(ApprovalRecord::Decision(decision)) => RecordClass::Decision(decision),
        Err(_) => RecordClass::Other,
    }
}

/// The step key a decision for the approval parked at `path` is filed under.
///
/// A child key of the parking step, so a run that parks and is decided keeps
/// both facts: the payload the decision was made against and the decision
/// itself never overwrite one another.
pub(crate) fn decision_key_for(tenant: &str, path: &str) -> StepKey {
    step_key(tenant, &format!("{path}/decision"))
}

// ── ApprovalId ──────────────────────────────────────────────────────────────

/// Which parked approval a decision is for: the run that parked it and the
/// step key it parked under.
///
/// Rendered `<run>:<step>` in hex, the form the parked report carries and the
/// estate CLI takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApprovalId {
    /// The run that parked it.
    run: RunId,
    /// The step key it parked under.
    key: StepKey,
}

impl ApprovalId {
    /// The id for `run`'s approval parked under `key`.
    pub(crate) fn of(run: RunId, key: StepKey) -> Self {
        Self { run, key }
    }

    /// The run that parked it, under which both the pending and the decision
    /// records are keyed; resumes name this run to replay the same records.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }

    /// The step key it parked under.
    #[must_use]
    pub fn key(&self) -> StepKey {
        self.key
    }

    /// Read `<run>:<step>` in hex as the approval it names.
    ///
    /// # Errors
    ///
    /// [`DecisionError::Unknown`] when the text is not an id: a malformed id
    /// names no pending approval.
    pub fn parse(text: &str) -> Result<Self, DecisionError> {
        let unknown = || DecisionError::Unknown {
            id: text.to_owned(),
        };
        let (run, key) = text.split_once(':').ok_or_else(unknown)?;
        let run = RunId::from_hex(run).map_err(|error| {
            lgwks_std::trace::debug!(error = %error, "ApprovalId::parse: the run half is not a run id");
            unknown()
        })?;
        let digest = Digest::from_hex(key).map_err(|error| {
            lgwks_std::trace::debug!(error = %error, "ApprovalId::parse: the step half is not a digest");
            unknown()
        })?;
        Ok(Self {
            run,
            key: StepKey::from_digest(digest),
        })
    }
}

impl fmt::Display for ApprovalId {
    /// `<run>:<step>`, each in hex.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}",
            self.run.id().to_hex(),
            self.key.to_hex()
        )
    }
}

// ── Decision ────────────────────────────────────────────────────────────────

/// How a parked approval is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApprovalVerdict {
    /// Release the payload to the parked run.
    Approve,
    /// Refuse the payload; the resume fails typed.
    Deny,
}

/// Who decided a parked approval, how, and against which bytes.
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    /// Who decided, for the audit and the approval.
    by: String,
    /// Whether the payload is released.
    verdict: ApprovalVerdict,
    /// The digest the decision is made against: the pending one, or refused.
    digest: Digest,
}

impl ApprovalDecision {
    /// Approve the payload with `digest`, decided by `by`.
    #[must_use]
    pub fn approve(by: &str, digest: Digest) -> Self {
        Self {
            by: by.to_owned(),
            verdict: ApprovalVerdict::Approve,
            digest,
        }
    }

    /// Deny the payload with `digest`, decided by `by`.
    #[must_use]
    pub fn deny(by: &str, digest: Digest) -> Self {
        Self {
            by: by.to_owned(),
            verdict: ApprovalVerdict::Deny,
            digest,
        }
    }

    /// Who decided, for the audit and the approval.
    #[must_use]
    pub fn by(&self) -> &str {
        &self.by
    }

    /// Whether the payload is released.
    #[must_use]
    pub fn verdict(&self) -> ApprovalVerdict {
        self.verdict
    }

    /// The digest the decision is made against.
    #[must_use]
    pub fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Why a decision could not be recorded.
#[derive(Debug)]
#[non_exhaustive]
pub enum DecisionError {
    /// The decision names bytes that are not the pending payload's.
    DigestMismatch {
        /// The digest the pending approval holds, in hex.
        want: String,
        /// The digest the decision named, in hex.
        got: String,
    },
    /// The approval already has a recorded decision.
    AlreadyDecided {
        /// The approval's id.
        id: String,
    },
    /// The approval belongs to another tenant.
    ForeignTenant {
        /// The tenant that parked it.
        owner: String,
        /// The tenant that asked to decide it.
        asked: String,
    },
    /// The approval lapsed before it was decided; the lapse is recorded.
    Expired {
        /// The approval's id.
        id: String,
    },
    /// No pending approval reads as this id.
    Unknown {
        /// The id that was asked for.
        id: String,
    },
    /// The store refused the read or the write behind the decision.
    Store {
        /// The store's own refusal.
        source: Box<FlowError>,
    },
}

impl fmt::Display for DecisionError {
    /// What was decided, and why it could not be recorded.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::DigestMismatch { ref want, ref got } => write!(
                formatter,
                "the decision names digest {got}, but the pending approval holds {want}; refusing to decide other bytes"
            ),
            Self::AlreadyDecided { ref id } => {
                write!(formatter, "approval {id} already has a recorded decision")
            }
            Self::ForeignTenant {
                ref owner,
                ref asked,
            } => write!(
                formatter,
                "approval belongs to tenant {owner:?}, not {asked:?}; refusing to decide another tenant's approval"
            ),
            Self::Expired { ref id } => write!(
                formatter,
                "approval {id} lapsed before it was decided; the lapse is recorded"
            ),
            Self::Unknown { ref id } => {
                write!(formatter, "no pending approval reads as {id:?}")
            }
            Self::Store { ref source } => {
                write!(formatter, "the run store refused the decision: {source}")
            }
        }
    }
}

impl std::error::Error for DecisionError {
    /// The store's own refusal, when there is one.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Store { ref source } => Some(&**source),
            _ => None,
        }
    }
}

// ── PendingApproval ─────────────────────────────────────────────────────────

/// One parked approval awaiting its decision, as [`Host::pending`](crate::task::Host::pending)
/// lists it.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// The approval's id, `<run>:<step>` in hex.
    id: String,
    /// The tenant that parked it.
    tenant: String,
    /// The run that parked it.
    run: RunId,
    /// The step path that parked it.
    path: String,
    /// The written label naming the approval.
    name: String,
    /// The digest a decision must match, over the archived payload.
    digest: Digest,
    /// The payload's length in bytes.
    payload_len: usize,
    /// When the decision is owed by, on the host's clock.
    expires_at: Duration,
}

impl PendingApproval {
    /// The approval's id, `<run>:<step>` in hex.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The tenant that parked it, and the only tenant that may decide it;
    /// any other tenant is refused as ForeignTenant and the attempt is audited.
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The run that parked it, whose pending and decision records the pending
    /// listing reads under; a resume names this run to return the approved payload.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }

    /// The step path that parked it.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The written label naming the approval.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The digest a decision must match, over the archived payload.
    #[must_use]
    pub fn digest(&self) -> &Digest {
        &self.digest
    }

    /// The archived payload's length in bytes, as written; the pending listing
    /// copies it for budgeting while a decision checks the digest, never this length.
    #[must_use]
    pub fn payload_len(&self) -> usize {
        self.payload_len
    }

    /// When the decision is owed by, on the host's clock.
    #[must_use]
    pub fn expires_at(&self) -> Duration {
        self.expires_at
    }
}

// ── AuditEntry ──────────────────────────────────────────────────────────────

/// One refused foreign-tenant decision, as [`Host::audit`](crate::task::Host::audit)
/// reads it back.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// When the attempt landed, on the host's clock.
    at: Duration,
    /// The tenant that asked to decide.
    tenant: String,
    /// The approval id that was asked for.
    id: String,
    /// The tenant that parked it.
    owner: String,
}

impl AuditEntry {
    /// When the attempt landed, on the host's clock.
    #[must_use]
    pub fn at(&self) -> Duration {
        self.at
    }

    /// The tenant that asked to decide.
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The approval id that was asked for.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The tenant that parked it.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Record a refused foreign-tenant decision.
    pub(crate) fn foreign(at: Duration, tenant: &str, id: &str, owner: &str) -> Self {
        Self {
            at,
            tenant: tenant.to_owned(),
            id: id.to_owned(),
            owner: owner.to_owned(),
        }
    }
}

/// The audit of refused foreign-tenant decisions one host keeps, newest last.
pub(crate) struct AuditLog {
    /// The attempts, oldest first; bounded by the host's ceiling.
    entries: VecDeque<AuditEntry>,
}

impl AuditLog {
    /// No attempts recorded.
    pub(crate) fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    /// Record `entry`, dropping the oldest past the bound.
    pub(crate) fn record(&mut self, entry: AuditEntry) {
        self.entries.push_back(entry);
        while self.entries.len() > MAX_AUDIT_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// Every recorded attempt, oldest first.
    pub(crate) fn entries(&self) -> Vec<AuditEntry> {
        self.entries.iter().cloned().collect()
    }
}

/// The pending approvals one tenant holds.
///
/// The listing [`Host::pending`](crate::task::Host::pending) answers: every
/// undecided, unexpired pending record for `tenant`, up to `limit`, each with
/// the id a decision names. `decided` probes the decision key beside each
/// pending record, so a decision that lands mid-listing is not listed as
/// pending.
pub(crate) fn pending_for(
    scanned: Vec<(RunId, String, String, Vec<u8>)>,
    decide: &dyn Fn(RunId, &str) -> Result<bool, FlowError>,
    tenant: &str,
    limit: usize,
    now: Duration,
) -> Result<Vec<PendingApproval>, FlowError> {
    let mut out = Vec::new();
    for (run, key_hex, path, bytes) in scanned {
        if out.len() >= limit {
            break;
        }
        let RecordClass::Pending(pending) = classify_record(&bytes) else {
            continue;
        };
        let expires_at = pending.parked_at_nanos.saturating_add(pending.ttl_nanos);
        if nanos_of(now) >= expires_at {
            continue;
        }
        if decide(run, &pending.path)? {
            continue;
        }
        let key = Digest::from_hex(&key_hex)
            .map(StepKey::from_digest)
            .map_err(|cause| {
                FlowError::failed(format!("a pending approval's key is not a digest: {cause}"))
            })?;
        out.push(PendingApproval {
            id: ApprovalId::of(run, key).to_string(),
            tenant: tenant.to_owned(),
            run,
            path,
            name: pending.name.clone(),
            digest: pending.digest,
            payload_len: pending.payload.len(),
            expires_at: Duration::from_nanos(expires_at),
        });
    }
    Ok(out)
}

/// Park an approval named `what` for `tenant` outside any run, decided
/// through the same path as a flow's park.
///
/// The door an app's `AwaitApproval` effect takes when the app is served with
/// a store: the pending id is deterministic in the tenant and the naming
/// text, so a second park of the same wait re-parks the same id rather than
/// opening a second decision. The payload is the naming text itself, owed for
/// `limit`.
///
/// Edge-gated, like the app that parks through it: without the app there is
/// no caller, and an ungated helper would be dead code on a host that never
/// serves one.
///
/// # Errors
///
/// Whatever the store refuses.
#[cfg(feature = "edge")]
pub(crate) fn park_app_approval(
    store: &RunStore,
    tenant: &str,
    what: &str,
    limit: Duration,
) -> Result<ApprovalId, FlowError> {
    let naming = approval_digest(format!("lgwks.bot/app-approval:{tenant}:{what}").as_bytes());
    let run = RunId::from_hex(&naming.to_hex()[..32]).map_err(|cause| {
        FlowError::failed(format!("an app approval's run could not be named: {cause}"))
    })?;
    let key = step_key(tenant, &format!("app/approval/{what}"));
    let id = ApprovalId::of(run, key);
    if let Some(stored) = store.lookup(tenant, run, key)?
        && matches!(classify_record(&stored), RecordClass::Pending(_))
    {
        return Ok(id);
    }
    let payload = what.as_bytes().to_vec();
    let archived = payload.to_record().map_err(|cause| {
        FlowError::failed(format!(
            "an app approval's payload could not be archived: {cause}"
        ))
    })?;
    let pending = PendingRecord {
        tenant: tenant.to_owned(),
        run,
        path: format!("app/approval/{what}"),
        name: what.to_owned(),
        digest: approval_digest(&archived),
        payload,
        parked_at_nanos: nanos_of(crate::script::Clock::wall().now()),
        ttl_nanos: nanos_of(limit),
    };
    let definition = DefinitionIdentity::new("approval", 1, naming, 1);
    let record = ApprovalRecord::Pending(pending);
    let bytes = record.to_record().map_err(|cause| {
        FlowError::failed(format!(
            "an app approval's record could not be archived: {cause}"
        ))
    })?;
    match store.append(
        tenant,
        run,
        key,
        &format!("app/approval/{what}"),
        &definition,
        bytes,
    )? {
        super::run_store::Appended::Recorded
        | super::run_store::Appended::AlreadyRecorded
        | super::run_store::Appended::Conflicting => Ok(id),
    }
}
