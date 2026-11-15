//! T06: a saturated chain does not starve an independent one.
//!
//! What this row can and cannot mean here, stated before the tests rather than
//! discovered while writing them.
//!
//! The row asks two things. The first is **saturated task A cannot starve
//! independent B** — a chain at its capacity ceiling must not stop an unrelated
//! chain from completing. That is about the *walk*: a chain whose entries are all
//! held open, and which therefore occupies the transition slot on every tick,
//! must not prevent a different chain's entries from being reached and run. Both
//! tests below are that claim, and both hold.
//!
//! The second is **one slow source does not block unrelated ready work** — and
//! that is proved by the per-poll deadline, in `slow_source` and `watchdogs`. The
//! gap it closed was real: a tick's observation phase joins every source in a wave
//! with `lgwks_std::task::join_all_boxed`, which polls them on the calling thread
//! and returns only when the whole wave has resolved, so `MAX_IN_FLIGHT_POLLS`
//! bounded the fan-out but not the wait, and a source that never resolved held the
//! tick with every other chain's action behind it.
//!
//! The repair is a per-poll deadline rather than a per-wave one, which is the
//! narrower of the two: it bounds the one poll that is stuck and leaves its
//! siblings free, where a wave deadline would cancel every source in the wave
//! whenever any one of them was slow. It is a policy the bot *declares* through
//! its own builder (`EcsBuilder::with_poll_deadline`) rather than a second
//! admission surface beside `GrantSet`, which is the shape this crate keeps: a
//! budget the substrate applies to work it already owns, not one a caller has to
//! grant a source permission to declare.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::{Bot, GrantSet};

use crate::declarable::Declarable;
use crate::effects::memory_scope;

use super::{Lands, NeverSettles, TestResult, held_tick, ran_with};

/// The handles a test needs to read two chains.
struct PairRig {
    /// The bot under test.
    bot: Bot,
    /// The value the independent chain's source reports.
    ready_value: Rc<Cell<u32>>,
    /// How many times the saturated chain's source was polled.
    saturated_polls: Rc<Cell<u32>>,
    /// How many times the independent chain's source was polled.
    ready_polls: Rc<Cell<u32>>,
    /// Every value the saturated chain's action ran with.
    saturated_seen: Rc<RefCell<Vec<u32>>>,
    /// Every value the independent chain's action ran with.
    ready_seen: Rc<RefCell<Vec<u32>>>,
}

/// Build a two-chain bot: chain 0's action never settles, chain 1's always does.
///
/// Chain 0 is the saturated one and chain 1 the independent one, declared
/// through the same builder call chain — so there is one bot, one world and one
/// schedule step. The point of T06 is that these two live *on one substrate*,
/// not that they are two processes that happen to interleave.
///
/// `NeverSettles` is what holds chain 0 at capacity. Its entry goes to
/// `OutcomeUnknown` and stays there, so the chain has a transition it can never
/// drop: every tick walks it, finds it held, and stops there. That is the
/// saturation, and it is *bounded* — the transition is one state per entry of
/// the spec, so a held chain costs its declared slots and nothing more.
fn saturated_pair(name: &'static str) -> Result<PairRig, Box<dyn Error>> {
    let saturated_value = Rc::new(Cell::new(1));
    let saturated_polls = Rc::new(Cell::new(0));
    let saturated_seen = Rc::new(RefCell::new(Vec::new()));
    let ready_value = Rc::new(Cell::new(2));
    let ready_polls = Rc::new(Cell::new(0));
    let ready_seen = Rc::new(RefCell::new(Vec::new()));

    let bot = Bot::builder(name)
        .observe(Declarable::over(
            Rc::clone(&saturated_value),
            Rc::new(Cell::new(None)),
            Rc::new(Cell::new(false)),
            Rc::clone(&saturated_polls),
            "test::saturated",
        ))
        .on(
            |observed: &u32| *observed > 0,
            NeverSettles {
                seen: Rc::clone(&saturated_seen),
            },
        )
        .observe(Declarable::over(
            Rc::clone(&ready_value),
            Rc::new(Cell::new(None)),
            Rc::new(Cell::new(false)),
            Rc::clone(&ready_polls),
            "test::independent",
        ))
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&ready_seen),
            },
        )
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?;
    Ok(PairRig {
        bot,
        ready_value,
        saturated_polls,
        ready_polls,
        saturated_seen,
        ready_seen,
    })
}

