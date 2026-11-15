//! Write-ahead and restart: what is recorded before the effect leaves the
//! process, and what a reconstructed bot does with the record it finds.

use super::*;

/// The intent and the prepared dispatch are both committed before the effect
/// runs, and the outcome is committed after it.
///
/// The ordering is asserted from inside the action rather than after the tick,
/// because that is what makes the claim mean "before the handoff". A journal
/// written after the effect returned would satisfy an after-the-tick assertion
/// just as well, and would be no record at all: the crash it is meant to
/// survive is the one that happens while the bytes are in flight.
#[test]
fn the_dispatch_intent_is_recorded_before_the_effect_leaves_the_process() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(None));
    let key = Rc::new(RefCell::new(None));

    let mut bot = armed(
        FixedSource,
        is_value,
        NoteWhatIsRecorded {
            store: Rc::clone(&store),
            seen: Rc::clone(&seen),
            key: Rc::clone(&key),
        },
        scope(identity()?, Rc::clone(&store))?,
    )?;

    let fired = bot.tick()?;
    assert_eq!(fired, 1, "the one chain must dispatch its one action");

    let at_handoff_seen = seen
        .borrow()
        .clone()
        .ok_or("the action never ran, so the write-ahead claim was never measured")?;
    assert_eq!(
        at_handoff_seen.as_slice(),
        [EventKind::IntentAdmitted, EventKind::DispatchPrepared],
        "at the instant the effect ran, the journal must already hold the intent and the \
         prepared dispatch for it, and nothing else: an append that landed later would be \
         no record of the crash this exists to survive"
    );

    let at_handoff = key
        .borrow()
        .ok_or("the journal held no event to take a key from")?;
    let after = recorded(&store)?;
    assert_eq!(
        after.len(),
        3,
        "the outcome is committed after the handoff, so the run must end with exactly one \
         more event than the action saw: {after:?}"
    );
    assert_eq!(after[0].key(), at_handoff);
    assert_eq!(after[1].key(), at_handoff);
    assert_eq!(after[2].key(), at_handoff);
    assert_eq!(
        after[2].kind(),
        EventKind::OutcomeObserved,
        "the third append is what the attempt became, and it is the only one that may \
         follow the handoff"
    );
    assert!(
        matches!(
            after[2],
            EffectEvent::OutcomeObserved {
                evidence: EffectEvidence::Applied,
                ..
            }
        ),
        "an action that returned Ok landed, and the journal must say so: {:?}",
        after[2]
    );
    Ok(())
}

