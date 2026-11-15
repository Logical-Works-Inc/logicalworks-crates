//! T10, T11, T12: what a predecessor's state decides, what a restart may
//! resend, and whose artifact store is whose.

use crate::band_family::band_family;
use crate::sim::rig::{FixedSource, NeverSettles, is_value};
use crate::sim::{self, Band};

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::time::Duration;

use lgwks_bot::error::KernelError;
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::spec::{Bot, RetryPolicy};
use lgwks_bot::{EcsError, GrantSet};

use super::{Acting, Expect, TestResult, committed, scope_of, tick_as};

// ── T10: a required predecessor's state gates its dependent ────────────────

/// T10: over a drawn number of recovery cycles, a predecessor whose effect never
/// settled keeps its dependent from running -- the dependent does not proceed
/// merely because the predecessor is no longer active.
///
/// The seed draws the number of cycles rather than the states, because the
/// property under test is that *no* number of restarts resolves the dependency by
/// omission.
fn a_required_predecessor_holds_its_dependent(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let cycles = sim.rng().between(1, 8);
        let store = Rc::new(RefCell::new(MemoryJournal::new()));
        let entered = Rc::new(Cell::new(0));
        let dependent = Acting::lands();
        let dependent_ran = dependent.record();

        for cycle in 0..cycles {
            // Every cycle rebuilds the bot over the same journal, which is what a
            // restart does: the recovered attempt is in the store and nothing has
            // told the store it landed.
            let mut bot = Bot::builder("sim-t10")
                .observe(FixedSource)
                .on(
                    is_value,
                    NeverSettles {
                        entered: Rc::clone(&entered),
                    },
                )
                .on(|_: &u32| true, dependent.clone())
                .with_effects(scope_of(&store)?)
                .build(&GrantSet::empty())?;
            tick_as(&mut bot, Expect::Hold)?;
            assert_eq!(
                dependent_ran.borrow().len(),
                0,
                "cycle {cycle}: the dependent ran behind a predecessor the journal never settled"
            );
        }
        assert_eq!(
            entered.get(),
            1,
            "however many cycles ran, the predecessor's body was entered exactly once: a \
             recovered attempt that is held is not re-entered, and the dependent never ran"
        );
        assert!(
            !committed(&store)?.is_empty(),
            "the attempt is on the record, so nothing resolves by omission"
        );
        sim.trace.record_number("cycles", u64::from(cycles));
        Ok(())
    })
}

/// The states T10 names for a required predecessor, beside the one decided
/// state that does not hold its dependent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Predecessor {
    /// It definitely failed and still has attempts left.
    Failed,
    /// It definitely failed with no attempt left, so it was given up on.
    Abandoned,
    /// Its outcome never arrived.
    Unknown,
    /// Its condition did not hold, so it ran nothing and left nothing in doubt.
    Skipped,
}

impl Predecessor {
    /// Every state, in a fixed order, so the seed's draw names the same state on
    /// every run.
    const ALL: [Self; 4] = [Self::Failed, Self::Abandoned, Self::Unknown, Self::Skipped];

    /// The spelling the trace records.
    const fn tag(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
            Self::Unknown => "unknown",
            Self::Skipped => "skipped",
        }
    }
}

