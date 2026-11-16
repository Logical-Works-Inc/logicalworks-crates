//! Black-box acceptance for the durable half of a dispatch: what is written
//! down before an effect leaves the process, and what a reconstructed bot does
//! with the record it finds.
//!
//! Three facts are pinned here, and none of them was observable from outside
//! the crate before this file existed.
//!
//! - **Write-ahead.** The journal holds `IntentAdmitted` and `DispatchPrepared`
//!   for an attempt *before* the action's own body runs. That is the whole of
//!   RQ-007, and the only vantage point from which it can be measured is the
//!   effect itself: an assertion made after the tick returns cannot tell
//!   "recorded before the handoff" from "recorded instead of the handoff".
//! - **A recovered unknown is held, not retried.** A bot rebuilt against a
//!   journal that records a dispatch with no outcome refuses to begin another
//!   attempt on that action, reports it through
//!   [`Bot::pending`](lgwks_bot::spec::Bot::pending) with the key to settle it
//!   by, and does not enter the action. Nothing established that the bytes did
//!   not arrive, so sending them again is how a merge is duplicated.
//! - **A recovered attempt is not re-minted.** The journal's ladder allows one
//!   walk of `IntentAdmitted → DispatchPrepared → OutcomeObserved → Verified`
//!   per key, so an attempt the journal already records cannot be minted a
//!   second time under the same identity. A restart therefore continues the
//!   attempt sequence rather than restarting it, and an attempt whose effect is
//!   recorded as landed is retired instead of re-dispatched.
//!
//! # Why these tests cannot pass on the tree before the change
//!
//! They are not weakened versions of something that already ran: on the
//! pre-change tree there is no journal on the dispatch path at all, no
//! `EffectScope` to hand one over with, and no key on a pending report. This
//! file does not compile against it. That is the honest statement of what it
//! would have caught, and it is why the assertions below are on the effect's
//! own observations rather than on what a tick returned.
//!
//! # Layout
//!
//! This file holds the fixtures more than one group uses. Each behaviour has
//! its own module: `recovery` (write-ahead and restart), `recovery_barrier`
//! (a recovered unknown in front of a successor), `ownership` (one controller
//! against another), `input_identity` (what counts as the same input),
//! `handoff` (the durability an external effect needs) and `receipts`
//! (outcome receipts and unreadable journals).

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::broker::{Broker, DispatchError};
use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::effect::FlowRevision;
use lgwks_bot::effect::{EventId, InputIdentity};
use lgwks_bot::error::{DispatchCertainty, KernelError};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::journal::{
    ContinuationPolicy, DurabilityPromise, EffectEvent, EffectJournal, EventKind, FileJournal,
    JournalError, MemoryJournal,
};
use lgwks_bot::spec::{Bot, EffectEvidence, EffectIdentity, EffectScope, TransitionHold};
use lgwks_bot::verb::{EffectLifetime, Evaluate, Execute, Observe};

use crate::sim;

// The scaffolding below lives in `sim::rig`, because the simulation families
// and this acceptance file drive the same bot, the same broker and the same
// shared-store journal adapter. One definition and two callers: a second copy
// would let a family pass while this file tested a bot that no longer exists.
use sim::rig::{
    FixedSource, NeverSettles, NoteWhatIsRecorded, Run, broker, identity, is_value, recorded, scope,
};

mod handoff;
mod input_identity;
mod ownership;
mod receipts;
mod recovery;
mod recovery_barrier;
mod scripted_journal;

/// What a test reports when its precondition did not hold.
///
/// A scope crosses `IdError` and `BrokerError` as well as `EcsError`, and none
/// of the three converts into another, so a test that builds one reports
/// through `Box<dyn Error>` and names the failure rather than flattening it
/// into a variant it is not.
type TestResult = Result<(), Box<dyn Error>>;

/// The name every bot in this file is built under.
///
/// A constant rather than a literal per call site because an action's identity
/// is derived from the bot's name as well as from its position: two bots whose
/// names differ declare different actions, and the restart test's whole subject
/// is that the second bot resolves the first one's keys.
const NAME: &str = "durable-dispatch";

