//! `await approval`: park for a human decision, resume on it.
//!
//! A flow names what it wants done and parks: the block's bytes are the
//! payload, archived and digested exactly as an `act` archives its input, and
//! the pending record is committed through the run's store. The run then ends
//! [`Parked`](super::FlowError::Parked), holding no task, no thread and no
//! timer. A resume finds the pending record and either parks again (still
//! undecided), returns the [`Approved`] payload (approved), or fails typed
//! (denied, lapsed).
//!
//! # Why the payload block runs inside the step
//!
//! The macro hands the block to [`approval_at`] as a closure, so the payload
//! is computed only when no pending record exists: a resume replays the
//! recorded payload and never re-polls the body, the same once-per-run rule
//! [`remember`](super::remember) keeps. A payload that is recomputed on every
//! resume could not be the bytes a decision was made against.
//!
//! # What binds a decision to its bytes
//!
//! [`approval_digest`] digests the archived payload, and an `act` under a
//! gating [`EffectPolicy`] archives its input the
//! same way before it compares. A decision names the digest it was made
//! against, and [`Host::decide`](crate::task::Host::decide) refuses a digest
//! that is not the pending one, so an approval can never release different
//! bytes than the ones that were parked.
//!
//! # Bounds
//!
//! At most [`MAX_PENDING_PER_TENANT`] undecided approvals per tenant by
//! default ([`HostBuilder::max_pending_approvals`](crate::task::HostBuilder::max_pending_approvals)
//! lowers or raises it); a park past the ceiling is refused at admission,
//! before the payload block runs. Expiry is the `within` the word declared,
//! measured on the host's clock, never wall time: a resume past the deadline
//! records the lapse and fails [`ApprovalExpired`](super::FlowError::ApprovalExpired).
//!
//! [`Host::decide`](crate::task::Host::decide) and
//! [`Host::pending`](crate::task::Host::pending) are the only doors that read
//! and write these records; the cell's `AwaitApproval` effect parks through
//! the same records when the app is served with a store, so one decision path
//! serves flows and apps alike.

use std::future::Future;
use std::sync::Arc;

use lgwks_std::hash::{Digest, Hasher};

use crate::effect::RunId;
use crate::journal::DurabilityPromise;
use crate::journal::frame::SaturatingFrom;
use crate::task::{StoreError, StoreLimitKind};

use super::run_store::{Durable, installed, step_definition};
use super::{FlowError, Scope, Site, StepKey};

pub(crate) use super::approval_policy::installed_policy;
pub use super::approval_policy::{
    DefaultPolicy, EffectDisposition, EffectPolicy, with_effect_policy,
};
#[cfg(feature = "edge")]
pub(crate) use super::approval_records::park_app_approval;
pub use super::approval_records::{
    ApprovalDecision, ApprovalId, ApprovalVerdict, AuditEntry, DecisionError, PendingApproval,
};
pub(crate) use super::approval_records::{
    ApprovalRecord, AuditLog, DecisionRecord, PendingRecord, RecordClass, classify_record,
    decision_key_for, pending_for,
};

/// The default ceiling on undecided approvals per tenant.
///
/// A bound on live decisions, not on records: decided and lapsed approvals
/// leave the count when their decision lands, so a tenant that decides what
/// it parks never fills it.
pub const MAX_PENDING_PER_TENANT: usize = 10_000;

/// The highest ceiling [`HostBuilder::max_pending_approvals`](crate::task::HostBuilder::max_pending_approvals)
/// accepts.
///
/// The same order as the run store's own per-run ceiling: past it the number
/// is a budget in name only.
pub const MAX_PENDING_APPROVALS: usize = 65_536;

/// How many refused foreign-tenant decisions one host's audit ring retains.
///
/// A report, not a store: the audit names attempts, and an unbounded memory of
/// other tenants' attempts is a leak with their names in it.
pub(crate) const MAX_AUDIT_ENTRIES: usize = 1_024;

