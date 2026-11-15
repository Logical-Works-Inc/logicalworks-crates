//! What counts as the same input: event identity against content identity
//! (issues #101 and #129).

use super::*;

/// `true` for the payload half alone, so a new event id with the old payload
/// still holds and a new payload does not.
fn payload_is(seen: &EventId<u32>, want: u32) -> bool {
    *seen.value() == want
}

/// A restarted bot must not read a *different* request as already applied.
///
/// Counterexample A in issue #101: process A observes request A, the action
/// lands and is journaled `Applied`. The journal is rebuilt while the source
/// now returns request B. A digest bound to the process-local `Revision`
/// counter derives the same identity for both (both are revision 1 after a
/// restart) and skips B. B must run.
#[test]
fn a_changed_request_after_reconstruction_is_not_skipped_as_applied() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(EventId::new(1, 10)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let identity = identity()?;

    let mut first = armed(
        Requests(Rc::clone(&seen)),
        |seen: &EventId<u32>| payload_is(seen, 10),
        Records(Rc::clone(&log)),
        scope(identity, Rc::clone(&store))?,
    )?;
    assert_eq!(first.tick()?, 1, "request A runs");
    assert_eq!(*log.borrow(), vec![EventId::new(1, 10)]);

    // A different request, with a different event id and a different payload.
    // The condition is the same declaration as A's ("this payload"), so what
    // is under test is the digest, not a second builder shape.
    *seen.borrow_mut() = EventId::new(2, 20);
    let mut second = armed(
        Requests(Rc::clone(&seen)),
        |seen: &EventId<u32>| payload_is(seen, 20),
        Records(Rc::clone(&log)),
        scope(identity, Rc::clone(&store))?,
    )?;
    assert_eq!(
        second.tick()?,
        1,
        "request B runs; it was not silently omitted as already applied"
    );
    assert_eq!(
        *log.borrow(),
        vec![EventId::new(1, 10), EventId::new(2, 20)],
        "and the receiver sees the exact values, in order"
    );
    Ok(())
}

/// Two events with equal payloads are two events.
///
/// Content equality is not event identity (issue #101). The id is part of
/// `EventId`'s `InputIdentity` and its `PartialEq`, so neither the change
/// filter nor the dispatch digest can collapse them.
#[test]
fn two_equal_payloads_with_distinct_event_ids_are_two_events() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(EventId::new(1, 7)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let identity = identity()?;

    let mut bot = armed(
        Requests(Rc::clone(&seen)),
        |_v: &EventId<u32>| true,
        Records(Rc::clone(&log)),
        scope(identity, Rc::clone(&store))?,
    )?;

    assert_eq!(bot.tick()?, 1, "event 1 runs");
    *seen.borrow_mut() = EventId::new(2, 7);
    assert_eq!(
        bot.tick()?,
        1,
        "event 2 runs even though the payload is equal"
    );
    assert_eq!(
        *log.borrow(),
        vec![EventId::new(1, 7), EventId::new(2, 7)],
        "two distinct events, both delivered"
    );
    Ok(())
}

/// A journal from another flow is refused at assembly.
///
/// The ownership identity is the whole of run + flow + environment, not the
/// two fields assembly used to check (issue #101).
#[test]
fn a_journal_from_another_flow_is_refused() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(EventId::new(1, 1)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let mine = identity()?;

    let mut first = armed(
        Requests(Rc::clone(&seen)),
        |_v: &EventId<u32>| true,
        Records(Rc::clone(&log)),
        scope(mine, Rc::clone(&store))?,
    )?;
    first.tick()?;
    drop(first);

    let other_flow = EffectIdentity::new(
        mine.run(),
        mine.environment(),
        FlowRevision::from_tagged(
            "blake3_256",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )?,
    );
    match Bot::builder(NAME)
        .observe(Requests(Rc::clone(&seen)))
        .on(|_v: &EventId<u32>| true, Records(Rc::clone(&log)))
        .with_effects(scope(other_flow, Rc::clone(&store))?)
        .build(&GrantSet::empty())
    {
        Err(EcsError::Kernel(KernelError::ActionNotDeclared { .. })) => Ok(()),
        other => Err(format!("expected a foreign-journal refusal, got {other:?}").into()),
    }
}