/// One bot under [`NAME`]: `source` observed, `action` armed behind
/// `condition`, and every dispatch recorded through `effects`.
///
/// The one builder in this file. Every test here declares a single chain and
/// differs only in those four parts, so spelling the builder out per test is
/// how one of them would come to build a bot the others do not.
fn armed<S, C, A>(
    source: S,
    condition: C,
    action: A,
    effects: EffectScope,
) -> Result<Bot, Box<dyn Error>>
where
    S: Observe + 'static,
    S::Output: PartialEq + InputIdentity + 'static,
    C: Evaluate<S::Output> + 'static,
    A: Execute<Input = S::Output> + 'static,
    A::Output: 'static,
{
    Ok(Bot::builder(NAME)
        .observe(source)
        .on(condition, action)
        .with_effects(effects)
        .build(&GrantSet::empty())?)
}

/// A bot that acts on every event `source` reports, through `action`, recorded
/// through `effects`.
fn every_request<A>(
    source: Rc<RefCell<EventId<u32>>>,
    action: A,
    effects: EffectScope,
) -> Result<Bot, Box<dyn Error>>
where
    A: Execute<Input = EventId<u32>> + 'static,
    A::Output: 'static,
{
    armed(
        Requests(source),
        |_seen: &EventId<u32>| true,
        action,
        effects,
    )
}

/// Tick `bot` once and require the indeterminate report an effect that never
/// settles gives; anything else fails the test with `why` and what came back.
fn expect_indeterminate(bot: &mut Bot, why: &str) -> TestResult {
    match bot.tick() {
        Err(EcsError::Kernel(KernelError::EffectIndeterminate { .. })) => Ok(()),
        other => Err(format!("{why}; the tick returned {other:?}").into()),
    }
}

/// A source whose value the test sets between bots.
///
/// The #104 scenario is a source that *moves* across the restart: the first
/// bot sees `1`, the reconstructed one sees `2`. A constant source cannot
/// express "the condition is false now", which is the half of the scenario
/// that used to bury the unknown.
struct Shifting(Rc<Cell<u32>>);

impl Observe for Shifting {
    type Output = u32;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u32, EcsError> {
        call.0.check(Observe::required_caps(self))?;
        Ok(self.0.get())
    }

    fn domain_id(&self) -> &str {
        "test::shifting"
    }
}

/// The successor's action: counts every entry, and never fails.
struct Counts(Rc<Cell<u32>>);

impl Execute for Counts {
    type Input = u32;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u32)) -> Result<(), EcsError> {
        call.0.check(Execute::required_caps(self))?;
        self.0.set(self.0.get().saturating_add(1));
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::counts"
    }
}

/// Always true: the successor's condition in the #104 scenario.
fn always(_seen: &u32) -> bool {
    true
}

/// A source the test can retarget to a new `(id, payload)` pair.
///
/// The id is part of `PartialEq` and of [`InputIdentity`], which is the point:
/// two events with equal payloads must still be two events.
struct Requests(Rc<RefCell<EventId<u32>>>);

impl Observe for Requests {
    type Output = EventId<u32>;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<EventId<u32>, EcsError> {
        call.0.check(Observe::required_caps(self))?;
        Ok(*self.0.borrow())
    }

    fn domain_id(&self) -> &str {
        "test::requests"
    }
}

/// An action that records every input it was entered with.
struct Records(Rc<RefCell<Vec<EventId<u32>>>>);

impl Execute for Records {
    type Input = EventId<u32>;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &EventId<u32>)) -> Result<(), EcsError> {
        call.0.check(Execute::required_caps(self))?;
        self.0.borrow_mut().push(*call.1);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::records"
    }
}

/// The handles a state-watch scenario reads: one shared store, the value the
/// source reports, and how often the counting action ran.
struct Watch {
    store: Rc<RefCell<MemoryJournal>>,
    seen: Rc<Cell<u32>>,
    counted: Rc<Cell<u32>>,
    identity: EffectIdentity,
}

impl Watch {
    /// Fresh handles under one identity, the source at `0` and nothing counted.
    fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            store: Rc::new(RefCell::new(MemoryJournal::new())),
            seen: Rc::new(Cell::new(0)),
            counted: Rc::new(Cell::new(0)),
            identity: identity()?,
        })
    }

    /// A bot that counts every value the source reports, recorded in the
    /// shared store.
    fn bot(&self) -> Result<Bot, Box<dyn Error>> {
        self.bot_with(scope(self.identity, Rc::clone(&self.store))?)
    }

    /// The same bot recorded through `effects`.
    fn bot_with(&self, effects: EffectScope) -> Result<Bot, Box<dyn Error>> {
        armed(
            Shifting(Rc::clone(&self.seen)),
            always,
            Counts(Rc::clone(&self.counted)),
            effects,
        )
    }
}
