//! Row-addressed seeded sweeps: one band per T-row, named for its row.
//!
//! The companion to [`t_rows`](super::t_rows), which holds the rows whose
//! falsifier is one real journey. This file holds the rows where the honest
//! oracle is a *sweep*: the seed chooses the shape of the scenario -- how many
//! tenants, which chain fails, how wide a held mass, how many restarts -- and
//! the property must hold for every seed in the band.
//!
//! The rows live beside this file by subject: `refresh` (T06, T07, T08: what a
//! read does and what a held mass cannot starve) and `dependents` (T10, T11,
//! T12: what a predecessor's state decides, and whose store is whose).
//!
//! # Why a sweep is the stronger claim
//!
//! A single run of "tenants never cross" is a sample. A sweep is a statement
//! about every seed in a declared band, and every family here runs its band
//! twice and requires the two trace hashes to match, so a nondeterministic run
//! fails even when every assertion happens to pass. The hash is built from
//! dispositions, counts and committed values, never from elapsed time, which is
//! what makes it a receipt a reader can compare rather than a log they must
//! diff.
//!
//! # What is real and what is seeded
//!
//! The bot, the broker, the real `MemoryJournal` and the real artifact store are
//! all the shipped types. What the seed decides is the scenario, and the clock
//! is never read at all. Nothing here reimplements the contract under test: a
//! simulation with its own copy of the ladder would pass while the shipped
//! ladder rotted.
//!
//! # Naming
//!
//! Every test here ends in its row id, so
//! `cargo nextest list --workspace -E 'test(/_t08$/)'` answers "which tests
//! address T08" without reading the row map.

use crate::sim;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::{DispatchCertainty, KernelError};
use lgwks_bot::journal::{EffectEvent, EffectJournal, MemoryJournal};
use lgwks_bot::spec::{Bot, EffectScope};
use lgwks_bot::verb::{EffectLifetime, Execute, RefreshReason};

use sim::rig::{identity, scope};

#[path = "sim_t_rows/sim_dependents.rs"]
mod dependents;
#[path = "sim_t_rows/sim_refresh.rs"]
mod refresh;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// What a tick is expected to do, so the reader of one scenario sees which case
/// it is exercising rather than inferring it from the assertions after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// The tick fired, or held an already-held entry. Both are facts here.
    Settled,
    /// The tick must report the source's own refusal.
    Refusal,
    /// The tick must report an action whose outcome never arrived, as either of
    /// the two variants a hold takes.
    Hold,
}

/// What an [`Acting`] body reports when it runs.
///
/// One enum rather than one struct per behaviour, because the two behaviours
/// differ only in the outcome they report, and two `Execute` impls would be two
/// copies of the same admission prologue for the sake of a one-line difference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// The effect landed, and the value it ran with is recorded.
    Lands,
    /// The acknowledgement never arrived, so the entry is held rather than done.
    Held,
    /// The effect provably did not land, so the entry may be retried while its
    /// attempt budget lasts and is abandoned when it runs out.
    Refuses,
}

/// An action that records its inputs and then lands or holds, per [`Outcome`].
///
/// The held shape is what a starved-or-duplicated walk has to survive: the body
/// genuinely ran, and nothing proved the bytes did not arrive, so the walk must
/// report the entry as outstanding rather than treat its silence as completion.
#[derive(Clone)]
struct Acting {
    /// What this body reports when it runs.
    outcome: Outcome,
    /// Every value it ran with, in order.
    ran: Rc<RefCell<Vec<u32>>>,
    /// How many times the body was entered.
    entered: Rc<Cell<u32>>,
}

impl Acting {
    /// An action that lands, recording each value it ran with.
    fn lands() -> Self {
        Self::with(Outcome::Lands)
    }

    /// An action that holds, counting its entries.
    fn holds() -> Self {
        Self::with(Outcome::Held)
    }

    /// An action that definitely fails, counting its entries.
    fn refuses() -> Self {
        Self::with(Outcome::Refuses)
    }

    /// An action with a fresh record and a zeroed counter.
    fn with(outcome: Outcome) -> Self {
        Self {
            outcome,
            ran: Rc::new(RefCell::new(Vec::new())),
            entered: Rc::new(Cell::new(0)),
        }
    }

    /// A handle on this action's own record, cloned so the caller and the action
    /// both read the same vector.
    fn record(&self) -> Rc<RefCell<Vec<u32>>> {
        Rc::clone(&self.ran)
    }

    /// How many times the body ran.
    fn entries(&self) -> u32 {
        self.entered.get()
    }
}

