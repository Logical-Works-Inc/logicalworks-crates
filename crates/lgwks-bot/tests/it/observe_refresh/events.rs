//! T09: event identity and latest-state supersession.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lgwks_bot::effect::EventId;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::spec::Bot;

use crate::declarable::Declarable;
use crate::effects::memory_scope;
use crate::event_feed::{EventFeed, LandsEvents};

use super::{NeverSettles, TestResult, held_tick, ran_with, tick};

/// The handles a test needs to queue events and read what ran.
struct EventRig {
    /// The bot under test.
    bot: Bot,
    /// The events still to be delivered, in order.
    queue: Rc<RefCell<Vec<EventId<u32>>>>,
    /// Every event id the action ran with, in order.
    seen: Rc<RefCell<Vec<u64>>>,
}

/// Build a one-chain bot over an event feed.
fn event_bot(domain: &'static str) -> Result<EventRig, Box<dyn std::error::Error>> {
    let queue = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let bot = Bot::builder(domain)
        .observe(EventFeed {
            queue: Rc::clone(&queue),
            polls: Rc::new(Cell::new(0)),
            domain,
        })
        .on(
            |event: &EventId<u32>| event.id() > 0,
            LandsEvents {
                seen: Rc::clone(&seen),
                domain: "test::lands_events",
            },
        )
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?;
    Ok(EventRig { bot, queue, seen })
}

/// Two events over equal payloads are two events, and a redelivery of one is
/// one.
///
/// The row's own distinction, exercised in both directions in one run because
/// they are the same mechanism read twice: the admitted-input identity of an
/// `EventId` carries its event id, so two ids over equal payloads derive
/// different identities and both execute, while the same id delivered again
/// derives the same one and retires. A bot that keyed dedup on the payload would
/// fire the first and silently drop the second.
#[test]
fn identical_payloads_with_distinct_event_ids_both_execute_and_a_redelivery_does_not_t09()
-> TestResult {
    let mut rig = event_bot("test::t09_feed")?;

    // Event 1 over payload 7, then event 2 over the *same* payload.
    rig.queue.borrow_mut().push(EventId::new(1, 7));
    tick(&mut rig.bot)?;
    assert_eq!(*rig.seen.borrow(), vec![1], "the first event executes");

    rig.queue.borrow_mut().push(EventId::new(2, 7));
    tick(&mut rig.bot)?;
    assert_eq!(
        *rig.seen.borrow(),
        vec![1, 2],
        "a second event id over an identical payload is a different event and must \
         execute; the payload alone cannot decide this"
    );

    // The same event, delivered again. The feed is drained, so the redelivery is
    // queued explicitly.
    rig.queue.borrow_mut().push(EventId::new(2, 7));
    tick(&mut rig.bot)?;
    assert_eq!(
        *rig.seen.borrow(),
        vec![1, 2],
        "a redelivery of the same event must not execute it a second time"
    );

    // And the first event comes round again, to show the dedup is over the whole
    // tagged history rather than over the last event only.
    rig.queue.borrow_mut().push(EventId::new(1, 7));
    tick(&mut rig.bot)?;
    assert_eq!(
        *rig.seen.borrow(),
        vec![1, 2],
        "a redelivery of an *older* event must not execute either — the fence is \
         over the events that have landed, not the most recent one"
    );
    Ok(())
}