/// T10, swept over every predecessor state: a dependent behind a predecessor
/// that failed, was abandoned or became Unknown never runs, over a drawn number
/// of ticks; a dependent behind a skipped predecessor runs once.
///
/// One dependent and one predecessor per scenario, so the only thing the seed
/// varies is the state the predecessor is left in and how long the bot keeps
/// ticking afterwards. The row's subject is "merely because it is no longer
/// active": a failed entry stops being attempted once its budget is spent, an
/// unknown one is held rather than re-entered, and neither is a reason for the
/// dependent to proceed. The skipped arm is the control that keeps the sweep
/// honest: a predecessor whose guard did not hold decided that nothing should
/// happen, so the entry behind it is not held hostage by an effect that never
/// started, and a sweep that blocked everything would fail there.
fn every_predecessor_state_decides_its_dependent(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let drawn = usize::try_from(sim.rng().below(4))?;
        let state = *Predecessor::ALL
            .get(drawn)
            .ok_or("the draw names a predecessor state")?;
        let ticks = sim.rng().between(1, 8);
        let store = Rc::new(RefCell::new(MemoryJournal::new()));
        let predecessor = match state {
            Predecessor::Failed | Predecessor::Abandoned => Acting::refuses(),
            Predecessor::Unknown => Acting::holds(),
            Predecessor::Skipped => Acting::lands(),
        };
        let dependent = Acting::lands();
        let dependent_ran = dependent.record();
        let guard = match state {
            Predecessor::Skipped => |_: &u32| false,
            Predecessor::Failed | Predecessor::Abandoned | Predecessor::Unknown => is_value,
        };
        let policy = match state {
            Predecessor::Abandoned => RetryPolicy::new(1, Duration::ZERO, Duration::MAX),
            Predecessor::Failed | Predecessor::Unknown | Predecessor::Skipped => {
                RetryPolicy::new(3, Duration::ZERO, Duration::MAX)
            }
        };
        let mut bot = Bot::builder("sim-t10-states")
            .with_retry_policy(policy)
            .observe(FixedSource)
            .on(guard, predecessor.clone())
            .on(|_: &u32| true, dependent)
            .with_effects(scope_of(&store)?)
            .build(&GrantSet::empty())?;

        for tick in 0..ticks {
            let outcome = bot.tick();
            match (state, outcome) {
                (Predecessor::Skipped, Ok(_)) => {}
                (Predecessor::Skipped, Err(error)) => {
                    let refusal = Err(format!(
                        "tick {tick}: a skipped predecessor holds nothing, got {error:?}"
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "every_predecessor_state_decides_its_dependent: returning an error to the caller");
                    return refusal;
                }
                (_, Ok(fired)) => {
                    let refusal = Err(format!(
                        "tick {tick}: a {} predecessor read as a clean {fired}-effect tick",
                        state.tag()
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "every_predecessor_state_decides_its_dependent: returning an error to the caller");
                    return refusal;
                }
                (_, Err(error)) => assert!(
                    matches!(
                        error,
                        EcsError::Kernel(
                            KernelError::DomainError { .. }
                                | KernelError::EffectIndeterminate { .. }
                        ) | EcsError::PendingTransition { .. }
                    ),
                    "tick {tick}: a {} predecessor is reported by a typed variant: {error:?}",
                    state.tag()
                ),
            }
        }

        let attempts_left = policy.max_attempts;
        match state {
            Predecessor::Skipped => {
                assert_eq!(
                    predecessor.entries(),
                    0,
                    "a skipped predecessor ran nothing"
                );
                assert_eq!(
                    dependent_ran.borrow().as_slice(),
                    [sim::rig::VALUE],
                    "the dependent of a skipped predecessor ran once, with the observed value"
                );
            }
            Predecessor::Failed | Predecessor::Abandoned | Predecessor::Unknown => {
                assert!(
                    dependent_ran.borrow().is_empty(),
                    "after {ticks} ticks the dependent of a {} predecessor ran: {:?}",
                    state.tag(),
                    dependent_ran.borrow()
                );
                assert!(
                    !bot.pending().is_empty(),
                    "the {} predecessor stays reported rather than dropped",
                    state.tag()
                );
                let entered = predecessor.entries();
                assert!(
                    (1..=attempts_left).contains(&entered),
                    "the {} predecessor ran at least once and never past its budget of \
                     {attempts_left}: {entered}",
                    state.tag()
                );
                if state == Predecessor::Unknown {
                    assert_eq!(entered, 1, "a held predecessor is never re-entered");
                }
            }
        }
        sim.trace.record(state.tag());
        sim.trace.record_number("ticks", u64::from(ticks));
        sim.trace
            .record_number("predecessor_entries", u64::from(predecessor.entries()));
        Ok(())
    })
}

// ── T11: a late settlement stays attributable to its own attempt ────────────

