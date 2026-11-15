//! T08: a declared failure forces a refresh, not a permanent quiet state.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::{Bot, GrantSet, RefreshReason};

use crate::declarable::Declarable;
use crate::effects::memory_scope;

use super::{Lands, TestResult, ran_with, refused_tick, tick};

/// Each of T08's four causes, as the pair a test needs to arm one.
///
/// Written as a table rather than four hand-written tests because the row is a
/// claim about *all four*: a repair that handles disconnect and silently misses
/// the overflow passes any test that only checks disconnect.
const CAUSES: [(RefreshReason, &str); 4] = [
    (RefreshReason::Disconnected, "disconnected"),
    (RefreshReason::WatchOverflow, "watch-overflow"),
    (RefreshReason::StaleRemoteKey, "stale-remote-key"),
    (RefreshReason::InvalidationFailed, "invalidation-failed"),
];

/// The handles a test needs to arm one declarable source and read what it did.
///
/// A struct rather than a five-tuple: the tuple's shape is written three times
/// across this file, and a named field is what makes `polls` distinguishable
/// from `value` at a call site rather than by position.
struct DeclarableRig {
    /// The bot under test.
    bot: Bot,
    /// The value the source reports.
    value: Rc<Cell<u32>>,
    /// What `cache_state` answers.
    reason: Rc<Cell<Option<RefreshReason>>>,
    /// Whether the source refuses to read.
    refusing: Rc<Cell<bool>>,
    /// How many times the source was polled.
    polls: Rc<Cell<u32>>,
    /// Every value the action ran with, in order.
    seen: Rc<RefCell<Vec<u32>>>,
}

/// Build a one-chain bot over a declarable source.
fn declarable_bot(domain: &'static str) -> Result<DeclarableRig, Box<dyn Error>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let source = Declarable::new(1, domain);
    let (value, reason, refusing, polls) = source.handles();
    let bot = Bot::builder(domain)
        .observe(source)
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&seen),
            },
        )
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?;
    Ok(DeclarableRig {
        bot,
        value,
        reason,
        refusing,
        polls,
        seen,
    })
}

/// A source whose baseline the substrate is told is unsound is re-read on the
/// next tick, and the tick report names the cause.
///
/// The counterexample this closes: a source reports `Disconnected`, the
/// substrate keeps the value it last saw, and because a re-read would have
/// compared *equal* against that value the substrate never looks again. The bot
/// goes quiet and the only thing a caller can see is zero fired effects, which
/// is indistinguishable from a healthy source that held still.
#[test]
fn a_declared_failure_forces_a_refresh_rather_than_a_permanent_quiet_state() -> TestResult {
    for (cause, spelling) in CAUSES {
        let mut rig = declarable_bot("test::t08_source")?;

        // One clean tick: the source is read, the value commits, the entry runs,
        // and the chain settles. Nothing is armed, so nothing should be forced.
        tick(&mut rig.bot)?;
        let baseline_polls = rig.polls.get();
        assert!(
            baseline_polls >= 1,
            "{spelling}: the baseline tick must have polled the source at least once"
        );
        assert!(
            !rig.bot.tick_report().forced_any(),
            "{spelling}: an undeclared source is not forced — a bot that forces \
             everything would satisfy this row without reading anything"
        );
        let fired_before = ran_with(&rig.seen).len();
        assert!(
            fired_before >= 1,
            "{spelling}: the baseline tick must have fired its entry once"
        );

        // The value has not changed at all. The source says its baseline cannot
        // be trusted, and the substrate must re-read anyway: with the value
        // unchanged, a correct implementation sees "equal" here and fires
        // nothing, which is the right answer — and it only knows it is right
        // because it looked.
        rig.value.set(1);
        rig.reason.set(Some(cause));
        tick(&mut rig.bot)?;

        assert!(
            rig.polls.get() > baseline_polls,
            "{spelling}: a source that declared its cache unsound was not re-read; \
             polls stayed at {}",
            rig.polls.get()
        );
        let report = rig.bot.tick_report();
        let forced = report
            .forced()
            .first()
            .ok_or_else(|| format!("{spelling}: the tick report names no forced refresh"))?;
        assert_eq!(
            forced.reason(),
            cause,
            "{spelling}: the report must name the cause the source declared"
        );
        assert_eq!(
            forced.chain(),
            0,
            "{spelling}: the forced chain is the one whose source declared it"
        );
        assert_eq!(
            forced.domain(),
            "test::t08_source",
            "{spelling}: the report names the source by its own domain identifier"
        );
        assert_eq!(
            ran_with(&rig.seen).len(),
            fired_before,
            "{spelling}: a forced re-read of an unchanged value fires nothing — the \
             value really did not move"
        );
    }
    Ok(())
}

