//! The ECS bot's unit tests: shared fixtures in `fixtures` and `test_support`,
//! cases in `test_part_*`.

#[cfg(test)]
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use super::EcsError;
use super::bot::EcsBot;
use super::effects::Effects;
use super::entry::AttemptRecord;
use super::entry::EntryState;
use super::entry::RetryReading;
use super::entry::Transition;
use super::fold::fire_plan;
use super::fold::observe_fold;
use super::fold::validate;
use super::identity::derive_action_digest;
use super::identity::derive_action_id;
use super::identity::derive_input_stamp;
use super::identity::hash_parts;
use super::ledger::Ledger;
use super::resources::Chains;
use super::resources::Observed;
use super::resources::Policy;
use super::resources::Polled;
use super::resources::TickError;
use super::scope::EffectScope;
use super::watchdog::PollWatchdog;
use super::watchdog::WavePoll;
use super::work::AbandonReason;
use super::work::AdmittedInput;
use super::work::PendingWork;
use super::work::TransitionHold;
use super::work::WorkId;
use super::work::identify_output;
use crate::broker::{Broker, DispatchError};
use crate::cap::{Auth, Cap, Demand};
use crate::clock::Clock;
use crate::effect::EventId;
use crate::effect::InputIdentity;
use crate::effect::{
    ActionDigest, ActionId, AttemptId, EffectIdentity, EffectKey, EnvironmentEpoch, FlowRevision,
};
use crate::effect::{EnvironmentId, RunId};
use crate::error::{DispatchCertainty, KernelError, RetryClass};
use crate::gate::GrantSet;
use crate::journal::EffectEvidence;
use crate::journal::{DurabilityPromise, DurableAck, JournalPosition, MemoryJournal};
use crate::journal::{EffectEvent, EffectJournal, JournalError};
use crate::spec::{Erased, typed_entry};
use crate::verb::{EffectLifetime, Evaluate, Execute, Observe};
use lgwks_deps::bevy_ecs::{
    prelude::World,
    schedule::{IntoScheduleConfigs, LogLevel, Schedule, ScheduleBuildSettings},
};
use lgwks_std::hash::Hasher;
use lgwks_std::retry::RetryPolicy;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

mod fixtures;
use fixtures::{
    AbaSource, Count, CountU64, Counted, Exhausting, Flaky, FlakyKind, Holds, Record, Rev, Script,
    Switched, Yielding, domain_refusal, first_hold, identities, identity,
};

mod test_support;
pub(crate) use test_support::test_effects;
use test_support::{
    AltTick, CountsU32, FaultJournal, NeedsCaps, NoSettlementRoom, PrefixProbe,
    PrefixProbeNextSchema, PrefixProbeTail, Refuses, RefusesToEvaluate, SCRIPT,
    SwappedEntryJournal, TEST_ENV, TEST_FLOW, TEST_RUN, Unpolled, counting_bot, held_key,
    indeterminate_once_bot, net_grants, refused, test_effects_with,
};

mod test_part_00;
mod test_part_01;
mod test_part_02;
mod test_part_03;
mod test_part_04;
mod test_part_05;

// The cases INVARIANTS.md cites by path (INV-BOT-41, -42, -123) live here,
// at `ecs::tests::<name>`, so the `invariants` lane resolves them.