/// T11: a predecessor that reported itself indeterminate is never re-entered
/// across a drawn number of restarts, and every restart holds it under the same
/// key rather than resolving it by omission.
///
/// The row's subject is attribution: an effect whose outcome never arrived cannot
/// be re-sent, because the bytes may well have landed. The seed draws how many
/// restarts happen, so "one restart held it" is not the whole claim.
fn an_indeterminate_predecessor_is_never_resent(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let restarts = sim.rng().between(1, 8);
        let store = Rc::new(RefCell::new(MemoryJournal::new()));
        let entered = Rc::new(Cell::new(0));
        let rebuild = || -> Result<Bot, Box<dyn Error>> {
            Ok(Bot::builder("sim-t11")
                .observe(FixedSource)
                .on(
                    is_value,
                    NeverSettles {
                        entered: Rc::clone(&entered),
                    },
                )
                .with_effects(scope_of(&store)?)
                .build(&GrantSet::empty())?)
        };

        let mut first = rebuild()?;
        tick_as(&mut first, Expect::Hold)?;
        let after_first = entered.get();
        let key = committed(&store)?
            .first()
            .map(|event| event.key())
            .ok_or("the attempt was not on the record")?;

        for restart in 0..restarts {
            let mut rebuilt = rebuild()?;
            let held = rebuilt.pending();
            assert_eq!(
                held.len(),
                1,
                "restart {restart}: the recovered unknown is reported, not forgotten"
            );
            assert_eq!(
                held[0].key(),
                Some(key),
                "restart {restart}: and it names the same key, so the hold is attributable"
            );
            tick_as(&mut rebuilt, Expect::Hold)?;
            assert_eq!(
                entered.get(),
                after_first,
                "restart {restart}: the body was not re-entered, so nothing was sent twice"
            );
        }
        sim.trace.record_number("restarts", u64::from(restarts));
        Ok(())
    })
}

// ── T12: one tenant's store is not another's ───────────────────────────────

/// T12: a drawn number of tenants, each holding the *same bytes*, commit one
/// artifact each and read back only their own -- identical digests across tenants
/// must not become a shared shelf, a shared count, or a shared read.
#[cfg(feature = "script")]
fn identical_digests_never_cross_tenants(band: Band) -> TestResult {
    use lgwks_bot::proposal::ArtifactStore;

    sim::assert_replays(band, |sim| {
        let tenants = sim.rng().between(2, 16);
        let store = ArtifactStore::new();
        let bytes = b"the same bytes from many tenants".to_vec();
        let digest = ArtifactStore::digest_of(&bytes)?;
        let names: Vec<String> = (0..tenants)
            .map(|index| format!("tenant-{index}"))
            .collect();

        for tenant in &names {
            store.write(tenant, &bytes)?;
            assert!(
                store.holds(tenant, &digest),
                "{tenant}: holds its own artifact under a digest every tenant shares"
            );
            assert!(
                store.read(tenant, &digest).is_some(),
                "{tenant}: reads its own bytes"
            );
            assert_eq!(
                store.artifacts(tenant),
                1,
                "{tenant}: exactly one artifact, however many writers share its digest"
            );
            assert_eq!(
                store.writers(tenant, &digest),
                1,
                "{tenant}: the writer count under a shared digest is this tenant's own, never a \
                 neighbour's"
            );
        }
        assert!(
            store.read("a-tenant-that-never-wrote", &digest).is_none(),
            "a tenant that never wrote reads nothing under a digest it does not own"
        );
        sim.trace.record_number("tenants", u64::from(tenants));
        Ok(())
    })
}

// ── The families, one declared test per band ───────────────────────────────

band_family! {
    a_required_predecessor_holds_its_dependent_t10 => a_required_predecessor_holds_its_dependent, 39;
    an_indeterminate_predecessor_is_never_resent_t11 => an_indeterminate_predecessor_is_never_resent, 40;
    every_predecessor_state_decides_its_dependent_t10 => every_predecessor_state_decides_its_dependent, 42;
}

// `ArtifactStore` lives in `proposal`, which is gated on `script`.
#[cfg(feature = "script")]
band_family! {
    identical_digests_never_cross_tenants_t12 => identical_digests_never_cross_tenants, 41;
}