/// A chain held at capacity does not stop an independent chain from completing.
///
/// The oracle is the log of what ran, not a clock. Chain 0's action reports an
/// indeterminate outcome, so its entry is held for the rest of the run and the
/// walk stops there on every tick — that is the saturation. Chain 1's action
/// lands. If the held chain could starve the independent one, chain 1's log
/// would be empty and this fails on an exact value rather than on a timing.
///
/// The second half is the half a single tick would miss: the held chain is
/// re-walked and re-holds on *every* later tick, and the independent chain runs
/// again on each. Saturation that starves is saturation that persists, so a test
/// that only checks the first tick would pass against a repair that fixed one
/// tick's ordering and left the next one starved.
#[test]
fn a_chain_held_at_capacity_does_not_starve_an_independent_chain() -> TestResult {
    let mut rig = saturated_pair("t06-saturated")?;

    // Three ticks over a still-moving world: the first opens both generations
    // and the next two prove the hold persists without costing chain 1 anything.
    for tick_index in 1..=3_u32 {
        // Chain 0 re-holds, so its own action's error is the first failure in
        // declaration order on every tick the hold persists; chain 1 lands
        // regardless.
        held_tick(&mut rig.bot)?;
        assert_eq!(
            rig.saturated_polls.get(),
            tick_index,
            "the saturated chain's source is polled on every tick: a source that \
             stopped being read would make the saturation vacuous"
        );
        assert_eq!(
            rig.ready_polls.get(),
            tick_index,
            "the independent chain's source is polled on every tick too — a walk \
             that stopped at the held chain would never reach it"
        );
        assert_eq!(
            ran_with(&rig.saturated_seen),
            vec![1],
            "the saturated chain's action ran once across three ticks and its \
             outcome is held throughout: a second run would be a replay of an \
             effect that may already have happened"
        );
        assert_eq!(
            ran_with(&rig.ready_seen),
            vec![2],
            "the independent chain's value held still, so its entry is already \
             settled: a saturated chain cannot starve an independent one, and a \
             quiet one does not re-fire either"
        );
    }

    // The independent chain is the one that moves. It runs a new generation
    // while chain 0 is still held, which is the ordering claim rather than a
    // side effect of it: a walk that stopped at the held chain would leave this
    // log at one entry however ready chain 1 is.
    rig.ready_value.set(3);
    held_tick(&mut rig.bot)?;
    assert_eq!(
        ran_with(&rig.ready_seen),
        vec![2, 3],
        "the independent chain runs a new generation while the other chain is \
         still held at capacity"
    );
    assert_eq!(
        ran_with(&rig.saturated_seen),
        vec![1],
        "and the held chain still has not re-entered its action"
    );

    assert!(
        !rig.bot.pending().is_empty(),
        "the saturated chain's entry is still held after four ticks — this is the \
         capacity being occupied, not a tick that failed"
    );
    let held = rig.bot.pending();
    assert!(
        held.iter().all(|work| work.id().chain() == 0),
        "the only held work is chain 0's: a hold that spread to chain 1 would be \
         starvation wearing a different name — {:?}",
        held
    );
    Ok(())
}

/// A chain held at capacity does not starve an independent chain, at every
/// saturation tier the host can reach.
///
/// 100, 1,000 and 10,000 held chains declared on **one** bot alongside one
/// independent chain. Each held chain occupies a transition that the walk
/// reaches and cannot drop, so the walk's work grows with the mass while the
/// independent chain's share of it does not. The claim is that the independent
/// chain's action still runs, at every tier, and that every held chain is still
/// reported as held rather than silently dropped to make room for it.
///
/// The tier reached is recorded rather than clamped, so a host that reaches only
/// 100 does not read as though it reached 10,000.
#[test]
fn a_saturated_mass_does_not_starve_an_independent_chain_at_every_tier() -> TestResult {
    let mut reached: Vec<usize> = Vec::new();

    for requested in [100_usize, 1_000, 10_000] {
        let mass = Rc::new(RefCell::new(Vec::new()));
        let independent = Rc::new(RefCell::new(Vec::new()));

        // The independent chain is declared first, so it is chain 0 and its
        // effect is the first the walk reaches — the position a starved walk
        // would lose.
        let independent_polls = Rc::new(Cell::new(0));
        let mut builder = Bot::builder("t06-tiers").observe(Declarable::over(
            Rc::new(Cell::new(1)),
            Rc::new(Cell::new(None)),
            Rc::new(Cell::new(false)),
            Rc::clone(&independent_polls),
            "test::independent",
        ));
        builder = builder.on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::clone(&independent),
            },
        );

        for _ in 0..requested {
            let seen = Rc::clone(&mass);
            builder = builder
                .observe(Declarable::over(
                    Rc::new(Cell::new(1)),
                    Rc::new(Cell::new(None)),
                    Rc::new(Cell::new(false)),
                    Rc::new(Cell::new(0)),
                    "test::saturated",
                ))
                .on(|observed: &u32| *observed > 0, NeverSettles { seen });
        }

        let mut bot = builder
            .with_effects(memory_scope()?)
            .build(&GrantSet::empty())?;

        // The independent chain's own effect lands, so its entry never holds; the
        // saturated chains each hold and report their own indeterminate outcome,
        // and chain 0 is first in declaration order, so that is what the tick
        // reports. The chains behind it still ran.
        held_tick(&mut bot)?;

        assert_eq!(
            bot.source_domains().len(),
            requested.saturating_add(1),
            "the bot declares one independent chain and {requested} saturated ones"
        );
        assert_eq!(
            ran_with(&independent),
            vec![1],
            "at {requested} saturated chains the independent chain's action still ran"
        );
        assert_eq!(
            ran_with(&mass).len(),
            requested,
            "every saturated chain reached its own action once: a walk that stopped \
             at the first hold would have run one, not {requested}"
        );
        assert_eq!(
            bot.pending().len(),
            requested,
            "and every one of them is still reported as held rather than dropped to \
             make room — a dropped hold is a lost effect nobody would ever see"
        );
        reached.push(requested);
    }

    assert_eq!(
        reached,
        vec![100, 1_000, 10_000],
        "the tiers this run reached, recorded rather than clamped, so a reader is \
         never told a concurrency number nobody ran"
    );
    Ok(())
}