impl Execute for Acting {
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
        self.entered.set(self.entries().saturating_add(1));
        self.ran.borrow_mut().push(*call.1);
        match self.outcome {
            Outcome::Lands => Ok(()),
            Outcome::Held => Err(EcsError::Kernel(KernelError::EffectIndeterminate {
                domain: "test::acting".to_owned(),
                cause: "the acknowledgement never arrived".to_owned(),
            })),
            // `NotDelivered`, not `Refused`: a transient refusal the chain
            // retries while it has budget, so the Failed and Abandoned arms of
            // T10 are two budgets over one action rather than two actions.
            Outcome::Refuses => Err(EcsError::Kernel(KernelError::DomainError {
                domain: "test::acting".to_owned(),
                certainty: DispatchCertainty::NotDelivered,
                cause: "the upstream refused before anything was delivered".to_owned(),
            })),
        }
    }

    fn domain_id(&self) -> &str {
        "test::acting"
    }
}

/// The effect scope a bot in this file dispatches through.
fn scope_of(store: &Rc<RefCell<MemoryJournal>>) -> Result<EffectScope, Box<dyn Error>> {
    scope(identity()?, Rc::clone(store))
}

/// Every committed event in `store`, through the trait rather than the concrete
/// adapter: what these families are about is what the journal promises, and a
/// test reading `MemoryJournal` directly would keep passing if the promise
/// changed.
fn committed(store: &Rc<RefCell<MemoryJournal>>) -> Result<Vec<EffectEvent>, Box<dyn Error>> {
    Ok(EffectJournal::committed(&*store.borrow())?)
}

/// Tick `bot` and check the outcome against `expect`.
///
/// One helper rather than a `tick` and a `refused_tick`, because they differ only
/// in which arm of the same `match` they treat as the failure: spelled twice, the
/// two would drift and a scenario would quietly stop checking the refusal it
/// exists to provoke. Ticking returns `Err` for a hold as well as for a fault,
/// and several rows are about a run that fires nothing precisely because
/// something was held, so `Anything` accepts both.
fn tick_as(bot: &mut Bot, expect: Expect) -> TestResult {
    let held = |error: &EcsError| {
        matches!(
            error,
            EcsError::Kernel(KernelError::EffectIndeterminate { .. })
                | EcsError::PendingTransition { .. }
        )
    };
    match (bot.tick(), expect) {
        (Ok(fired), Expect::Settled) => {
            assert_eq!(
                fired,
                bot.tick_report().fired(),
                "the tick's count and its report are one fact read twice"
            );
            Ok(())
        }
        (Err(error), Expect::Settled) if held(&error) => Ok(()),
        (Ok(fired), Expect::Refusal) => Err(format!(
            "a source that refused read as a {fired}-effect quiet tick; the refusal has to be \
             reported"
        )
        .into()),
        (Err(error), Expect::Refusal) => {
            assert!(
                matches!(error, EcsError::Kernel(KernelError::DomainError { .. })),
                "the refusal keeps the source's own type: {error:?}"
            );
            Ok(())
        }
        (Ok(fired), Expect::Hold) => Err(format!(
            "an action whose outcome was unknown read as a {fired}-effect clean tick; the hold \
             has to be reported"
        )
        .into()),
        (Err(error), Expect::Hold) => {
            assert!(
                held(&error),
                "the hold keeps a typed variant, because a retry classifier reads it and \
                 neither arm of this match is a decision: {error:?}"
            );
            Ok(())
        }
        (Err(error), _) => Err(format!("the tick failed with {error:?}").into()),
    }
}

/// One chain's committed values, recorded into `sim`'s trace.
///
/// A helper rather than a `record_number` per call site, because a trace that
/// recorded a labelled value at one site and a bare one at another would make two
/// families' hashes incomparable, which is the one thing the receipt exists to
/// prevent.
fn record_values(sim: &mut sim::Sim, label: &str, values: impl Iterator<Item = u32>) {
    for value in values {
        sim.trace.record_number(label, u64::from(value));
    }
}

/// The four reasons a source may declare its own baseline unsound, with the
/// spelling a failure message uses.
///
/// One table rather than four literals per call site: T08 asks that *every* reason
/// reach the same repair, and a family that named three of the four would pass
/// while the fourth went unexercised.
const CAUSES: [(RefreshReason, &str); 4] = [
    (RefreshReason::Disconnected, "disconnect"),
    (RefreshReason::WatchOverflow, "watch overflow"),
    (RefreshReason::StaleRemoteKey, "stale remote key"),
    (RefreshReason::InvalidationFailed, "invalidation failure"),
];