/// A bot rebuilt against a journal refuses to begin an attempt the journal
/// records as dispatched with no outcome, and reports it instead.
///
/// The counter is the assertion that matters. A substrate that had retried
/// would leave the same pending report behind — the entry is held either way —
/// so "how many times did the effect run" is the only observable that
/// separates a hold from a resend.
#[test]
fn a_restart_holds_an_uncertain_attempt_rather_than_resending_it() -> TestResult {
    let run = Run::new();

    let mut first = run.never()?;

    expect_indeterminate(
        &mut first,
        "an effect that may or may not have happened must be reported as \
                 indeterminate",
    )?;
    assert_eq!(
        run.entered.get(),
        1,
        "the action is entered once, on the first attempt"
    );

    let at_rest = recorded(&run.store)?;
    assert_eq!(
        at_rest.len(),
        2,
        "an indeterminate effect records no outcome, so the journal holds exactly the \
         intent and the prepared dispatch: {at_rest:?}"
    );
    let key = at_rest[0].key();
    assert_eq!(
        at_rest[1].key(),
        key,
        "both events are about the one attempt"
    );

    // The restart: same identity, same journal, a new broker and a new bot.
    let mut second = run.never()?;

    let refused = second.tick();
    match refused {
        Err(EcsError::PendingTransition { work, outstanding }) => {
            assert_eq!(
                outstanding, 1,
                "the one held entry must be counted, and nothing else must be reported \
                 as open"
            );
            assert_eq!(
                work.key(),
                Some(key),
                "the refusal must name the key the caller settles by; a report that named \
                 the entry alone would leave the caller deriving an identity the journal \
                 already has"
            );
            assert!(
                matches!(
                    work.hold(),
                    TransitionHold::UnsettledByRecovery { action } if *action == key.action()
                ),
                "the refusal must say the entry is held because an earlier attempt never \
                 settled, rather than reading as a fresh attempt or as an exhausted \
                 budget: {:?}",
                work.hold()
            );
        }
        other => {
            return Err(format!(
                "a tick that finds a recovered unknown must refuse and name it, not \
                 dispatch it or report a clean tick; it returned {other:?}"
            )
            .into());
        }
    }
    assert_eq!(
        run.entered.get(),
        1,
        "the effect body must not have been entered a second time: a resend is the \
         duplicate merge this refusal exists to prevent"
    );

    let held = second.pending();
    assert_eq!(
        held.len(),
        1,
        "the held attempt must be reported, not silently absorbed: {held:?}"
    );
    assert_eq!(
        held[0].key(),
        Some(key),
        "the report a caller acts on must name the same key the tick refused with"
    );
    assert!(
        matches!(
            held[0].hold(),
            TransitionHold::UnsettledByRecovery { action } if *action == key.action()
        ),
        "the pending report must agree with the refusal about why the entry is held: {:?}",
        held[0].hold()
    );
    Ok(())
}

/// Evidence that the effect did not land lets a recovered attempt be tried
/// again, under a new attempt identity.
///
/// Without this, `resolve_effect`'s `NotApplied` would be a promise the durable
/// path cannot keep: the entry would be "eligible for an attempt again" and the
/// attempt would then be refused, because a restart that mints attempt one
/// again reproduces a key the journal has already walked past. The journal
/// allows one walk of its ladder per key, so continuing a run means continuing
/// its attempt sequence.
#[test]
fn evidence_that_the_effect_did_not_land_lets_a_recovered_attempt_be_tried_again() -> TestResult {
    let run = Run::new();

    let mut first = run.never()?;
    expect_indeterminate(
        &mut first,
        "the first run's effect must be reported as indeterminate",
    )?;
    let key = recorded(&run.store)?
        .first()
        .map(|event| event.key())
        .ok_or("the first attempt must have been recorded")?;

    let mut second = run.never()?;
    match second.tick() {
        Err(EcsError::PendingTransition { .. }) => {}
        other => {
            return Err(format!(
                "the first tick of the restart must hold the recovered attempt and say \
                 so; it returned {other:?}"
            )
            .into());
        }
    }
    assert_eq!(
        run.entered.get(),
        1,
        "the first tick of the restart must hold, not resend"
    );

    second.resolve_effect(&key, EffectEvidence::NotApplied)?;
    let settled = recorded(&run.store)?;
    assert_eq!(
        settled.last().map(|event| event.kind()),
        Some(EventKind::OutcomeObserved),
        "settling a recovered attempt writes the fact down; a bot that only dropped the \
         key from its own list would come back with the same unknown it just cleared"
    );

    let retried = second.tick();
    assert!(
        !matches!(
            retried,
            Err(EcsError::Kernel(KernelError::EffectRefused { .. }))
        ),
        "evidence that nothing was delivered makes the entry eligible for another \
         attempt, and the second attempt must not be refused by the journal as a repeat \
         of the first: {retried:?}"
    );
    assert_eq!(
        run.entered.get(),
        2,
        "the action must have been entered again: the attempt the caller repaired has to \
         reach the effect, not stop at the record"
    );
    assert_eq!(
        recorded(&run.store)?.len(),
        5,
        "the second attempt is a new key and walks the ladder from the beginning, so the \
         journal holds the first attempt's intent, dispatch and outcome and then the \
         second attempt's intent and dispatch"
    );
    Ok(())
}

