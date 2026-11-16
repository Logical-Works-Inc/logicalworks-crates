//! T06, the slow-source half: a poll that never resolves.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lgwks_bot::ecs::{DEFAULT_POLL_DEADLINE, EcsError, MAX_POLL_DEADLINE};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::spec::Bot;
use lgwks_bot::verb::RefreshReason;

use crate::effects::memory_scope;

use super::{Lands, TestResult, Wedged, ran_with, tick};

/// The handles a test needs to drive one wedged chain beside an independent one.
struct WedgedRig {
    /// The bot under test.
    bot: Bot,
    /// Whether the wedged chain's source parks.
    wedged: Rc<Cell<bool>>,
    /// The value the wedged chain reports once it answers.
    wedged_value: Rc<Cell<u32>>,
    /// How many times the wedged chain's poll body was entered.
    wedged_entered: Rc<Cell<u32>>,
    /// What the wedged chain declares about its own cache.
    wedged_reason: Rc<Cell<Option<RefreshReason>>>,
    /// How many times the independent chain's source was polled.
    ready_entered: Rc<Cell<u32>>,
    /// The value the independent chain's source reports, so a test can make that
    /// chain have work ready on the tick it cares about.
    ready_value: Rc<Cell<u32>>,
    /// Every value the independent chain's action ran with, in order.
    ready_seen: Rc<RefCell<Vec<u32>>>,
    /// Every value the wedged chain's action ran with, in order.
    wedged_seen: Rc<RefCell<Vec<u32>>>,
}

/// Build a two-chain bot under `deadline`: chain 0's source can be wedged, chain
/// 1's always answers.
///
/// One bot, one world, one schedule step — the same construction
/// `saturated_pair` uses, and for the same reason: T06 is a claim about two
/// chains on *one* substrate, not about two processes that happen to interleave.
/// The difference is which chain is saturated. There the walk is held open by a
/// transition; here the *observation phase* is held open by a source, which is
/// the half that a fan-out cap does not bound.
///
/// The independent chain is declared second so it shares the observation wave with
/// the wedged one: two chains that landed in different waves would not prove
/// that a cancellation in one wave leaves the other free, which is the property.
fn wedged_pair(deadline: Duration) -> Result<WedgedRig, Box<dyn Error>> {
    let wedged = Rc::new(Cell::new(false));
    let wedged_value = Rc::new(Cell::new(1));
    let wedged_entered = Rc::new(Cell::new(0));
    let wedged_reason = Rc::new(Cell::new(None));
    let wedged_seen = Rc::new(RefCell::new(Vec::new()));
    let ready_entered = Rc::new(Cell::new(0));
    let ready_value = Rc::new(Cell::new(2));
    let ready_seen = Rc::new(RefCell::new(Vec::new()));

    let bot = Bot::builder("t06-slow-source")
        .with_poll_deadline(deadline)
        .observe(Wedged {
            wedged: Rc::clone(&wedged),
            value: Rc::clone(&wedged_value),
            entered: Rc::clone(&wedged_entered),
            reason: Rc::clone(&wedged_reason),
            domain: "test::wedged",
        })
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&wedged_seen),
            },
        )
        .observe(Wedged {
            wedged: Rc::new(Cell::new(false)),
            value: Rc::clone(&ready_value),
            entered: Rc::clone(&ready_entered),
            reason: Rc::new(Cell::new(None)),
            domain: "test::independent",
        })
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&ready_seen),
            },
        )
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?;

    Ok(WedgedRig {
        bot,
        wedged,
        wedged_value,
        wedged_entered,
        wedged_reason,
        ready_entered,
        ready_value,
        ready_seen,
        wedged_seen,
    })
}

