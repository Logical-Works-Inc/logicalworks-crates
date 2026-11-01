//! `send`: one outbound effect, dispatched once.
//!
//! A `send` hands bytes to a receiver — mail, a webhook, a payment — through
//! a [`SendPort`], and the port is the only thing that dispatches: the word
//! never dials out itself, so the tests install a stub and no test can send a
//! real effect. Two records make it once-only, both written once and never
//! updated, so every replay is idempotent:
//!
//! - the *intent*, claimed through the run store before anything dispatches. A
//!   `send` that dispatches before its intent is durable is a defect, and a
//!   storeless run is refused rather than run ungated;
//! - the *dispatched marker*, claimed after every dispatch that may have
//!   landed, including an applied one. A resume that finds the marker never
//!   dispatches again.
//!
//! The outcome is never stored: it is read from the receiver every time, so a
//! `send` is settled from [`EffectEvidence`] and nothing else. An effect whose
//! outcome is unknown after a crash stays unknown until evidence settles it:
//! a resume that finds the marker never dispatches again, and a resume that
//! finds evidence settles on it. Nothing dispatches twice and nothing is
//! assumed sent or unsent.
//!
//! A `send` is an external act, so the [`EffectPolicy`](super::EffectPolicy)
//! governs it: under the default policy it needs a matching `await approval`
//! over the payload bytes, exactly like an `act` whose effect leaves the
//! process.
//!
//! # Errors
//!
//! The word's own failures are [`FlowError`] arms: [`FlowError::Blocked`]
//! without the `bot.notify` grant, [`FlowError::Store`] with no store or past
//! an ephemeral one, [`FlowError::NeedsApproval`] when the policy gates the
//! effect and no approval matches, and [`FlowError::Failed`] when a resumed
//! payload differs from the recorded intent or a record decodes as something
//! else. A port failure the port reports as landed-nothing reaches the caller
//! as the port's typed error; a failure it reports as may-have-landed ends
//! the step [`Unknown`](SendOutcome::Unknown).

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::wire::{Archive, Deserialize, Serialize};

use crate::cap::Cap;
use crate::journal::EffectEvidence;
use crate::verb::EffectLifetime;

use super::domain::gate_approval;
use super::run_store::{Durable, installed, step_definition};
use super::scope::step_key;
use super::{FlowError, Scope, Site};

/// The receiver a `send` dispatches through.
///
/// Implemented by the effect's real receiver in production and by a stub in
/// the tests. `find` answers what the receiver holds under `key`; `dispatch`
/// hands `payload` over once under that key and answers what the receiver
/// reports. `may_have_landed` says whether the port's error leaves the effect
/// unknown: `false` proves nothing was handed over and the step fails typed,
/// `true` keeps the step [`Unknown`](SendOutcome::Unknown) until evidence
/// settles it.
pub trait SendPort: Sync {
    /// The port's own failure, answered as the flow's.
    type Error: Into<FlowError> + std::fmt::Debug;

    /// What the receiver holds under `key`, or `None` when it holds nothing.
    fn find(
        &self,
        key: &[u8],
    ) -> impl Future<Output = Result<Option<EffectEvidence>, Self::Error>> + Send;

    /// Hand `payload` over once under `key`.
    fn dispatch(
        &self,
        key: &[u8],
        payload: &[u8],
    ) -> impl Future<Output = Result<EffectEvidence, Self::Error>> + Send;

    /// Whether `error` leaves the effect unknown rather than unattempted.
    fn may_have_landed(error: &Self::Error) -> bool;
}

impl<P> SendPort for &P
where
    P: SendPort + Sync,
{
    /// The port's own failure, answered as the flow's.
    type Error = P::Error;

    /// Read through the shared reference.
    fn find(
        &self,
        key: &[u8],
    ) -> impl Future<Output = Result<Option<EffectEvidence>, Self::Error>> + Send {
        (*self).find(key)
    }

    /// Dispatch through the shared reference.
    fn dispatch(
        &self,
        key: &[u8],
        payload: &[u8],
    ) -> impl Future<Output = Result<EffectEvidence, Self::Error>> + Send {
        (*self).dispatch(key, payload)
    }

    /// The port's own answer.
    fn may_have_landed(error: &Self::Error) -> bool {
        P::may_have_landed(error)
    }
}

/// What one `send` step established.
///
/// Settled from [`EffectEvidence`]: `Settled` carries what the receiver
/// reports — [`Applied`](EffectEvidence::Applied) or
/// [`NotApplied`](EffectEvidence::NotApplied) — and `Unknown` carries the
/// honest fact that nothing is known. Unknown stays unknown until a later
/// `send` reads evidence that settles it; it is never upgraded by guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
#[non_exhaustive]
pub enum SendOutcome {
    /// The receiver's evidence: the effect landed, or it affirmatively did not.
    Settled(EffectEvidence),
    /// The dispatch may have landed and no evidence settles it yet.
    Unknown,
}