/// An intermediate value overtaken before any entry acted on it is reported as
/// superseded, not as a fired effect and not as a retire.
///
/// The situation is a generation held open: the first value's action reports an
/// indeterminate outcome, so the entry is held and the transition keeps its
/// binding. A second value then arrives, and a third before the held one is
/// settled. The substrate admits only the newest once the walk releases the
/// chain, so the second value is passed over entirely — and the row asks for that
/// to be *visible*, because "fired twice" would claim work the bot never did and
/// "fired once" would hide a value the caller never saw.
#[test]
fn an_intermediate_value_is_reported_as_superseded_rather_than_fired_or_retired() -> TestResult {
    let value = Rc::new(Cell::new(1));
    let reason = Rc::new(Cell::new(None));
    let polls = Rc::new(Cell::new(0));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut bot = Bot::builder("t09-latest")
        .observe(Declarable::over(
            Rc::clone(&value),
            Rc::clone(&reason),
            Rc::new(Cell::new(false)),
            Rc::clone(&polls),
            "test::t09_latest",
        ))
        .on(
            |observed: &u32| *observed > 0,
            NeverSettles {
                seen: Rc::clone(&seen),
            },
        )
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?;

    // Generation 1 opens and its action is held: the outcome is unknown, so the
    // entry stays outstanding and the transition keeps value 1 bound to it.
    held_tick(&mut bot)?;
    assert_eq!(
        ran_with(&seen),
        vec![1],
        "the first generation ran its action against value 1"
    );
    assert!(
        !bot.pending().is_empty(),
        "the entry is held: an indeterminate outcome is not a decision"
    );

    // Value 2 commits while generation 1 is still held. It cannot be admitted
    // over the binding, so it waits in the slot, unacted-on. The held generation
    // is still the one the walk reaches, so the tick reports the hold again
    // rather than a clean run.
    value.set(2);
    held_tick(&mut bot)?;
    assert_eq!(
        ran_with(&seen),
        vec![1],
        "the held generation keeps its own binding: value 2 must not reach an entry \
         that was acknowledged against value 1"
    );

    // Value 3 arrives before generation 1 releases. Now value 2 is passed over:
    // the substrate admits only the newest when the chain comes free, and nothing
    // ever acted on 2.
    value.set(3);
    held_tick(&mut bot)?;

    let report = bot.tick_report();
    assert_eq!(
        ran_with(&seen),
        vec![1],
        "nothing new ran: the held entry is still held"
    );
    assert!(
        report.superseded_any(),
        "an intermediate value overtaken before it was acted on must be reported; \
         report was {report:?}"
    );
    let superseded = report
        .superseded()
        .first()
        .ok_or("the pass-over is named")?;
    assert_eq!(
        superseded.chain(),
        0,
        "the pass-over names the chain whose value was overtaken"
    );
    assert_eq!(
        superseded.revision(),
        2,
        "and names the revision the *replaced* value was committed under — the one \
         that will never be acted on"
    );
    assert!(
        !report.forced_any(),
        "a pass-over is not a forced refresh: nothing about the source's caching \
         was in question"
    );
    assert_eq!(
        report.fired(),
        0,
        "a passed-over value is not work done, so it is not counted as a fired effect"
    );

    // Settle the held generation. Settling it admits the newest committed
    // observation — 3, which took the intermediate's place — and that opens the
    // next generation, so this is the tick the action runs for real. It runs
    // with `NeverSettles`, so it holds again; what matters here is the value it
    // ran with, which is the one that survived rather than the one that was
    // passed over.
    let held = bot
        .pending()
        .first()
        .cloned()
        .ok_or("the held entry is reported")?;
    let key = held
        .key()
        .ok_or("a held entry with an attempt carries its key")?;
    bot.resolve_effect(&key, lgwks_bot::spec::EffectEvidence::Applied)?;
    assert_eq!(
        bot.pending().len(),
        0,
        "the settlement decides the generation it names, and reports the chain as \
         free of held work"
    );

    held_tick(&mut bot)?;
    assert_eq!(
        ran_with(&seen),
        vec![1, 3],
        "settling generation 1 admits the newest committed value — 3 — and that is \
         the one that runs. Value 2 never ran, which is what the pass-over reported."
    );
    // No pass-over on the settling tick. Settling generation 1 admits value 3 and
    // runs it, so nothing was overtaken: a pass-over is a value that was never
    // acted on, and 3 was. Reporting one here would be the same defect in the
    // other direction — a healthy bot claiming it skipped a state it did not.
    assert_eq!(
        bot.tick_report().superseded(),
        &[],
        "admitting the newest value is not itself a pass-over: value 3 was admitted \
         here, so nothing unacted was replaced"
    );
    Ok(())
}