/// A source that never resolves does not stop an independent chain from running
/// in the same tick.
///
/// The row's second clause, and the half that needed a per-poll deadline to
/// exist. Chain 0's source is wedged: its poll is entered, parks forever and
/// never wakes. Chain 1's source answers immediately. Both are in one observation
/// wave, so if the deadline were per *wave* rather than per *poll* — or if the
/// cancellation stopped the wave — chain 1 would never commit and the tick would
/// never reach the walk at all.
///
/// Three observations, and each is a different failure to catch:
///
/// 1. the tick **returns**, measured on an independent watchdog thread against
///    `deadline + slack`. A source that held the tick would make this thread the
///    only thing that ever finished.
/// 2. chain 1's **action ran** in that same tick, from its log. A tick that
///    returned early without running anything would satisfy (1).
/// 3. chain 0 is **reported stalled**, naming its own `domain_id`. A tick that
///    returned silently would satisfy (1) and (2) and hide the whole defect.
///
/// The watchdog thread is the oracle for (1) and is deliberately not the tick's
/// own executor: it is an OS thread that samples the elapsed time and reports
/// it, so a tick that never returns is caught by something that does not depend
/// on the tick.
#[test]
fn a_slow_source_does_not_block_an_independent_chain() -> TestResult {
    // Short enough that the test is quick, long enough that a healthy poll — which
    // answers on its first poll — is never at risk of being cut short by it.
    let deadline = Duration::from_millis(120);
    let slack = Duration::from_secs(20);
    let mut rig = wedged_pair(deadline)?;

    // A clean tick first, so both chains hold a committed baseline before the
    // wedging begins. Without it the first observation of each chain is the thing
    // under test, and a substrate whose *first* read did not fire would fail this
    // for a reason that has nothing to do with a source that stopped answering.
    // Establishing the baseline is also what the row is about: an independent
    // chain's work is a chain that has already been observed.
    tick(&mut rig.bot)?;

    rig.wedged.set(true);
    // The independent chain has work ready on the very tick the wedged one is
    // cancelled, so what is being checked is that it was *not stopped* rather
    // than that it had nothing to do anyway.
    rig.ready_value.set(3);

    let (elapsed, outcome) = time_a_tick(&mut rig.bot, deadline.saturating_add(slack));
    let report = rig.bot.tick_report();

    assert!(
        elapsed <= deadline.saturating_add(slack),
        "the tick took {elapsed:?} against a declared {deadline:?} per-poll deadline \
         and {slack:?} of slack: a source that never resolves still holds the tick"
    );

    // The independent chain committed and acted in the tick that cancelled its
    // sibling, which is the whole of the claim. It ran with `3`, not the `2` the
    // baseline tick committed: a chain that read the same value would not have
    // fired at all, and the row is about a chain that had work ready and was not
    // stopped from doing it.
    assert_eq!(
        ran_with(&rig.ready_seen),
        vec![2, 3],
        "the independent chain's action ran again in the same tick the wedged chain \
         was cancelled in"
    );
    assert!(
        rig.ready_entered.get() >= 2,
        "the independent chain's source was polled again: {} entries",
        rig.ready_entered.get()
    );

    // The wedged chain committed nothing and acted nothing on top of what the
    // baseline tick already fired.
    assert_eq!(
        ran_with(&rig.wedged_seen),
        vec![1],
        "a cancelled poll commits nothing, so the chain it belongs to added no \
         effect beyond the one its baseline tick fired: {:?}",
        ran_with(&rig.wedged_seen)
    );

    // Reported, naming the source that stopped answering rather than a chain
    // index the caller has to resolve itself.
    assert_eq!(
        report
            .stalled()
            .iter()
            .map(|row| (row.chain(), row.domain()))
            .collect::<Vec<_>>(),
        vec![(0, "test::wedged")],
        "the tick reports exactly the wedged chain, by chain index and by the \
         source's own domain_id"
    );
    assert_eq!(
        report.stalled()[0].deadline(),
        deadline,
        "the report names the budget that was actually applied, so a reader is not \
         guessing which bound cancelled the poll"
    );
    assert!(
        report.stalled_any(),
        "`stalled_any` and `stalled` are one fact read twice"
    );

    // And the tick's own result is a *typed* stall rather than a silent success:
    // the chain that did not read is a chain the caller has to know about.
    assert!(
        matches!(outcome, Some(EcsError::PollStalled { chain: 0, .. })),
        "the tick reports the cancellation with its own variant, naming the chain: \
         {outcome:?}"
    );
    Ok(())
}

