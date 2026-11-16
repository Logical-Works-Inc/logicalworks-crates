//! Acceptance for the observation layer's failure and supersession rows.
//!
//! Three rows of `docs/orchestration-acceptance.spec.md`, each driven through
//! the public surface only:
//!
//! | Row | What it asks | What this file observes |
//! |---|---|---|
//! | **T08** (LC-04) | Disconnect, watch overflow, stale remote key and invalidation failure force refresh rather than permanent quiet state | Each of the four reasons, armed on one source and left armed, forces the next tick to re-read it and appear in `TickReport::forced` with that reason. A forced read of an *unchanged* value commits nothing and fires nothing; a forced read of a *changed* one commits the newer value and then returns to quiet, so the repair is not "re-read forever". A refresh that *fails* leaves the mark standing. |
//! | **T09** (DX-07) | Identical payloads with different event ids both execute; a duplicate delivery does not. Latest-state mode reports supersession separately | Two `EventId`s over equal payloads are two admitted inputs and fire twice; the same id delivered again retires, as does a redelivery of an *older* id. An intermediate value overtaken before it was acted on is reported in `TickReport::superseded` with the revision it was committed under — neither a fired effect nor a retire. |
//! | **T06** (LC-03) | A saturated task A cannot starve independent B; one slow source does not block unrelated ready work | Both halves. The saturation half: a chain whose entries are all held open, and a mass of 100 / 1,000 / 10,000 such chains, never starves an independent chain declared beside it. The slow-source half, in three tests: a source whose poll never resolves is cancelled at the bot's declared per-poll deadline, the independent chain beside it still commits and acts **in the same tick**, and the tick itself returns inside the deadline on an independent watchdog thread; the cancelled chain is re-polled on the next tick and commits when it answers; and a deadline that bounds nothing — zero, or past the ceiling — is refused at build. |
//!
//! The rows live beside this file by subject: `forced_refresh` (T08), `events`
//! (T09), `saturation` (T06, the walk) and `slow_source` with `watchdogs` (T06,
//! the observation phase). This file holds the sources, actions and tick
//! helpers they share.
//!
//! # What is proved here, and how the oracles work
//!
//! Every ordering is established by a shape a reader can inspect — a log of what
//! ran, a chain index in a report, a counter of poll bodies entered — and the one
//! place a clock appears is the *measurement* that the tick returned inside its
//! budget. That measurement is taken on an independent OS thread, the shape
//! `resume_liveness.rs` uses, because an oracle that shares the tick's own driver
//! cannot observe that tick failing to make progress. T06's own text forbids a
//! sleep as the oracle for a *completion*, and none is used: the wedged source
//! is a future that parks forever without waking, so "it never resolves" is a
//! property of the test's own fixture rather than a duration nobody waited for.
//!
//! # Why the oracle is a controlled completion
//!
//! Every *ordering* here is established by the shape of a run the test can read —
//! a held entry, a value that moved, a log of what ran — and never by a clock. The
//! one exception is stated above rather than smuggled in: T06's slow-source test
//! measures how long the tick took, which is a measurement *of the substrate*,
//! and T06's own text forbids a sleep as the oracle for a completion. It is not
//! used as one.
//!
//! # What is real
//!
//! Real `Bot`, real `bevy_ecs` schedule, real `MemoryJournal` behind a real
//! `EffectScope` with a real `Broker`. The sources and actions are the smallest
//! ones that can express the row, and every one of them reports through the
//! public `Observe` / `Execute` traits — nothing reaches past them into the
//! crate's internals.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::KernelError;
use lgwks_bot::spec::Bot;
use lgwks_bot::verb::{EffectLifetime, Execute, Observe, RefreshReason};

mod events;
mod forced_refresh;
mod saturation;
mod slow_source;
mod watchdogs;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

// ── Actions ────────────────────────────────────────────────────────────────

/// An action that lands, and records the value it ran with.
///
/// The log is the oracle for T06 and T09: it is the only place a run's order is
/// visible from outside, so every ordering assertion in this file reads it
/// rather than a counter.
struct Lands {
    /// Every value this action ran with, in the order it ran.
    seen: Rc<RefCell<Vec<u32>>>,
}

impl Execute for Lands {
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
        self.seen.borrow_mut().push(*call.1);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::lands"
    }
}

/// An action that never settles: its attempt may still be live.
///
/// This is how a generation stays *open* long enough for a newer observation to
/// overtake it, which is the situation latest-state mode has to report. It
/// returns the action's own indeterminate error rather than a generic refusal,
/// because an indeterminate effect is the only one of the four certainties that
/// leaves the entry held instead of decided.
struct NeverSettles {
    /// Every value this action ran with, in the order it ran.
    seen: Rc<RefCell<Vec<u32>>>,
}

impl Execute for NeverSettles {
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
        self.seen.borrow_mut().push(*call.1);
        Err(EcsError::Kernel(KernelError::EffectIndeterminate {
            domain: "test::never_settles".to_owned(),
            cause: "the acknowledgement never arrived".to_owned(),
        }))
    }

    fn domain_id(&self) -> &str {
        "test::never_settles"
    }
}