/// A forced refresh commits the value the source reported, not the one it
/// invalidated.
///
/// The half of T08 that is easy to get wrong in the other direction: a repair
/// that re-reads but then compares the fresh read against the baseline it
/// already distrusted would keep the stale value forever and re-observe on every
/// tick without ever converging. Here the value moves *while* the cache is
/// declared unsound, so the forced tick has to both report the forced refresh and
/// commit the newer value.
#[test]
fn a_forced_refresh_commits_the_newer_value_and_then_returns_to_quiet() -> TestResult {
    let mut rig = declarable_bot("test::t08_move")?;

    tick(&mut rig.bot)?;
    assert_eq!(
        ran_with(&rig.seen),
        vec![1],
        "the first generation runs on 1"
    );

    // The baseline is unsound *and* the value moved. The forced tick has to
    // commit 2, not 1: 1 is exactly the value the declaration says cannot be
    // trusted.
    rig.value.set(2);
    rig.reason.set(Some(RefreshReason::WatchOverflow));
    tick(&mut rig.bot)?;

    assert_eq!(
        ran_with(&rig.seen),
        vec![1, 2],
        "the forced refresh must commit the value the source reported, not the one \
         it declared unsound"
    );
    assert!(
        rig.bot.tick_report().forced_any(),
        "the tick that re-read a declared-unsound source reports itself as one"
    );
    let after_move = rig.polls.get();

    // The declaration is spent: the next tick has a sound baseline again, so it
    // is not forced, and a value that has not moved fires nothing. This is the
    // difference between "forced a refresh" and "stayed permanently invalid",
    // and it is the half a test that only checks the first tick would miss.
    rig.reason.set(None);
    tick(&mut rig.bot)?;
    assert!(
        !rig.bot.tick_report().forced_any(),
        "a source that declared nothing is not forced: the previous declaration was \
         spent by the read it forced"
    );
    assert_eq!(
        rig.polls.get(),
        after_move.saturating_add(1),
        "the quiet tick still polls — an unforced tick that skipped the poll would \
         pass this assertion for the wrong reason"
    );
    assert_eq!(
        ran_with(&rig.seen),
        vec![1, 2],
        "a quiet tick fires nothing, and says so rather than looking quiet because \
         it stopped looking"
    );
    Ok(())
}