/// A stalled chain is re-polled on the next tick, and commits when it answers.
///
/// The half that makes the stall a deferral rather than a retirement. A
/// cancellation that spent the chain's baseline — or cleared its forced-refresh
/// mark — would leave the source permanently unobserved, which is the quiet state
/// INV-BOT-120 exists to rule out, reached by a different road.
///
/// Three observations:
///
/// 1. the next tick **enters the poll body again**, from the counter. A substrate
///    that remembered the cancellation and skipped the source would never reach
///    the second entry.
/// 2. the chain **commits and fires** once the source answers.
/// 3. it is **no longer reported stalled** on the tick that read it, so the report
///    says the source recovered rather than leaving a stale cancellation up.
#[test]
fn a_stalled_chain_is_re_polled_and_commits_when_it_answers() -> TestResult {
    let deadline = Duration::from_millis(120);
    let mut rig = wedged_pair(deadline)?;

    // The clean baseline tick, for the same reason as the test above: a chain's
    // first observation is its own thing, and this row is about what happens to a
    // chain whose poll has already been cancelled once.
    tick(&mut rig.bot)?;

    rig.wedged.set(true);
    stalled_tick(&mut rig.bot)?;
    let after_stall = rig.bot.tick_report();
    assert_eq!(
        after_stall.stalled().len(),
        1,
        "the first tick cancelled chain 0's poll: {:?}",
        after_stall.stalled()
    );

    // The chain is declared unsound across the stall, so the mark has to survive
    // it: a cancellation that cleared the mark would let the next tick compare a
    // fresh read against the baseline the stall was supposed to replace.
    rig.wedged_reason.set(Some(RefreshReason::Disconnected));

    // The source recovers, with a *different* value, so a commit that did not
    // happen cannot be confused with a commit of the old baseline.
    rig.wedged.set(false);
    rig.wedged_value.set(7);
    tick(&mut rig.bot)?;

    assert!(
        rig.wedged_entered.get() >= 2,
        "the next tick entered the poll body again ({} entries): a substrate that \
         remembered the cancellation skipped the source",
        rig.wedged_entered.get()
    );
    assert_eq!(
        ran_with(&rig.wedged_seen),
        vec![1, 7],
        "the recovered chain committed the value it read and fired on it, on top \
         of the one its baseline tick fired"
    );
    let recovered = rig.bot.tick_report();
    assert!(
        recovered.stalled().is_empty(),
        "the tick that read the source reports no stall: {:?}",
        recovered.stalled()
    );
    assert!(
        recovered.forced().iter().any(|row| row.chain() == 0),
        "and the forced-refresh mark survived the cancellation rather than being \
         spent by a read that never committed: {:?}",
        recovered.forced()
    );
    Ok(())
}

/// A per-poll deadline of zero, or one past the ceiling, is refused at build.
///
/// The bound is only a bound if it cannot be set to something that is not one. A
/// zero budget cancels every poll before its first poll — a bot that observes
/// nothing and reports every chain as stalled — and a budget past the ceiling is
/// a caller asking for a bot that can still hang. Both are refused with a typed
/// error naming the number, because a silently clamped deadline is
/// indistinguishable from the one the caller asked for.
///
/// Run against the *real* builder rather than a helper, because the refusal is
/// what this observes and the builder is the only thing that can make it.
#[test]
fn a_poll_deadline_that_bounds_nothing_is_refused_at_build() -> TestResult {
    let zero = Bot::builder("t06-zero")
        .with_poll_deadline(Duration::ZERO)
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty());
    assert!(
        matches!(zero, Err(EcsError::PollDeadlineUnbounded { deadline }) if deadline.is_zero()),
        "a zero per-poll deadline is refused, naming the number: {zero:?}"
    );

    let past = MAX_POLL_DEADLINE.saturating_add(Duration::from_secs(1));
    let over = Bot::builder("t06-over")
        .with_poll_deadline(past)
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty());
    match over {
        Err(EcsError::PollDeadlineExceeded { deadline, ceiling }) => {
            assert_eq!(
                deadline, past,
                "the refusal names the budget that was asked for"
            );
            assert_eq!(
                ceiling, MAX_POLL_DEADLINE,
                "and the ceiling that would have been accepted, so the repair is a \
                 number rather than a guess"
            );
        }
        other => return Err(format!("a budget past the ceiling was accepted: {other:?}").into()),
    }

    // The declared default is inside both bounds, so a caller that never calls
    // `with_poll_deadline` gets a bounded poll rather than an unbounded one.
    let default = Bot::builder("t06-default")
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty());
    assert!(default.is_ok(), "the declared default builds: {default:?}");
    assert!(
        DEFAULT_POLL_DEADLINE > Duration::ZERO && DEFAULT_POLL_DEADLINE <= MAX_POLL_DEADLINE,
        "the default ({DEFAULT_POLL_DEADLINE:?}) is inside the bounds the builder \
         enforces, so the default build is a bounded one"
    );
    Ok(())
}