// ── Sources ────────────────────────────────────────────────────────────────

/// A source that can be made to never resolve, and reports how often it was
/// entered.
///
/// The wedging is *controlled* rather than a sleep: `wedged` is a cell the test
/// sets, and while it is set the poll returns a future that parks forever without
/// ever completing. Nothing here waits on real elapsed time to *decide*
/// anything — the deadline is what the substrate applies, and the only clock in
/// this test is the one the watchdog thread uses to report how long the tick took,
/// which is a measurement of the substrate rather than an oracle about the
/// domain.
///
/// How many times the body was entered matters as much as what it returned: it is
/// how the test proves the next tick *re-polls* a stalled chain rather than
/// remembering the cancellation and skipping the source.
struct Wedged {
    /// Whether the poll parks forever instead of reading.
    wedged: Rc<Cell<bool>>,
    /// The value this poll reports once it is not wedged.
    value: Rc<Cell<u32>>,
    /// How many times the poll body was entered.
    entered: Rc<Cell<u32>>,
    /// What `cache_state` answers, so a chain can be declared unsound and its
    /// forced-refresh mark watched across a stall.
    reason: Rc<Cell<Option<RefreshReason>>>,
    /// The domain identity a report names this source by.
    domain: &'static str,
}

impl Observe for Wedged {
    type Output = u32;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u32, EcsError> {
        call.0.check(Observe::required_caps(self))?;
        self.entered.set(self.entered.get().saturating_add(1));
        if self.wedged.get() {
            // Parks without ever resolving and without waking anyone, which is
            // the one shape a deadline has to survive: it is not a slow source,
            // it is a source that has stopped answering. The `resolve` keeps the
            // guard type-checked without a `loop {}`, so a reader can see this
            // is a deliberate park and not an oversight.
            let resolve = std::future::pending::<()>();
            resolve.await;
        }
        Ok(self.value.get())
    }

    fn cache_state(&self) -> Option<RefreshReason> {
        self.reason.get()
    }

    fn domain_id(&self) -> &str {
        self.domain
    }
}

// ── Shared assertions ──────────────────────────────────────────────────────

/// Every value an action ran with, in order.
fn ran_with(seen: &Rc<RefCell<Vec<u32>>>) -> Vec<u32> {
    seen.borrow().clone()
}

/// The tick that fired, or a named failure.
///
/// Ticking returns `Err` for a held transition as well as for a fault, and this
/// file needs to say which it saw — "the tick failed" is not an observation, and
/// several rows are about a run that fires nothing precisely because something
/// was held.
fn tick(bot: &mut Bot) -> TestResult {
    match bot.tick() {
        Ok(fired) => {
            assert_eq!(
                fired,
                bot.tick_report().fired(),
                "the tick's count and its report are one fact read twice: {fired} then {}",
                bot.tick_report().fired()
            );
            Ok(())
        }
        Err(EcsError::PendingTransition { .. }) => Ok(()),
        Err(error) => Err(format!("the tick failed with {error:?}").into()),
    }
}

/// Run one tick whose source is expected to refuse, and assert it said so.
///
/// Separate from [`tick`] because a refusing source and a healthy one want
/// opposite reports, and folding them into one helper would mean every caller
/// re-asserting the distinction it came here to read. The variant is matched
/// exactly rather than accepted as "any error": a refusal that arrived as a
/// `TypeMismatch` or a `PendingTransition` would be a different defect wearing
/// the same silence.
fn refused_tick(bot: &mut Bot) -> TestResult {
    match bot.tick() {
        Ok(fired) => Err(format!(
            "a source that refused read as a {fired}-effect quiet tick; the \
             refusal has to be reported"
        )
        .into()),
        Err(error) => {
            assert!(
                matches!(error, EcsError::Kernel(KernelError::DomainError { .. })),
                "the refusal is the source's own error and keeps its type: {error:?}"
            );
            Ok(())
        }
    }
}

/// Run one tick whose action is expected to leave its entry held, and assert the
/// hold was reported.
///
/// The third distinct tick outcome in this file, beside a clean tick and a
/// refusing source. A hold is reported as an indeterminate effect on the tick
/// whose walk reaches the attempt, and as `PendingTransition` on a later tick
/// that finds the same attempt still held — both are the substrate refusing to
/// call an unknown outcome a decision, which is the property under test. Folding
/// either into [`tick`] would mean every caller re-deciding which of the three
/// it was looking at.
fn held_tick(bot: &mut Bot) -> TestResult {
    match bot.tick() {
        Ok(fired) => Err(format!(
            "an action whose outcome was unknown read as a {fired}-effect clean \
             tick; the hold has to be reported"
        )
        .into()),
        Err(error) => {
            assert!(
                matches!(
                    error,
                    EcsError::Kernel(KernelError::EffectIndeterminate { .. })
                        | EcsError::PendingTransition { .. }
                ),
                "the hold keeps a typed variant — a retry classifier reads it, and \
                 neither arm of this match is a decision: {error:?}"
            );
            Ok(())
        }
    }
}
