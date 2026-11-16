//! The world layer's error: what the four verbs, the spec, the registry and the
//! tick raise, above the kernel's own [`KernelError`].
//!
//! Defined where it is raised (INV-BOT-ERROR-TYPED, ADR 0061). The verb traits
//! return it, so a domain that implements a verb reports a kernel failure by
//! `?` or `.into()`: [`EcsError::Kernel`] carries it unchanged and `retry_class`
//! delegates, so wrapping never changes what a retry policy reads.

use std::time::Duration;

use super::{EffectEvidence, PendingWork, WorkId};
use crate::broker::DispatchError;
use crate::effect::{ActionDigest, AttemptId, EffectKey};
use crate::error::KernelError;
use crate::verb::Unstable;

/// Error from bot construction, admission, or execution above the kernel.
///
/// `#[non_exhaustive]`: the vocabulary is expected to grow as the substrate
/// gains failure modes, and a consumer matching on it must keep a wildcard arm
/// so a new variant is a compile-time prompt there rather than a silent
/// fallthrough.
///
/// # Examples
///
/// A kernel failure carried through `?` keeps its text.
///
/// ```
/// use lgwks_bot::ecs::EcsError;
/// use lgwks_bot::error::KernelError;
///
/// let kernel = KernelError::EffectIndeterminate {
///     domain: String::from("gh::issue"),
///     cause: String::from("timed out"),
/// };
/// let line = kernel.to_string();
/// let world = EcsError::from(kernel);
/// assert!(matches!(world, EcsError::Kernel(_)));
/// assert_eq!(world.to_string(), line);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum EcsError {
    /// A kernel failure, carried unchanged: authority, conflict, domain and
    /// dispatch refusals reach the world layer through this arm.
    Kernel(KernelError),
    /// The bot spec is incomplete: a required field is missing (currently the
    /// name; an empty chain list is allowed).
    ///
    /// `cause` is the detail the producing site refused with. It is required
    /// rather than optional because the field name alone does not say *why* the
    /// value was missing: a threshold that does not parse and a chain list that
    /// is absent are both "missing target", and only the cause tells a caller
    /// which one it is looking at. Rendered through `Escaped` because it
    /// originates in the spec document.
    IncompleteSpec {
        /// What is missing.
        field: &'static str,
        /// Why the field is missing, in the producing site's own words.
        ///
        /// Required rather than optional because the two ways this is reached
        /// are different defects with different repairs: a field that was absent
        /// from a document, and a field whose translation failed. It is untrusted
        /// -- it names something from the spec document -- so it renders through
        /// the escaping wrapper.
        ///
        /// A site with no underlying error states its own reason rather than
        /// leaving this empty; there is no "unknown" here, because a caller
        /// holding this is being told what to repair.
        cause: String,
    },
    /// The spec names a domain this binary does not run.
    ///
    /// The registry is the list of domains a binary declares at compile time —
    /// see `DomainRegistry` — so this is a mismatch between the
    /// document and the program it was handed to, not a runtime failure. The
    /// domain may exist and be registered elsewhere; it is simply not linked
    /// here, and no amount of retrying adds it. The identifier is carried as a
    /// `String` because it came from the document, and is escaped where it is
    /// rendered so a hostile spec cannot forge a log line with it.
    UnregisteredDomain {
        /// The identifier the spec named, spelled as the spec spelled it.
        domain: String,
    },
    /// The registry declares one identifier twice within a single role.
    ///
    /// A duplicate would make dispatch depend on declaration order: the first
    /// constructor would silently win and a spec would have no way to name the
    /// ambiguity. The registry therefore refuses to build anything until the
    /// lists are distinct — see `DomainRegistry::validate`. An
    /// identifier may still appear once as a source and once as an action: a
    /// domain that observes a repository and also acts on one is one domain
    /// with two roles, not a collision.
    DuplicateDomain {
        /// The identifier declared twice, spelled as the declaration spelled it.
        domain: String,
        /// Which declaration list holds the pair: `"source"` or `"action"`.
        role: &'static str,
        /// The first declaration position, zero-based.
        first: usize,
        /// The second declaration position, zero-based.
        second: usize,
    },
    /// The serialized spec exceeds [`crate::spec::MAX_SPEC_BYTES`]. The bound is
    /// defensive: it stops a hostile or runaway manifest from allocating
    /// without limit before any validation runs.
    SpecTooLarge {
        /// Observed length in bytes.
        bytes: usize,
        /// The applied bound.
        limit: usize,
    },
    /// The serialized spec is not valid JSON for the schema.
    MalformedSpec {
        /// The parser's positional diagnostic, with control characters escaped
        /// so an untrusted field name cannot forge a log line.
        cause: String,
    },
    /// The serialized spec declares a version this binary does not implement.
    ///
    /// A document is a versioned contract, not a schema to guess at: a spec
    /// written for a later materializer may name fields with meanings this one
    /// does not have, so it is refused before any of it is interpreted rather
    /// than partially understood. The found and supported versions are carried
    /// as numbers so a caller can decide the repair without parsing the
    /// rendering.
    UnsupportedSpecVersion {
        /// The version the document declared.
        found: u32,
        /// The version this build implements.
        supported: u32,
    },
    /// A spec named a condition identifier outside the supported vocabulary.
    ///
    /// Conditions are the one part of a chain that carries parameters (a
    /// threshold), and the vocabulary is therefore closed and versioned with the
    /// type it evaluates rather than open to arbitrary wire text. An identifier
    /// this build does not know is refused before the chain is built, never
    /// treated as an always-true condition — a silently inert gate is how an
    /// effect a person expected to be guarded fires anyway.
    UnknownCondition {
        /// The condition identifier the spec spelled.
        condition: String,
        /// The parse failure the registry recorded for the identifier's
        /// argument, rendered as the `FromStr` error's own text.
        ///
        /// It is required because the identifier alone cannot distinguish the
        /// two ways this is reached: an identifier outside the vocabulary, and
        /// an identifier in it whose threshold argument does not parse. A
        /// caller repairing a spec needs to know which. Held as a `String` and
        /// rendered through `Escaped`, exactly as [`EcsError::MalformedSpec`]
        /// holds its cause, so a payload that reaches it cannot forge a log
        /// line.
        argument_parse: String,
    },
    /// The synchronous adapter, `Bot::tick`, was called on a
    /// thread an async runtime is already driving.
    ///
    /// This is a refusal rather than a failure of the work: the adapter parks
    /// the calling thread until its verb futures resolve, and the thread it was
    /// handed may be the one that owns the runtime's driver. Parked, that
    /// driver cannot advance a timer, report a socket ready, or run the sibling
    /// task a verb is waiting on, so the tick would never return. The repair is
    /// at the call site and there are two of them: await the tick from inside
    /// the runtime (`Bot::tick_async`), or move the
    /// call out of the runtime.
    ///
    /// Reported even when the runtime has several worker threads, where parking
    /// one of them may happen to work: which thread the caller was handed is
    /// not knowable from the adapter, and a deadlock that only appears on a
    /// current-thread runtime is exactly what this refuses to have.
    ///
    /// Reachable with the default `rt` feature. Without it there is no engine
    /// runtime in the build (`--no-default-features` withdraws tokio entirely)
    /// and nothing for the adapter to detect, so this variant is documented
    /// rather than constructed.
    TickInsideRuntime,
    /// A source poll was cancelled by the bot's declared per-poll deadline
    /// before it resolved.
    ///
    /// Not a [`KernelError::DomainError`] because the domain reported nothing:
    /// the substrate stopped waiting, so it knows the poll did not finish and
    /// nothing at all about why it was slow. Carries the budget the poll was
    /// actually given, so a caller triaging the stall is reading the number the
    /// bot declared rather than one the report had to invent.
    ///
    /// `NotDelivered`, so a consumer's retry classifier reads it as safe: a
    /// cancelled observation commits nothing and no effect was attempted on its
    /// account.
    PollStalled {
        /// The chain whose source stopped answering, in declaration order.
        chain: usize,
        /// The budget the poll was given, as the bot declared it.
        deadline: Duration,
    },
    /// A source's subject was still moving when it was read, so no reading of
    /// it could be confirmed.
    ///
    /// The torn read, as a typed outcome rather than as a value. A source that
    /// read a document while another writer was half-way through it has seen
    /// bytes no one ever held as a whole; reporting that as the current state
    /// makes the substrate commit a value and fire a chain against a document
    /// that does not exist. So the reading is refused and the observation is
    /// **pending**: the chain keeps the value it already holds and reads again
    /// on the next tick, which is the same discipline a failed poll follows and
    /// for the same reason — the value that was to be replaced is still there.
    ///
    /// Its own variant rather than a [`KernelError::DomainError`] because the
    /// repair differs: nothing is wrong with the domain and no amount of
    /// retrying *this* call helps beyond the bound the read protocol already
    /// applied. A caller triaging a source that keeps reporting this is looking
    /// at a writer that never settles, which is a fact about the other side.
    ///
    /// `NotDelivered`, so a consumer's retry classifier reads it as safe: no
    /// effect was attempted and nothing was committed.
    UnstableObservation {
        /// The source that could not confirm a reading.
        domain: String,
        /// How many reads were taken, and which axis kept moving.
        unstable: Unstable,
    },
    /// The declared per-poll deadline was refused, because it would bound
    /// nothing.
    ///
    /// A zero budget cancels every poll before it is first polled, so a bot
    /// built with one observes nothing forever and reports every chain as
    /// stalled. That is a wiring mistake with a distinct repair, not a tuning
    /// choice, and it is refused at build rather than discovered on the first
    /// tick.
    PollDeadlineUnbounded {
        /// The zero that was asked for.
        deadline: Duration,
    },
    /// The declared per-poll deadline was refused, because it is above the
    /// ceiling this crate will grant.
    ///
    /// The ceiling exists because the deadline is the one bound standing between
    /// a source that stops answering and a bot that stops ticking. A budget past
    /// it converts a recoverable stall into a hang, and the caller who wants a
    /// longer one has to say why rather than discovering it at run time.
    PollDeadlineExceeded {
        /// The budget that was asked for.
        deadline: Duration,
        /// The largest budget that would have been accepted.
        ceiling: Duration,
    },
    /// A receiver refused the credential an adapter presented.
    ///
    /// Distinct from [`KernelError::CapabilityDenied`] because the authority
    /// *was* granted and the receiver rejected it anyway: its issuer cancelled,
    /// rotated or expired it upstream. The repair is carried in the error as a
    /// [`NeedSet`](crate::spec::NeedSet), so a caller reads what to re-grant
    /// rather than reconstructing it from a status code — and it is a `NeedSet`,
    /// not a string, so the whole shortfall arrives at once the way every other
    /// admission shortfall does.
    ///
    /// Never a retry. The same credential against the same receiver gives the
    /// same status, so this is the typed outcome that ends a refresh-then-retry
    /// loop rather than feeding one.
    CredentialRejected {
        /// The domain whose upstream refused the credential.
        domain: String,
        /// The status the receiver reported.
        status: u16,
        /// What a caller must re-grant for the call to be worth making again.
        needs: crate::spec::NeedSet,
    },
    /// An evaluate condition failed structurally: not a false result, but an
    /// error in the condition itself.
    EvaluateError {
        /// What went wrong.
        cause: String,
    },
    /// A value at the erasure boundary was handed to a stage that expects a
    /// different type.
    ///
    /// A wiring defect in the chain, not a domain failure, and this variant
    /// exists so the two cannot be confused for one another. The action never
    /// ran; no domain was reached; there is nothing to retry. It used to be
    /// reported as a [`KernelError::DomainError`], which read as "the domain
    /// failed" and — before certainty was carried — spent a whole retry
    /// budget on a defect no attempt could repair.
    ///
    /// The fields are all `&'static str` because this is a *constructive* error:
    /// it is caught where the concrete types are still in hand, so it can name
    /// them, and naming them is the whole diagnostic. It is never built from
    /// runtime text.
    TypeMismatch {
        /// Where it was caught, as a stable site name (`"spec::typed_entry"`,
        /// `"observe_fold rendezvous"`), so the loud report is also greppable.
        site: &'static str,
        /// The chain index, when the site knows which chain it was walking.
        chain: Option<usize>,
        /// The type the stage was built for.
        expected: &'static str,
        /// The type it was handed.
        observed: &'static str,
    },
    /// A tick finished with a transition still holding work.
    ///
    /// This is the outcome the ECS substrate used to express by *silence*: a
    /// tick that failed partway committed its revision, so an unchanged source
    /// never reached the entries the failure skipped, and a later clean tick was
    /// indistinguishable from a handled transition. The work is now retained
    /// and this variant is how its retention is reported — held for an attempt
    /// or for evidence, or given up on.
    PendingTransition {
        /// The entry that needs the caller: the first one still open, in
        /// `(chain, entry)` order.
        ///
        /// A tick that gave up on an entry reports the error that caused the
        /// abandonment instead, since that keeps the typed variant intact;
        /// `Bot::pending` is where abandoned entries are named.
        ///
        /// Boxed for the reason [`PendingWork`] boxes its key: the hold names
        /// when a backoff ends and how long a deadline ran, and inline those
        /// put this error, which every unfinished tick returns by value, over
        /// the size at which `result_large_err` fires for every caller.
        work: Box<PendingWork>,
        /// Entries across every chain that are still unresolved: open, or
        /// abandoned and still blocking the entries behind them.
        ///
        /// Abandoned entries are counted because they are counted by the scan
        /// that picked `work`: a count that omitted them could pair a named
        /// abandonment with `outstanding: 0`, which is the one reading this
        /// error exists to prevent — a chain that is somehow both stuck and
        /// finished.
        outstanding: usize,
    },
    /// `resolve_effect` was given an entry whose outcome is already decided.
    ///
    /// Not a no-op: a caller that believes it acknowledged an effect that
    /// nothing was holding has a wrong model of the world, and the error is
    /// where they find that out.
    ///
    /// Deliberately narrower than "your evidence was refused". A settlement can
    /// be refused because it names a generation that is no longer there, or
    /// because it contradicts one that is; those are
    /// [`EvidenceSuperseded`](Self::EvidenceSuperseded) and
    /// [`EvidenceContradicted`](Self::EvidenceContradicted). Keeping this
    /// variant meaning only "there is no held effect at this address" is what
    /// lets a caller tell a wrong address from stale or contradictory evidence,
    /// which are three different repairs.
    NoSuchWork {
        /// The address that matched no held entry.
        work: WorkId,
    },
    /// `resolve_effect` named a generation that is not the one in the slot.
    ///
    /// The chain has been re-opened since the caller read
    /// [`PendingWork`], so the attempt the evidence is about is gone and a
    /// different one stands in its place. Accepting the report would apply it
    /// to work the caller never observed: `NotApplied` in particular would make
    /// an attempt nobody can account for eligible to run again. Nothing was
    /// changed, and re-reading the pending work gives the generation to report
    /// against.
    ///
    /// The two digests are the two bindings, not two counters: an
    /// [`ActionDigest`] is derived from the generation's revision together with
    /// the entry it belongs to, so comparing them is comparing which observed
    /// input the attempt was made against. That is the fact that decides
    /// whether evidence is about the same work, and it is why settlement does
    /// not have to be told the revision separately.
    EvidenceSuperseded {
        /// The address the evidence was submitted for.
        work: WorkId,
        /// The binding the caller's key names.
        named: ActionDigest,
        /// The binding the chain holds now.
        current: ActionDigest,
    },
    /// `resolve_effect` was given evidence about an attempt that is no longer
    /// the outstanding one on that entry.
    ///
    /// The generation the work was read from is still the live one — otherwise
    /// this would be [`EvidenceSuperseded`](Self::EvidenceSuperseded) — but the
    /// entry has been attempted again since, so the report is about work that
    /// is over. Nothing was changed.
    ///
    /// This is the one settlement refusal that is not a caller error at all. A
    /// caller that repeats a delivery it could not confirm is behaving exactly
    /// as it should, and the repeat is normally answered idempotently; it lands
    /// here only when the first delivery *did* take effect, the entry was
    /// attempted again, and the repeat then arrives after the new attempt
    /// began. Reporting success would be a lie about which attempt the evidence
    /// moved, and applying it would make an attempt whose effect may be live
    /// eligible to run a third time.
    EvidenceStaleAttempt {
        /// The address the evidence was submitted for.
        work: WorkId,
        /// The attempt the evidence was about.
        reported: AttemptId,
        /// The attempt the entry is on now.
        outstanding: AttemptId,
    },
    /// `resolve_effect` was given evidence opposite to what already settled the
    /// entry for this generation.
    ///
    /// Repeating the *same* evidence is not this error: it succeeds
    /// idempotently, so a caller whose first delivery was ambiguous can send it
    /// again without having to know whether it landed. This is the case where
    /// the entry's outcome is recorded and the new report says the other thing.
    EvidenceContradicted {
        /// The identity the evidence was submitted for.
        work: WorkId,
        /// The evidence already recorded for this generation.
        settled: EffectEvidence,
        /// The evidence that was refused.
        submitted: EffectEvidence,
    },
    /// The action ran and produced a known outcome; the journal refused to
    /// record it.
    ///
    /// Distinct from [`KernelError::EffectRefused`], which is a
    /// *pre-dispatch* refusal: nothing left the process. This one is a
    /// *post-effect* recording failure. The occurrence is already established
    /// and travels on the error, so a controller never has to infer it. Only
    /// the journal append is retryable; the action is never re-entered for a
    /// recording failure, because re-entering it is how a known outcome becomes
    /// a duplicate.
    EffectUnrecorded {
        /// The exact attempt identity the append was for.
        ///
        /// Boxed because `EffectKey` is 128 bytes and an error carrying one by
        /// value makes every `Result` in the workspace pay for a case that is
        /// refused before it changes anything — the same reason
        /// `JournalError::OutOfOrder` boxes it.
        key: Box<EffectKey>,
        /// What the action did. `Applied` means the effect is live;
        /// `NotApplied` means it definitely is not.
        evidence: EffectEvidence,
        /// Why the append was refused.
        ///
        /// Boxed with the key so this variant does not make `EcsError` itself
        /// large enough that every `Result<_, EcsError>` in the workspace pays
        /// `result_large_err`.
        cause: Box<DispatchError>,
    },
}

impl From<KernelError> for EcsError {
    /// Lets `?` carry a kernel failure up into the world layer unchanged.
    fn from(source: KernelError) -> Self {
        Self::Kernel(source)
    }
}

mod class;
mod display;

#[cfg(test)]
mod tests;
