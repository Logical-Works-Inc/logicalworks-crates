//! `error` owns the kernel error vocabulary and enforces INV-BOT-ERROR-TYPED:
//! every failure is a distinct typed variant carrying the capability, field,
//! domain, or condition that caused it. Variants that surface untrusted
//! runtime text carry it as a `String` by design: the boundary is typed (which
//! domain failed is always known), while the foreign payload stays a `String`
//! so a structured consumer still has the original bytes. It is escaped where
//! it is *rendered*, by the `Escaped` adapter below, so a payload cannot forge
//! a log line or carry a live terminal control into whatever reads the file.
//! There is no bare-string failure with no typed envelope, and no arm that
//! interpolates a payload without that adapter.
//!
//! # One typed error per layer
//!
//! This module holds [`KernelError`](crate::error::KernelError) and nothing above it: its variants name
//! kernel and `std` types only, and no file under `error/` imports a module of
//! a higher layer (`core_boundary` refuses it). Each higher layer defines the
//! errors it raises where it raises them and wraps the layers below with a
//! `From` impl, so `?` does the wrap: `EcsError::Kernel`, `SessionError::Kernel`
//! and `FlowError::Bot` carry the kernel failure unchanged. The crate-wide
//! [`BotError`](crate::app_error::BotError) for a consumer lives in the app layer and is a
//! sum of those layer errors, never a flat list of variants. A lower layer never
//! names an upper error (ADR 0061).
//!
//! The vocabulary draws one distinction that a single failure variant cannot:
//! **whether the effect happened**.
//! [`KernelError::DomainError`](crate::error::KernelError::DomainError) means the action
//! did not take effect, so a retry is a retry.
//! [`KernelError::EffectIndeterminate`](crate::error::KernelError::EffectIndeterminate)
//! means it may have, so a retry is a possible duplicate. They are separate
//! variants rather than one variant carrying a flag because a consumer's retry
//! decision has to be readable from the type: a caller that must parse a cause
//! string to learn whether it may retry will eventually get it wrong, and the
//! price of getting it wrong is a duplicated merge, message, or process launch.
//!
//! "Did not take effect" is necessary but not sufficient for a retry decision,
//! and the second half is what [`DispatchCertainty`](crate::error::DispatchCertainty) carries. A refusal that
//! happened before dispatch and a connection that never opened are both "the
//! effect did not happen", and they do not want the same answer: the first is
//! permanent, the second is a plain retry. Every `DomainError` producer states
//! which of the two it is, and `retry_class` is the single total map from each
//! layer's vocabulary to [`RetryClass`](crate::error::RetryClass) that a retry policy reads. Every wrapper
//! variant delegates to the error it wraps, so a failure keeps its class through
//! every layer. No retry decision in this crate inspects a rendered cause: a
//! policy that turns on the wording of a diagnostic message is a policy that
//! changes when someone edits the message.

use std::time::Duration;

use super::broker::DispatchError;
use super::cap::Cap;
use super::cap::Deficit;
use super::effect::ActionId;