/// A source whose poll fails keeps its baseline marked, so a *failed* refresh is
/// not mistaken for a completed one.
///
/// The failure mode this closes: the mark is spent by the forced poll, the poll
/// fails, and the next tick compares against the same unsound baseline it was
/// supposed to replace. The bot is quiet again for exactly the reason it was
/// quiet before, and the report says nothing.
#[test]
fn a_failed_refresh_keeps_the_baseline_marked() -> TestResult {
    let mut rig = declarable_bot("test::t08_fail")?;
    tick(&mut rig.bot)?;
    let baseline = rig.polls.get();

    rig.reason.set(Some(RefreshReason::Disconnected));
    tick(&mut rig.bot)?;
    assert!(
        rig.polls.get() > baseline,
        "the armed source must be re-read"
    );
    assert_eq!(
        rig.bot
            .tick_report()
            .forced()
            .first()
            .map(|forced| forced.reason()),
        Some(RefreshReason::Disconnected),
        "the tick report names the cause"
    );

    // The source now refuses, *and* keeps declaring. The forced read never
    // landed, so the mark has to outlive the tick that made it.
    rig.refusing.set(true);
    refused_tick(&mut rig.bot)?;

    // Re-read again: the baseline is still unsound because the forced poll that
    // would have replaced it did not succeed. `declared_refresh` skips a chain
    // that is already marked, so the mark the refusal left standing is what
    // forces this one — not the reason the test happened to re-arm.
    rig.reason.set(None);
    let before_retry = rig.polls.get();
    refused_tick(&mut rig.bot)?;
    assert!(
        rig.polls.get() > before_retry,
        "a refresh that failed must leave the baseline marked, or the bot returns \
         to quiet against the value it just said it could not trust: polls stayed \
         at {}",
        rig.polls.get()
    );
    assert_eq!(
        rig.bot
            .tick_report()
            .forced()
            .first()
            .map(|forced| forced.reason()),
        Some(RefreshReason::Disconnected),
        "and the report still names it — a source that declared nothing on this \
         tick is still carrying the mark its failed refresh left"
    );
    Ok(())
}

/// Two tenants' sources over one bot: one chain's declared failure forces that
/// chain's refresh and leaves the other's quiet.
///
/// The two-tenant case, and the reason the mark is per chain rather than per
/// bot: a bot-wide flag would make one source's transport failure re-read every
/// other source forever, which is the same permanent-quiet defect wearing the
/// opposite face.
#[test]
fn two_tenants_sources_forced_refreshes_stay_attributed_to_their_own_chain() -> TestResult {
    let quiet_value = Rc::new(Cell::new(1));
    let quiet_polls = Rc::new(Cell::new(0));
    let quiet_seen = Rc::new(RefCell::new(Vec::new()));
    let broken_value = Rc::new(Cell::new(1));
    let broken_reason = Rc::new(Cell::new(None));
    let broken_polls = Rc::new(Cell::new(0));
    let broken_seen = Rc::new(RefCell::new(Vec::new()));

    let mut bot = Bot::builder("t08-tenants")
        .observe(Declarable::over(
            Rc::clone(&quiet_value),
            Rc::new(Cell::new(None)),
            Rc::new(Cell::new(false)),
            Rc::clone(&quiet_polls),
            "test::tenant_quiet",
        ))
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&quiet_seen),
            },
        )
        .observe(Declarable::over(
            Rc::clone(&broken_value),
            Rc::clone(&broken_reason),
            Rc::new(Cell::new(false)),
            Rc::clone(&broken_polls),
            "test::tenant_broken",
        ))
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&broken_seen),
            },
        )
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?;

    tick(&mut bot)?;
    assert_eq!(ran_with(&quiet_seen), vec![1]);
    assert_eq!(ran_with(&broken_seen), vec![1]);

    let quiet_before = quiet_polls.get();
    broken_reason.set(Some(RefreshReason::StaleRemoteKey));
    tick(&mut bot)?;

    let report = bot.tick_report();
    assert_eq!(
        report.forced().len(),
        1,
        "exactly the chain that declared is forced, not the whole bot: {:?}",
        report.forced()
    );
    let forced = report.forced().first().ok_or("the broken chain is named")?;
    assert_eq!(
        forced.chain(),
        1,
        "the forced chain is the one whose source declared it"
    );
    assert_eq!(
        forced.domain(),
        "test::tenant_broken",
        "and it is named by its own domain, not the sibling's"
    );
    assert_eq!(forced.reason(), RefreshReason::StaleRemoteKey);
    assert!(
        broken_polls.get() > 1,
        "the declared-unsound source was re-read"
    );
    assert_eq!(
        quiet_polls.get(),
        quiet_before.saturating_add(1),
        "the sibling source is polled exactly once more, like any quiet tick — a \
         per-bot flag would re-read it forever"
    );
    assert_eq!(
        report.superseded(),
        &[],
        "a forced refresh is not a supersession: the value was equal, so nothing \
         was passed over"
    );
    Ok(())
}