/// The digest of an approval's archived payload.
///
/// Framed under its own label, so an approval digest and a proposal digest
/// over the same bytes differ: the two answer different questions — "what was
/// decided" and "what was admitted" — and one digest must never settle the
/// other.
///
/// `pub(crate)` because a gated `act` digests its archived input the same
/// way before it compares: one function, so the two sides cannot disagree
/// about what "the same bytes" means.
pub(crate) fn approval_digest(archived: &[u8]) -> Digest {
    let mut hasher = Hasher::new();
    hasher.write_framed(b"lgwks.bot/approval.v1");
    hasher.write_framed(archived);
    hasher.finalize()
}

/// A duration as whole nanoseconds, saturating rather than wrapping past
/// `u64`'s range: an expiry past the range is an approval that never lapses,
/// not one that lapsed before it parked.
///
/// `pub(crate)` because the host reads the same clock the step parked on:
/// one conversion, so a park, a replay and a decision cannot disagree about
/// what "past the deadline" means.
pub(crate) fn nanos_of(duration: std::time::Duration) -> u64 {
    u64::saturating_from(duration.as_nanos())
}

/// The refusal when there is nowhere to park: no store, or a store that does
/// not survive its writer.
///
/// A parked approval whose record could not survive the process is a run that
/// can never be resumed and a decision that can never be recorded, so the
/// step is refused at admission rather than parked into nothing.
fn promise_refusal() -> FlowError {
    FlowError::from(StoreError::PromiseUnmet {
        required: DurabilityPromise::ProcessCrash,
        offered: DurabilityPromise::Ephemeral,
    })
}

// ── Approved ────────────────────────────────────────────────────────────────

/// What a decided approval hands the flow: the payload the decision was made
/// against, and who made it.
///
/// Held by the run that parked it, so an `act` the policy gates can match the
/// digest of its archived input without reading the store again.
#[derive(Debug, Clone)]
pub struct Approved {
    /// The digest the decision matched, over the archived payload.
    digest: Digest,
    /// The payload's bytes, as parked.
    payload: Vec<u8>,
    /// Who decided, as the decision named them.
    by: String,
}

impl Approved {
    /// The digest the decision matched, over the archived payload.
    #[must_use]
    pub fn digest(&self) -> &Digest {
        &self.digest
    }

    /// The payload's bytes, as parked: the input a gated `act` runs on.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Who decided, as the decision named them.
    #[must_use]
    pub fn approved_by(&self) -> &str {
        &self.by
    }

    /// The approval a replayed decision carries.
    pub(crate) fn of(digest: Digest, payload: Vec<u8>, by: String) -> Self {
        Self {
            digest,
            payload,
            by,
        }
    }
}

// ── The step ────────────────────────────────────────────────────────────────