/// A recovered attempt whose effect the journal records as landed is retired,
/// not dispatched again.
///
/// The crash this covers is the benign one: the effect landed and the record of
/// it landed too, and only the process died. Re-dispatching on the way back
/// would be a duplicate of an effect nothing is uncertain about — and skipping
/// the entry without retiring it would leave the chain reporting the same
/// finished work as outstanding forever.
#[test]
fn a_recovered_attempt_whose_effect_landed_is_retired_rather_than_sent_again() -> TestResult {
    let run = Run::new();

    let mut first = run.lands()?;
    let fired = first.tick()?;
    assert_eq!(fired, 1, "the effect lands on the run before the crash");
    assert_eq!(run.entered.get(), 1);

    let mut second = run.lands()?;

    let refused = second.tick();
    assert!(
        !matches!(
            refused,
            Err(EcsError::Kernel(KernelError::EffectRefused { .. }))
        ),
        "a crash after the record of an effect landed must not make the run unable to \
         continue past that entry: {refused:?}"
    );
    assert_eq!(
        run.entered.get(),
        1,
        "the journal records the effect as landed, so nothing may send it again"
    );
    assert!(
        second.pending().is_empty(),
        "the entry is done for the generation its effect landed in, so nothing may be \
         reported as outstanding: {:?}",
        second.pending()
    );
    Ok(())
}

/// A live settlement is journaled before it is acknowledged, so a restart does
/// not resurrect an attempt this process already settled.
///
/// The counterexample (issue #106): `Ledger::settle` used to mutate only
/// in-memory state and return `Ok`, while the durable record still said
/// `DispatchPrepared` with no outcome. Rebuilding then restored the old
/// unknown and blocked the action again. The repair unifies live and recovered
/// settlement through one `OutcomeObserved` append that happens *before*
/// either is acknowledged.
#[test]
fn a_live_settlement_is_journaled_before_it_is_acknowledged() -> TestResult {
    let run = Run::new();

    let mut first = run.never()?;

    expect_indeterminate(&mut first, "expected indeterminate")?;
    let at_rest = recorded(&run.store)?;
    assert_eq!(
        at_rest.len(),
        2,
        "intent and prepared dispatch: {at_rest:?}"
    );
    let key = at_rest[0].key();

    let held = first.pending();
    assert_eq!(held.len(), 1, "one live unknown: {held:?}");
    assert_eq!(held[0].key(), Some(key));

    // Live settlement. The call must write the fact down, not only move memory.
    first
        .resolve_effect(&key, EffectEvidence::Applied)
        .map_err(|error| format!("live resolve failed: {error:?}"))?;
    let after = recorded(&run.store)?;
    assert_eq!(
        after.len(),
        3,
        "the settlement is a committed event, not an in-memory ack: {after:?}"
    );
    assert!(
        matches!(
            after[2],
            EffectEvent::OutcomeObserved {
                evidence: EffectEvidence::Applied,
                ..
            }
        ),
        "the third event is the settlement fact: {:?}",
        after[2]
    );

    // Reconstruct against the same journal. The settled attempt must not come
    // back as an unknown.
    let mut second = run.never()?;

    assert!(
        second.pending().is_empty(),
        "a journaled live settlement must not resurrect as an unknown: {:?}",
        second.pending()
    );
    assert_eq!(
        run.entered.get(),
        1,
        "and nothing may re-enter the action that already ran"
    );

    // A repeat of the same evidence is idempotent, before and after restart.
    first
        .resolve_effect(&key, EffectEvidence::Applied)
        .map_err(|error| format!("repeat on first failed: {error:?}"))?;
    second
        .resolve_effect(&key, EffectEvidence::Applied)
        .map_err(|error| format!("repeat on second failed: {error:?}"))?;
    let final_events = recorded(&run.store)?;
    assert_eq!(
        final_events.len(),
        3,
        "a repeated settlement does not append twice: {final_events:?}"
    );
    Ok(())
}