/// A yielding source turned once on a live wave, then again after the wave
/// expired, answers with its value; a source still pending on that turn is
/// the one reported stalled.
///
/// This is the interleaving a loaded host produces and an idle one does
/// not: the reaper fires between a source's `Pending` and the turn it woke
/// itself for. `join` waits on the real reaper, which only returns once the
/// wave has expired because both polls are still alive, so the interleaving
/// is forced rather than raced, and a wave that cut off the yielding source
/// unpolled fails here on every run.
#[test]
fn an_expired_wave_still_reads_a_source_ready_on_its_next_turn() {
    // Long enough that no host schedules the two first turns past it, so the
    // wave is live for both; a 1 ms deadline let a loaded host fire the reaper
    // before the first turn finished and stall it (run 38438866216, CI
    // `Tests (Linux container) (workspace)`). The expiry below is written, not
    // waited for, so no clock decides the interleaving.
    let deadline = Duration::from_secs(60);
    let watchdog = PollWatchdog::new(deadline, 2);
    let clock = Clock::wall();
    let armed = AtomicBool::new(false);
    let mut cx = Context::from_waker(Waker::noop());
    let yields = Rc::new(Cell::new(1_u32));
    let mut yielding = WavePoll {
        chain: 0,
        watchdog: &watchdog,
        slot: 0,
        installed: false,
        poll: Box::pin(std::future::poll_fn({
            let yields = Rc::clone(&yields);
            move |turn: &mut Context<'_>| {
                if yields.get() > 0 {
                    yields.set(yields.get().saturating_sub(1));
                    turn.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(None))
                }
            }
        })),
    };
    let mut wedged = WavePoll {
        chain: 1,
        watchdog: &watchdog,
        slot: 1,
        installed: false,
        poll: Box::pin(std::future::pending()),
    };
    assert!(
        yielding
            .turn(&mut cx, &clock, &armed, deadline)
            .is_pending(),
        "a source that yields is pending on its first turn"
    );
    assert!(
        wedged.turn(&mut cx, &clock, &armed, deadline).is_pending(),
        "a wedged source is pending on its first turn"
    );
    assert!(
        armed.load(Ordering::Relaxed),
        "a wave with a pending poll starts its reaper"
    );
    {
        // What the reaper writes when the deadline passes, then the wake it
        // would send: the live reaper sees `expired` and returns.
        let mut state = crate::journal::owner::lock(&watchdog.state);
        state.expired = true;
        // Also released, so a reaper that has not yet reached its wait returns
        // at the top of its loop rather than sleeping out the 60 s budget.
        state.released = true;
        watchdog.settled.notify_all();
    }
    watchdog.join();
    assert!(
        crate::journal::owner::lock(&watchdog.state).expired,
        "the reaper returned on the wave's expiry"
    );
    assert!(
        matches!(
            yielding.turn(&mut cx, &clock, &armed, deadline),
            Poll::Ready(Ok(None))
        ),
        "a source ready on the turn after expiry answers rather than stalls"
    );
    assert!(
        matches!(
            wedged.turn(&mut cx, &clock, &armed, deadline),
            Poll::Ready(Err(EcsError::PollStalled { chain: 1, .. }))
        ),
        "a source still pending after its last turn is the one cut off"
    );
    assert_eq!(
        yields.get(),
        0,
        "the yielding source was turned exactly twice"
    );
}