/// A source that returns to a previously landed `EventId` must retire rather
/// than refuse or re-enter.
///
/// The retire check is `Effects::applied_in`, which only knows what live
/// settlement folded into `applied`. A raw `OutcomeObserved` append left that
/// set empty, so the returning event minted `AttemptId::FIRST` again and the
/// journal answered `OutOfOrder` after the entry was already marked
/// `Unrecorded` — a false barrier instead of a retirement (issue #101).
#[test]
fn a_returning_landed_event_is_retired_not_refused() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(EventId::new(1, 10)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let identity = identity()?;

    let mut bot = every_request(
        Rc::clone(&seen),
        Records(Rc::clone(&log)),
        scope(identity, Rc::clone(&store))?,
    )?;

    assert_eq!(bot.tick()?, 1, "event 1 runs");
    *seen.borrow_mut() = EventId::new(2, 20);
    assert_eq!(bot.tick()?, 1, "event 2 runs");
    assert_eq!(
        *log.borrow(),
        vec![EventId::new(1, 10), EventId::new(2, 20)]
    );

    // Event 1 returns with the same identity. It already landed, so it retires.
    *seen.borrow_mut() = EventId::new(1, 10);
    match bot.tick() {
        Ok(fired) => {
            assert_eq!(fired, 0, "event 1 must be retired, not re-dispatched");
            assert_eq!(
                *log.borrow(),
                vec![EventId::new(1, 10), EventId::new(2, 20)],
                "no second delivery of event 1"
            );
            Ok(())
        }
        Err(error) => Err(format!(
            "expected retire/skip, got {error:?}; log={:?}",
            log.borrow()
        )
        .into()),
    }
}

/// A state watch that returns to a value it already landed must run again.
///
/// Content identity retires a redelivery of the same *event*. A state watch
/// `0 → 1 → 0` is not a redelivery: the third observation is a new episode of
/// work that happens to carry content the first one carried. Reading it as a
/// replay left the source stably at `0` while the receiver still held `1` — a
/// notification watch silently loses its second outage (issue #129).
///
/// The event-tagged opposite is
/// [`a_returning_landed_event_is_retired_not_refused`], which this test must
/// not disturb.
#[test]
fn a_state_watch_that_returns_to_a_landed_value_runs_again() -> TestResult {
    let watch = Watch::new()?;

    let mut bot = watch.bot()?;

    assert_eq!(bot.tick()?, 1, "episode 1: value 0 is new work");
    assert_eq!(watch.counted.get(), 1, "the receiver saw 0");

    watch.seen.set(1);
    assert_eq!(bot.tick()?, 1, "episode 2: value 1 is a movement");
    assert_eq!(watch.counted.get(), 2, "the receiver saw 1");

    // Back to 0. This is a change from the committed state 1, not a transport
    // redelivery of the first observation: no crash, no hash collision, no
    // concurrent controller. The content is the same as episode 1's, which is
    // exactly why content identity alone cannot be the retire test.
    watch.seen.set(0);
    assert_eq!(
        bot.tick()?,
        1,
        "episode 3: returning to 0 is a new episode, not a redelivery"
    );
    assert_eq!(
        watch.counted.get(),
        3,
        "dispatches [1, 1, 1], not [1, 1, 0]: the receiver must see 0 again"
    );

    let events = recorded(&watch.store)?;
    assert_eq!(
        events.len(),
        9,
        "three full intent/prepare/outcome ladders, one per episode: {events:?}"
    );
    Ok(())
}

/// A reconstructed bot that still sees the last applied state retires rather
/// than firing again.
///
/// The other half of the same distinction (issue #129): content identity is
/// what keeps a restart from double-firing on a source that never moved. A new
/// admission of the value the *last* applied episode carried is a resumption
/// of that episode's work, not a fresh one. Only an intervening different
/// value opens a new episode.
#[test]
fn a_reconstructed_bot_retires_an_unchanged_state_rather_than_firing_again() -> TestResult {
    let watch = Watch::new()?;

    let mut first = watch.bot()?;
    assert_eq!(first.tick()?, 1, "the first episode runs");
    assert_eq!(watch.counted.get(), 1);

    // The restart. The source is exactly where the applied episode left it.
    let mut second = watch.bot()?;
    assert_eq!(
        second.tick()?,
        0,
        "an unchanged source after reconstruction is work that already landed"
    );
    assert_eq!(
        watch.counted.get(),
        1,
        "no second delivery of the first episode: {:?}",
        watch.counted
    );

    // A movement after the restart is still new work.
    watch.seen.set(7);
    assert_eq!(
        second.tick()?,
        1,
        "a real movement still runs after a restart"
    );
    assert_eq!(watch.counted.get(), 2);
    Ok(())
}