/// One intent: the digest of the payload the step was entered to send.
///
/// Written once, read back on every resume, never updated: a resumed payload
/// with a different digest is a different effect and is refused rather than
/// sent under the first one's intent.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
struct SendRecord {
    /// The digest of the payload, framed as an intent.
    digest: Digest,
}

/// The intent's framing label, so an intent digest never settles another
/// question asked over the same bytes.
const INTENT_DOMAIN: &[u8] = b"lgwks.bot.send-intent.v1";

/// The digest of `payload` as an intent.
fn intent_digest(payload: &[u8]) -> Digest {
    let mut hasher = Hasher::new();
    hasher.write_framed(INTENT_DOMAIN).write_framed(payload);
    hasher.finalize()
}

/// `send receiver with payload`: one outbound effect as a step of its own.
///
/// Enters the `send` step, checks the run holds `bot.notify` and the policy's
/// approval, claims the intent durably, dispatches at most once per run, and
/// settles from the receiver's evidence.
///
/// # Errors
///
/// As documented above.
///
/// # Examples
///
/// ```rust
/// use lgwks_bot::journal::EffectEvidence;
/// use lgwks_bot::script::{FlowError, SendOutcome, SendPort};
///
/// struct StubMailer;
///
/// impl SendPort for StubMailer {
///     type Error = FlowError;
///
///     async fn find(&self, _key: &[u8]) -> Result<Option<EffectEvidence>, FlowError> {
///         Ok(None)
///     }
///
///     async fn dispatch(&self, _key: &[u8], _payload: &[u8]) -> Result<EffectEvidence, FlowError> {
///         Ok(EffectEvidence::Applied)
///     }
///
///     fn may_have_landed(_error: &FlowError) -> bool {
///         false
///     }
/// }
///
/// lgwks_bot::script! {
///     pub flow mail(mailer: &StubMailer) -> SendOutcome:
///         let outcome = send mailer with (b"hello".to_vec())
///         give back outcome
/// }
///
/// # fn main() -> Result<(), FlowError> {
/// let scope = lgwks_bot::script::Scope::root(lgwks_bot::script::Tenant::new("acme")?);
/// let refused = lgwks_bot::block_on(mail(&scope, &StubMailer));
/// assert!(
///     matches!(refused, Err(FlowError::Store { .. })),
///     "a send with nowhere durable to record its intent is refused"
/// );
/// # Ok(())
/// # }
/// ```
pub async fn send<P>(
    scope: &Scope,
    step: &str,
    receiver: &P,
    payload: &[u8],
) -> Result<SendOutcome, FlowError>
where
    P: SendPort,
{
    let child = scope.enter(step)?;
    let outcome = send_in(&child, step, receiver, payload).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// [`send`] for a `send <receiver> with <payload>` line that `script!` wrote
/// at `site`. Called by `script!`.
///
/// # Errors
///
/// As [`send`], located at the step.
#[doc(hidden)]
pub async fn send_at<P>(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    receiver: &P,
    payload: &[u8],
) -> Result<SendOutcome, FlowError>
where
    P: SendPort,
{
    let child = scope.enter_at(step, site)?;
    let outcome = send_in(&child, step, receiver, payload).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// The send itself, in the step `scope` already entered.
async fn send_in<P>(
    scope: &Scope,
    step: &str,
    receiver: &P,
    payload: &[u8],
) -> Result<SendOutcome, FlowError>
where
    P: SendPort,
{
    if scope.justification().is_some() {
        Box::pin(scope.justified(0)).await?;
    }
    scope.require(&[Cap::notify()])?;
    let Some(records) = installed() else {
        let refusal = Err(promise_refusal());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "send_in: returning an error to the caller");
        return refusal;
    };
    records
        .admit_external_handoff()
        .map_err(|refused| refused.located(scope))?;
    let Some(run) = scope.run() else {
        let refusal = Err(promise_refusal());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "send_in: returning an error to the caller");
        return refusal;
    };
    let archived = payload.to_vec().to_record().map_err(|cause| {
        FlowError::failed(format!("this send payload could not be archived: {cause}"))
    })?;
    gate_approval(scope, EffectLifetime::External, Some(archived))?;
    let tenant = scope.tenant().as_str();
    let key = scope.record_key();
    let digest = intent_digest(payload);
    let definition = step_definition(scope, step);
    let stored = records.lookup(tenant, run, key)?;
    if let Some(record) = stored {
        same_intent(&record.into_bytes(), digest, scope)?;
    } else {
        let intent = SendRecord { digest };
        let bytes = intent.to_record().map_err(|cause| {
            FlowError::failed(format!("this send intent could not be archived: {cause}"))
        })?;
        match records
            .claim(tenant, run, key, scope.path(), &definition, bytes)
            .await?
        {
            super::run_store::Appended::Recorded => {}
            super::run_store::Appended::AlreadyRecorded
            | super::run_store::Appended::Conflicting => {
                let stored = records.lookup(tenant, run, key)?.ok_or_else(|| {
                    FlowError::failed("the send intent vanished between its claim and its read")
                })?;
                same_intent(&stored.into_bytes(), digest, scope)?;
            }
        }
    }
    settle_over(scope, &records, receiver, payload, &definition).await
}