/// Error from authority, conflict, domain and dispatch failures: the kernel
/// layer's vocabulary.
///
/// `#[non_exhaustive]`: the vocabulary is expected to grow as domains gain
/// failure modes, and a consumer matching on it must keep a wildcard arm so a
/// new variant is a compile-time prompt there rather than a silent fallthrough.
///
/// # Examples
///
/// An effect that may or may not exist is never a plain retry.
///
/// ```
/// use lgwks_bot::error::{DispatchCertainty, KernelError, RetryClass};
///
/// let lost = KernelError::EffectIndeterminate {
///     domain: String::from("gh::issue"),
///     cause: String::from("timed out"),
/// };
/// assert_eq!(lost.retry_class(), RetryClass::RequiresEvidence);
///
/// let refused = KernelError::DomainError {
///     domain: String::from("gh::issue"),
///     certainty: DispatchCertainty::Refused,
///     cause: String::from("no such repository"),
/// };
/// assert_eq!(refused.retry_class(), RetryClass::Never);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum KernelError {
    /// One or more required capabilities were not granted.
    ///
    /// Carries the **whole** shortfall rather than its first element. A check
    /// that reported one missing capability at a time made admission a loop in
    /// which each pass revealed one more word, so a requirement list of length
    /// `n` cost `n` round trips to discover and the repair could not be written
    /// until the last of them. [`Deficit`] is what the check already computed,
    /// reported without discarding the rest of it, and
    /// [`Deficit::to_grant_set`] turns it back
    /// into the grant set that closes it.
    CapabilityDenied {
        /// Every requirement the check found ungranted, with the domain that
        /// declared each one where the check site knew it.
        deficit: Deficit,
    },
    /// The credential behind a proof expired, so the proof is worthless however
    /// much authority it still carries.
    ///
    /// The opposite of [`Self::CapabilityDenied`] rather than a variant of it: a
    /// deficit is closed by *granting*, and an expired credential is closed by
    /// **re-granting** — the capabilities are still held and now name nothing.
    /// A caller handed one arm for both would grant what it already has and be
    /// refused again, which is the admission loop `Deficit` exists to end, one
    /// layer out.
    ///
    /// Carries the capabilities and the two readings, so a caller can report how
    /// much life the credential had rather than only that it had none. `Refused`
    /// and so `RetryClass::Never`: presenting the same credential again produces
    /// the same refusal, and the repair is a new grant.
    CredentialExpired {
        /// Every capability the lapsed proof covered, in name order.
        capabilities: Vec<Cap>,
        /// The reading on the granting clock at which it expired.
        expired_at: Duration,
        /// The reading the check was made at.
        now: Duration,
    },
    /// Two editors wrote one record and this write lost (issue #278, row 5).
    ///
    /// The expected-version precondition an `Execute` writer of shared records
    /// carries: `expected` is the version the write saw, `found` is the version
    /// the store holds, and the two together are the whole fact a loser needs to
    /// re-read and try again. Recorded where the write was refused (the store's
    /// conflict log), never retried blindly: the same expectation against the
    /// same record gives the same refusal, so a retry without a re-read spends
    /// an attempt to learn nothing. `Refused` and so [`RetryClass::Never`].
    Conflict {
        /// The domain that refused the write (e.g. `"cas::store"`).
        domain: String,
        /// The record the write named.
        record: String,
        /// The version the write expected.
        expected: u64,
        /// The version the store holds.
        found: u64,
    },
    /// A domain action failed at runtime.
    ///
    /// Carries [`DispatchCertainty`] rather than leaving a consumer to infer
    /// the answer from the variant or from the cause. "It failed" is not the
    /// fact a retry decision turns on; "nothing reached the peer" and "the
    /// input was refused outright" are, and they are the same variant today
    /// without this field.
    DomainError {
        /// The domain that failed (e.g. `"gh::pr_status"`).
        domain: String,
        /// What the failure establishes about the effect. Required, so every
        /// producer states what it knows instead of defaulting to the
        /// retriable answer.
        certainty: DispatchCertainty,
        /// The underlying cause.
        cause: String,
    },
    /// A domain action failed *after* it may already have taken effect, so
    /// whether the effect occurred cannot be determined from the failure.
    ///
    /// The sibling of [`KernelError::DomainError`], and the difference is the
    /// whole point: `DomainError` means the effect did not happen, so a retry
    /// is a retry; this means it may have happened, so a retry is a possible
    /// duplicate. A consumer that retries every error it is handed will
    /// duplicate a merge, a message, or a process launch here, and it can only
    /// avoid that if the two are distinguishable without reading the cause.
    ///
    /// Reached when a request timed out, or a connection dropped, *after* the
    /// request was sent. `Execute` is the only verb with an effect, so the
    /// variant names no verb: it is an effect by construction.
    EffectIndeterminate {
        /// The domain whose action may or may not have taken effect.
        domain: String,
        /// Why the outcome could not be settled.
        cause: String,
    },
    /// `resolve_effect` was given a key for an action this bot does not declare.
    ///
    /// Distinct from the world layer's `NoSuchWork`, which is an action the
    /// bot *does* declare at an entry with nothing to settle. This one has no
    /// address to report at all, because the key's action does not appear in the
    /// declaration: the key came from somewhere other than
    /// `Bot::pending` — another bot's journal, or a
    /// journal this run does not own — and no evidence about it can be applied
    /// here.
    ActionNotDeclared {
        /// The action the key named.
        action: ActionId,
    },
    /// A fresh attempt on an action was refused because the journal records an
    /// earlier attempt on it as dispatched with no outcome.
    ///
    /// This is the one refusal in this enum that is a *recovery* answer rather
    /// than a caller mistake: the bot was reconstructed against a journal, the
    /// journal says an attempt on this action was prepared and nothing recorded
    /// what became of it, and the substrate will not begin another one. Nothing
    /// established that the bytes did not arrive, and a blind resend of a
    /// non-idempotent effect is exactly the duplicate the durable path exists
    /// to prevent.
    ///
    /// The repair is `Bot::resolve_effect`
    /// with the key from `Bot::pending`: `NotApplied` makes the entry eligible
    /// again, `Applied` records that the effect landed and the entry the key
    /// names is done for the generation the acknowledgement covers.
    EffectUnsettled {
        /// The action whose earlier attempt never settled.
        action: ActionId,
    },
    /// The broker or the journal refused a dispatch on its way out.
    ///
    /// Both halves arrive as one variant because they are refused at one
    /// instant — the handoff — and a caller's next move is the same for either:
    /// nothing left the process, and the tick reports it rather than retrying
    /// blind. [`DispatchError`] is where the two causes are told apart.
    EffectRefused {
        /// Which half refused, and why.
        cause: DispatchError,
    },
}

mod class;
mod display;

pub(crate) use class::unbound_domain;
pub use class::{DispatchCertainty, RetryClass};
pub(crate) use display::Escaped;

#[cfg(test)]
mod tests;