/// Run one tick while an independent OS thread watches the budget.
///
/// The instrument is a separate thread rather than a timer inside the tick, for
/// the reason `resume_liveness.rs` uses one: it must not be the thing being
/// measured. A tick that never returns leaves this call blocked on `bot.tick()`,
/// and the nextest timeout — not this test's own assertion — is what would then
/// report it, which is a measurement of the harness rather than of the substrate.
///
/// The returned `outcome` is the tick's own `Result`, and it is an `Option`
/// because a tick that never returned has none. The pair is returned together
/// so a caller asserts on the elapsed time and the typed error from one
/// observation rather than from two.
fn time_a_tick(bot: &mut Bot, budget: Duration) -> (Duration, Option<EcsError>) {
    let started = std::time::Instant::now();
    // A condition variable rather than a flag the sampler wakes to poll: the
    // sampler waits on the event itself, the caller's release, and the release
    // ends the wait at once.
    let release = Arc::new(Release {
        released: Mutex::new(false),
        bell: Condvar::new(),
    });
    let sampler = std::thread::Builder::new()
        .name("t06-tick-watchdog".into())
        .spawn({
            let release = Arc::clone(&release);
            move || sample_a_tick(budget, &release)
        });
    // Without a watchdog there is no independent oracle, so this is reported as a
    // maximal elapsed time rather than measured from inside the thing being
    // measured. The caller's budget assertion then fails on it, which is the
    // honest outcome: the row could not be evidenced on this host.
    let Ok(sampler) = sampler else {
        return (Duration::MAX, None);
    };
    let outcome = bot.tick().err();
    // Read the clock first, then release and join. A sampler is still waiting
    // until its budget elapses, and joining before reading would charge the tick
    // for the sampler finishing rather than for the tick finishing — which is how
    // a test of "this returns promptly" ends up measuring its own watchdog.
    let elapsed = started.elapsed();
    // A poisoned flag means the sampler already panicked, which the join below
    // reports; there is nothing left to release.
    if let Ok(mut released) = release.released.lock() {
        *released = true;
    }
    release.bell.notify_all();
    if sampler.join().is_err() {
        return (Duration::MAX, None);
    }
    (elapsed, outcome)
}

/// Wait on a plain OS thread that has no executor to stall, until the caller's
/// budget is spent or the tick it watches has returned.
///
/// In the ordinary case the caller's tick has already returned and rung the bell,
/// which ends the wait at once — the thread would otherwise outlive the answer by
/// spending the whole budget. The case the budget is actually for is a tick that
/// hangs: that is the failure being detected, and the thread is what detects it,
/// so it keeps waiting until the budget is spent.
///
/// This is not an executor thread and has nothing to stall: it is the independent
/// sampler, spawned to outlive whatever it measures. `rt::time::sleep` cannot be
/// used because it needs a reactor this thread deliberately does not run — a
/// watchdog sharing the tick's own driver cannot observe that tick failing to
/// make progress.
fn sample_a_tick(budget: Duration, release: &Release) {
    let Ok(guard) = release.released.lock() else {
        return;
    };
    // Whether the wait ended by the release, by the budget or by poison, the
    // sampler has nothing more to do: the caller reads the clock itself.
    drop(
        release
            .bell
            .wait_timeout_while(guard, budget, |released| !*released),
    );
}

/// The caller's release of the sampler: a flag and the bell that announces it.
struct Release {
    /// Whether the tick has returned.
    released: Mutex<bool>,
    /// Rung once the flag is set, so the sampler stops waiting at once.
    bell: Condvar,
}

/// Run one tick whose source is expected to be cancelled at its deadline, and
/// assert it said so.
///
/// The fourth distinct tick outcome in this file, beside a clean tick, a refusing
/// source and a held transition. It is a separate helper for the same reason the
/// other three are: a caller that came here to read a cancellation wants it
/// matched *exactly*, because `PollStalled` and `DomainError` are different
/// repairs — the first is a source that stopped answering, the second one that
/// refused — and a helper that accepted either would hide which one happened.
fn stalled_tick(bot: &mut Bot) -> TestResult {
    match bot.tick() {
        Ok(fired) => Err(format!(
            "a source that never resolved read as a {fired}-effect quiet tick; the \
             cancellation has to be reported"
        )
        .into()),
        Err(error) => {
            assert!(
                matches!(error, EcsError::PollStalled { .. }),
                "the cancellation is the substrate's own typed variant and keeps the \
                 budget that was applied: {error:?}"
            );
            Ok(())
        }
    }
}