/// Settle the send for an intent that is already durable: never dispatch when
/// the marker says one may have landed, settle from evidence when the
/// receiver holds it, and dispatch exactly once otherwise.
async fn settle_over<P>(
    scope: &Scope,
    records: &super::run_store::Records,
    receiver: &P,
    payload: &[u8],
    definition: &crate::task::DefinitionIdentity,
) -> Result<SendOutcome, FlowError>
where
    P: SendPort,
{
    let run = match scope.run() {
        Some(run) => run,
        None => {
            let refusal = Err(promise_refusal());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "settle_over: returning an error to the caller");
            return refusal;
        }
    };
    let tenant = scope.tenant().as_str();
    let key = scope.record_key();
    let marker_path = scope.join("dispatched");
    let marker_key = step_key(tenant, &marker_path);
    let idempotency = key.to_hex();
    if records.lookup(tenant, run, marker_key)?.is_some() {
        return match receiver.find(idempotency.as_bytes()).await {
            Ok(Some(evidence)) => Ok(SendOutcome::Settled(evidence)),
            Ok(None) => Ok(SendOutcome::Unknown),
            Err(failed) => {
                let refusal = Err(failed.into());
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "settle_over: the read-back failed");
                refusal
            }
        };
    }
    match receiver.find(idempotency.as_bytes()).await {
        Ok(Some(evidence)) => return Ok(SendOutcome::Settled(evidence)),
        Ok(None) => {}
        Err(failed) => {
            let refusal = Err(failed.into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "settle_over: the read-back failed");
            return refusal;
        }
    }
    let dispatched = match receiver.dispatch(idempotency.as_bytes(), payload).await {
        Ok(evidence) => Some(evidence),
        Err(failed) if P::may_have_landed(&failed) => {
            lgwks_std::trace::debug!(error = ?failed, "settle_over: the dispatch may have landed; claiming the marker");
            None
        }
        Err(failed) => {
            let refusal = Err(failed.into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "settle_over: the dispatch landed nothing");
            return refusal;
        }
    };
    // Claimed after every dispatch that may have landed — an applied one
    // included — so a resume that finds the marker never dispatches again:
    // it settles from evidence, or stays unknown until evidence settles it.
    let marker = SendRecord {
        digest: intent_digest(payload),
    };
    let bytes = marker.to_record().map_err(|cause| {
        FlowError::failed(format!("this send marker could not be archived: {cause}"))
    })?;
    match records
        .claim(tenant, run, marker_key, &marker_path, definition, bytes)
        .await?
    {
        super::run_store::Appended::Recorded
        | super::run_store::Appended::AlreadyRecorded
        | super::run_store::Appended::Conflicting => {}
    }
    match dispatched {
        Some(evidence) => Ok(SendOutcome::Settled(evidence)),
        None => Ok(SendOutcome::Unknown),
    }
}

/// The refusal when there is nowhere durable to record the intent: no store,
/// or a store that does not survive its writer.
///
/// An intent that cannot outlive the process is not an intent: after a crash
/// a recovered run could not tell "never sent" from "sent and forgotten", and
/// that is the state in which a blind resend happens.
fn promise_refusal() -> FlowError {
    FlowError::from(crate::task::StoreError::PromiseUnmet {
        required: crate::journal::DurabilityPromise::ProcessCrash,
        offered: crate::journal::DurabilityPromise::Ephemeral,
    })
}

/// Check that the recorded intent `bytes` is this payload's `digest`.
///
/// A resumed payload with another digest is a different effect, refused
/// rather than sent under the first one's intent; bytes that are not an
/// intent at all are refused with the decode error that says why.
fn same_intent(bytes: &[u8], digest: Digest, scope: &Scope) -> Result<(), FlowError> {
    let intent = match SendRecord::from_record(bytes) {
        Ok(intent) => intent,
        Err(cause) => {
            let refusal = Err(FlowError::failed(format!(
                "the record for this step is not a send intent ({cause}), so this resume \
                 cannot follow it"
            ))
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "same_intent: the record is not an intent");
            return refusal;
        }
    };
    if intent.digest == digest {
        return Ok(());
    }
    let refusal = Err(FlowError::failed(format!(
        "a different payload is already recorded for step {:?} under this run; refusing to \
         send under another intent",
        scope.path(),
    ))
    .located(scope));
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "same_intent: the resumed payload differs from the intent");
    refusal
}