/// Park for a human decision over `payload`, decided within `limit`.
///
/// The direct-call spelling of the word: the payload is already bytes, and
/// the step parks under `step` with the written label `name`. A resume after
/// the decision returns the [`Approved`] payload without re-parking; a resume
/// while it is undecided parks again under the same id.
///
/// # Errors
///
/// [`FlowError::Store`] carrying [`StoreError::PromiseUnmet`](crate::task::StoreError)
/// with no store or an ephemeral one; [`FlowError::InvalidBound`] for a zero
/// limit; the store's own refusal; [`FlowError::Parked`] once the pending
/// record is committed; [`FlowError::ApprovalDenied`] and
/// [`FlowError::ApprovalExpired`] on a resume that finds those.
pub async fn approval(
    scope: &Scope,
    step: &str,
    name: &str,
    payload: &[u8],
    limit: std::time::Duration,
) -> Result<Approved, FlowError> {
    let child = scope.enter(step)?;
    let bytes = payload.to_vec();
    let outcome = approval_in(&child, name, limit, move |_scope: Scope| async move {
        Ok::<_, FlowError>(bytes)
    })
    .await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// [`approval`](fn@approval) for an `await approval <label> within <duration>:` that
/// `script!` wrote at `site`, with the block as the payload's bytes.
///
/// The block runs only when no pending record exists, so a resume replays the
/// recorded payload and never re-polls it. Called by `script!`.
///
/// # Errors
///
/// As [`approval`](fn@approval), located at the step.
pub async fn approval_at<F, Fut>(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    name: &str,
    limit: std::time::Duration,
    body: F,
) -> Result<Approved, FlowError>
where
    F: FnOnce(Scope) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, FlowError>>,
{
    let child = scope.enter_at(step, site)?;
    let outcome = approval_in(&child, name, limit, body).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// The park itself, in the step `scope` already entered.
async fn approval_in<F, Fut>(
    scope: &Scope,
    name: &str,
    limit: std::time::Duration,
    body: F,
) -> Result<Approved, FlowError>
where
    F: FnOnce(Scope) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, FlowError>>,
{
    if limit.is_zero() {
        let refusal = Err(FlowError::InvalidBound {
            what: "the approval deadline",
            value: 0,
            max: u64::MAX,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "approval_in: returning an error to the caller");
        return refusal;
    }
    let Some(records) = installed() else {
        let refusal = Err(promise_refusal());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "approval_in: returning an error to the caller");
        return refusal;
    };
    records
        .admit_external_handoff()
        .map_err(|refused| refused.located(scope))?;
    let Some(run) = scope.run() else {
        let refusal = Err(promise_refusal());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "approval_in: returning an error to the caller");
        return refusal;
    };
    let key = scope.record_key();
    let tenant = scope.tenant().as_str();
    // A pending record means this run parked here before: replay it rather
    // than running the payload block again.
    if let Some(stored) = records.lookup(tenant, run, key)? {
        return resume_parked(scope, &records, run, key, &stored).await;
    }
    // First visit: the ceiling is charged before the payload block runs, so a
    // run refused for pressure never computes bytes nobody will decide on.
    if let Some(count) = records.0.pending_count(tenant)? {
        let max = u64::saturating_from(scope.approval_max());
        if count >= max {
            let refusal = Err(FlowError::from(StoreError::Limit {
                kind: StoreLimitKind::PendingApprovals,
                requested: count.saturating_add(1),
                limit: max,
            })
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "approval_in: returning an error to the caller");
            return refusal;
        }
    }
    let payload = body(scope.clone())
        .await
        .map_err(|error| error.located_at(scope.shared_path()))?;
    let archived = payload.to_record().map_err(|cause| {
        FlowError::failed(format!(
            "this approval's payload could not be archived: {cause}"
        ))
    })?;
    let digest = approval_digest(&archived);
    let parked_at = nanos_of(scope.clock().now());
    let ttl = nanos_of(limit);
    let pending = PendingRecord {
        tenant: tenant.to_owned(),
        run,
        path: scope.path().to_owned(),
        name: name.to_owned(),
        digest,
        payload,
        parked_at_nanos: parked_at,
        ttl_nanos: ttl,
    };
    let definition = step_definition(scope, name);
    let record = ApprovalRecord::Pending(pending.clone());
    let bytes = record.to_record().map_err(|cause| {
        FlowError::failed(format!(
            "this approval's record could not be archived: {cause}"
        ))
    })?;
    records
        .record(tenant, run, key, scope.path(), &definition, bytes)
        .await?;
    Err(parked_error(scope, run, key, &pending))
}

/// The terminal park for `pending`: committed, holding nothing.
fn parked_error(scope: &Scope, run: RunId, key: StepKey, pending: &PendingRecord) -> FlowError {
    FlowError::Parked {
        at: Arc::clone(scope.shared_path()),
        id: ApprovalId::of(run, key).to_string(),
        digest: pending.digest.to_hex(),
        expires_at: pending.parked_at_nanos.saturating_add(pending.ttl_nanos),
    }
}

/// Replay a step whose pending record is already committed.
async fn resume_parked(
    scope: &Scope,
    records: &super::run_store::Records,
    run: RunId,
    key: StepKey,
    stored: &super::run_store::StoredValue,
) -> Result<Approved, FlowError> {
    let pending = match classify_record(stored.bytes()) {
        RecordClass::Pending(pending) => pending,
        RecordClass::Decision(_) | RecordClass::Other => {
            let refusal = Err(FlowError::failed(
                "the record for this step is not a pending approval, so this resume cannot decide it",
            )
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "resume_parked: returning an error to the caller");
            return refusal;
        }
    };
    let tenant = scope.tenant().as_str();
    // A recorded decision stands, decided before any deadline is consulted: a
    // decision made in time answers even a resume that arrives late.
    let decision_key = decision_key_for(tenant, &pending.path);
    if let Some(decided) = records.lookup(tenant, run, decision_key)? {
        return follow_decision(scope, run, key, &pending, &decided);
    }
    // Undecided: the deadline is owed on the host's clock, and a resume past
    // it records the lapse so the ceiling reflects only live approvals.
    let expires_at = pending.parked_at_nanos.saturating_add(pending.ttl_nanos);
    if nanos_of(scope.clock().now()) >= expires_at {
        return lapse(scope, records, run, key, &pending).await;
    }
    Err(parked_error(scope, run, key, &pending))
}

/// Answer a replay from its recorded decision.
fn follow_decision(
    scope: &Scope,
    run: RunId,
    key: StepKey,
    pending: &PendingRecord,
    decided: &super::run_store::StoredValue,
) -> Result<Approved, FlowError> {
    let decision = match classify_record(decided.bytes()) {
        RecordClass::Decision(decision) if decision.pending_key_hex == key.to_hex() => decision,
        RecordClass::Decision(_) | RecordClass::Pending(_) | RecordClass::Other => {
            let refusal = Err(FlowError::failed(
                "the record beside this approval is not its decision, so this resume cannot follow it",
            )
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "follow_decision: returning an error to the caller");
            return refusal;
        }
    };
    if decision.digest != pending.digest {
        let refusal = Err(FlowError::failed(
            "the recorded decision names a different payload digest, so this resume cannot follow it",
        )
        .located(scope));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "follow_decision: returning an error to the caller");
        return refusal;
    }
    if decision.expired {
        let refusal = Err(FlowError::ApprovalExpired {
            at: Arc::clone(scope.shared_path()),
            id: ApprovalId::of(run, key).to_string(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "follow_decision: returning an error to the caller");
        return refusal;
    }
    if !decision.approved {
        let refusal = Err(FlowError::ApprovalDenied {
            at: Arc::clone(scope.shared_path()),
            id: ApprovalId::of(run, key).to_string(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "follow_decision: returning an error to the caller");
        return refusal;
    }
    let approved = Approved::of(pending.digest, pending.payload.clone(), decision.by);
    scope.hold_approval(approved.clone());
    Ok(approved)
}

/// Record the lapse of an undecided approval past its deadline, and fail it.
async fn lapse(
    scope: &Scope,
    records: &super::run_store::Records,
    run: RunId,
    key: StepKey,
    pending: &PendingRecord,
) -> Result<Approved, FlowError> {
    let tenant = scope.tenant().as_str();
    let decision = DecisionRecord {
        pending_key_hex: key.to_hex(),
        approved: false,
        expired: true,
        digest: pending.digest,
        by: String::from("expiry"),
    };
    let definition = step_definition(scope, &pending.name);
    let record = ApprovalRecord::Decision(decision);
    let bytes = record.to_record().map_err(|cause| {
        FlowError::failed(format!(
            "this approval's lapse could not be archived: {cause}"
        ))
    })?;
    let decision_key = decision_key_for(tenant, &pending.path);
    match records
        .claim(
            tenant,
            run,
            decision_key,
            &format!("{}/decision", pending.path),
            &definition,
            bytes,
        )
        .await?
    {
        super::run_store::Appended::Recorded | super::run_store::Appended::AlreadyRecorded => {}
        super::run_store::Appended::Conflicting => {
            // A decision landed beside the lapse: follow it rather than the
            // clock, because a human answer beats a deadline.
            if let Some(decided) = records.lookup(tenant, run, decision_key)? {
                return follow_decision(scope, run, key, pending, &decided);
            }
        }
    }
    let refusal = Err(FlowError::ApprovalExpired {
        at: Arc::clone(scope.shared_path()),
        id: ApprovalId::of(run, key).to_string(),
    });
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "lapse: returning an error to the caller");
    refusal
}