/// The applied-key index answers the same questions the vector did, at a
/// history size where scanning before every push was quadratic.
///
/// This drives the shipped path: [`Effects::note_applied`] is what the live
/// settle and the recovered seeding both call, and [`Effects::applied_in`]
/// is what the walk asks before it dispatches. Each key's digest is the one
/// `applied_in` recomputes for its coordinates, which is the discriminating
/// part — a wrong index answers false for a key it holds. The old
/// `Vec<EffectKey>` would need Θ(n²) comparisons to build this set; the
/// indexed form builds and queries it in one pass.
#[test]
fn applied_membership_answers_at_scale_where_a_scan_would_be_quadratic() -> TestResult {
    /// Distinct actions, one per obligation. Large enough that the removed
    /// scan-before-push is visibly quadratic, small enough for a suite.
    const COUNT: u64 = 50_000;

    /// A distinct 16-byte action id for `index`.
    fn action_for(index: u64) -> Result<ActionId, Box<dyn std::error::Error>> {
        Ok(ActionId::from_hex(&format!(
            "{:032x}",
            index.wrapping_add(1)
        ))?)
    }
    /// The coordinates `applied_in` would ask about for `index`.
    fn coordinates(index: u64) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let chain = index.checked_rem(16).ok_or("sixteen chains is not zero")?;
        let entry = index.checked_div(16).ok_or("sixteen chains is not zero")?;
        Ok((usize::try_from(chain)?, usize::try_from(entry)?))
    }

    let scope = test_effects()?;
    let mut effects = Effects::new(scope, JournalPosition::genesis());
    let identity = effects.identity();
    let flow = identity.flow();
    let run = identity.run();
    let environment = identity.environment();
    let epoch = EnvironmentEpoch::from_decimal("1")?;
    let attempt = AttemptId::from_decimal("1")?;
    let input = [0u8; 16];

    for index in 0..COUNT {
        let (chain, entry) = coordinates(index)?;
        let key = crate::effect::EffectIdentity::new(run, environment, flow).key(
            action_for(index)?,
            attempt,
            derive_action_digest(flow, chain, entry, &input),
            epoch,
        );
        effects.note_applied(key);
    }

    // A duplicate settlement must not grow either index.
    let (chain, entry) = coordinates(0)?;
    effects.note_applied(
        crate::effect::EffectIdentity::new(run, environment, flow).key(
            action_for(0)?,
            attempt,
            derive_action_digest(flow, chain, entry, &input),
            epoch,
        ),
    );
    assert_eq!(
        effects.applied_events.len(),
        usize::try_from(COUNT)?,
        "a duplicate settlement changed the event-identity index"
    );
    assert_eq!(
        effects.applied_latest.len(),
        usize::try_from(COUNT)?,
        "a duplicate settlement changed the latest-key index"
    );

    for index in 0..COUNT {
        let (chain, entry) = coordinates(index)?;
        let action = action_for(index)?;
        assert!(
            effects.applied_in(action, chain, entry, input, true),
            "the event identity for obligation {index} is absent from the index"
        );
        assert!(
            effects.applied_in(action, chain, entry, input, false),
            "the contract identity for obligation {index} is absent from the index"
        );
    }

    // An action the run never applied is not held, in either identity.
    let absent = action_for(COUNT)?;
    assert!(
        !effects.applied_in(absent, 0, 0, input, true)
            && !effects.applied_in(absent, 0, 0, input, false),
        "an unapplied obligation must not resolve as done"
    );
    Ok(())
}

/// An external handoff is refused before any rung when the settlement
/// record does not fit, so no attempt is ever left admitted and unable to
/// settle.
///
/// Acceptance for #122 item 2 / #156: the reservation happens before the
/// first write, so a journal that cannot hold the settlement refuses the
/// handoff and the store stays untouched — rather than admitting the intent
/// and the preparation and then being unable to persist the outcome.
#[test]
fn an_external_handoff_reserves_settlement_capacity_before_any_rung() -> TestResult {
    let appends = Rc::new(Cell::new(0));
    let journal = NoSettlementRoom {
        inner: MemoryJournal::new(),
        appends: Rc::clone(&appends),
    };
    let scope = test_effects_with(Box::new(journal))?;
    let mut effects = Effects::new(scope, JournalPosition::genesis());
    let key = crate::effect::EffectIdentity::new(
        RunId::from_hex(TEST_RUN)?,
        EnvironmentId::from_hex(TEST_ENV)?,
        FlowRevision::from_tagged("blake3_256", TEST_FLOW)?,
    )
    .key(
        ActionId::from_hex("1112131415161718191a1b1c1d1e1f20")?,
        AttemptId::from_decimal("1")?,
        ActionDigest::from_tagged(
            "blake3_256",
            "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeffe0e1e2e3e4e5e6e7e8e9eaebecedeeef",
        )?,
        EnvironmentEpoch::from_decimal("1")?,
    );

    match lgwks_std::task::block_on(effects.prepare(key, EffectLifetime::External)) {
        Err(DispatchError::Journal(JournalError::CapacityExceeded { .. })) => {}
        Err(other) => {
            return Err(format!("expected a capacity refusal, got {other:?}").into());
        }
        Ok(_) => {
            return Err("a handoff with no settlement room must be refused".into());
        }
    }
    assert_eq!(
        appends.get(),
        0,
        "the handoff must be refused before the first rung is written"
    );
    Ok(())
}
