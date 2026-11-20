//! One controller against another: a stale controller is fenced out of an
//! unknown effect, and a settled handoff passes ownership on (issue #120).

use super::scripted_journal::ScriptedJournal;
use super::*;
use crate::sim::nth;

/// The event-shaped counterpart of [`NeverSettles`].
struct NeverSettlesEvent(Rc<Cell<u32>>);

impl Execute for NeverSettlesEvent {
    type Input = EventId<u32>;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &EventId<u32>)) -> Result<(), EcsError> {
        call.0.check(Execute::required_caps(self))?;
        self.0.set(self.0.get().saturating_add(1));
        Err(EcsError::Kernel(KernelError::EffectIndeterminate {
            domain: "test::stale-controller".to_owned(),
            cause: "unknown".to_owned(),
        }))
    }

    fn domain_id(&self) -> &str {
        "test::never-settles-event"
    }
}

fn snapshot_race_scope(
    identity: EffectIdentity,
    store: Rc<RefCell<MemoryJournal>>,
    event: EffectEvent,
) -> Result<EffectScope, Box<dyn Error>> {
    Ok(EffectScope::new(
        identity,
        broker(identity.environment())?,
        Box::new(ScriptedJournal::over(store).injecting(event)),
    ))
}

/// A stale controller must not replace its recovery fold with a newly-read
/// tail. Controller A records an unknown handoff, while controller B still
/// holds the empty fold and observes a different input for the same action.
/// B's append is fenced before its receiver can run (issue #120).
#[test]
fn a_stale_controller_cannot_dispatch_past_another_controllers_unknown_effect() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let first_input = Rc::new(RefCell::new(EventId::new(1, 1)));
    let second_input = Rc::new(RefCell::new(EventId::new(2, 2)));
    let entered = Rc::new(Cell::new(0));
    let identity = identity()?;

    let mut first = every_request(
        Rc::clone(&first_input),
        NeverSettlesEvent(Rc::clone(&entered)),
        scope(identity, Rc::clone(&store))?,
    )?;
    let mut stale = every_request(
        second_input,
        NeverSettlesEvent(Rc::clone(&entered)),
        scope(identity, Rc::clone(&store))?,
    )?;

    assert!(matches!(
        first.tick(),
        Err(EcsError::Kernel(KernelError::EffectIndeterminate { .. }))
    ));
    assert_eq!(entered.get(), 1, "the first controller entered once");

    assert!(matches!(
        stale.tick(),
        Err(EcsError::Kernel(KernelError::EffectRefused {
            cause: DispatchError::Journal(JournalError::TailMismatch { .. })
        }))
    ));
    assert_eq!(
        entered.get(),
        1,
        "the stale controller must not dispatch a distinct input past the unknown effect"
    );
    Ok(())
}

/// Assembly rejects a journal that advances between its recovery fold and
/// retained append fence. Accepting that mixed snapshot would let the new bot
/// treat the injected outcome as absent while appending after it (issue #120).
#[test]
fn assembly_refuses_a_tail_that_advanced_after_recovery() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let entered = Rc::new(Cell::new(0));
    let identity = identity()?;
    let mut first = every_request(
        Rc::new(RefCell::new(EventId::new(1, 1))),
        NeverSettlesEvent(Rc::clone(&entered)),
        scope(identity, Rc::clone(&store))?,
    )?;

    assert!(matches!(
        first.tick(),
        Err(EcsError::Kernel(KernelError::EffectIndeterminate { .. }))
    ));
    let events = recorded(&store)?;
    assert_eq!(
        events.len(),
        2,
        "the unknown handoff has intent and prepare"
    );
    let key = nth(&events, 0)?.key();

    let result = Bot::builder(NAME)
        .observe(Requests(Rc::new(RefCell::new(EventId::new(2, 2)))))
        .on(
            |_seen: &EventId<u32>| true,
            NeverSettlesEvent(Rc::clone(&entered)),
        )
        .with_effects(snapshot_race_scope(
            identity,
            store,
            EffectEvent::OutcomeObserved {
                key,
                evidence: EffectEvidence::NotApplied,
            },
        )?)
        .build(&GrantSet::empty());

    assert!(matches!(
        result,
        Err(EcsError::Kernel(KernelError::EffectRefused {
            cause: DispatchError::Journal(JournalError::SnapshotStale {
                recovered_events: 2,
                committed_events: 3,
            }),
        }))
    ));
    assert_eq!(
        entered.get(),
        1,
        "assembly must not dispatch while recovering"
    );
    Ok(())
}

/// A later tail is not an acknowledgement for this controller's intent. The
/// foreign attempt is deliberately valid and unknown, so adopting its tail
/// would permit this receiver to run past a fact it never folded (issue #120).
/// The forged position holds the foreign attempt's event, so the refusal is
/// an entry mismatch at that position, not a missing event.
#[test]
fn a_forged_intent_acknowledgement_cannot_launder_a_foreign_unknown_effect() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let entered = Rc::new(Cell::new(0));
    let identity = identity()?;
    let mut bot = every_request(
        Rc::new(RefCell::new(EventId::new(1, 1))),
        NeverSettlesEvent(Rc::clone(&entered)),
        EffectScope::new(
            identity,
            broker(identity.environment())?,
            Box::new(ScriptedJournal::over(store).with_entries().forging()),
        ),
    )?;

    assert!(matches!(
        bot.tick(),
        Err(EcsError::Kernel(KernelError::EffectRefused {
            cause: DispatchError::Journal(JournalError::EntryMismatch { .. })
        }))
    ));
    assert_eq!(
        entered.get(),
        0,
        "a forged acknowledgement cannot reach the receiver"
    );
    Ok(())
}

/// Sequential ownership transfer still works: a settled handoff does not fence
/// out the successor (issue #120 acceptance).
///
/// This is the positive control for the three refusals above. Without it a
/// fence that rejected every second dispatch would pass them all. Controller A
/// runs a definite effect to completion, so the journal holds intent, prepare
/// and an `Applied` outcome rather than an unknown. Controller B is built over
/// the same store *after* those events — its recovery fold is the advanced
/// tail, not the empty one the stale controller held — and dispatches a
/// different input for the same action. B's receiver runs.
#[test]
fn a_successor_dispatches_after_a_settled_handoff() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let first_input = Rc::new(RefCell::new(EventId::new(1, 1)));
    let second_input = Rc::new(RefCell::new(EventId::new(2, 2)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let identity = identity()?;

    let mut first = every_request(
        Rc::clone(&first_input),
        Records(Rc::clone(&log)),
        scope(identity, Rc::clone(&store))?,
    )?;
    let fired = first.tick()?;
    assert_eq!(fired, 1, "the first controller dispatched its one action");
    assert_eq!(
        log.borrow().as_slice(),
        [EventId::new(1, 1)],
        "the first receiver saw its input"
    );
    let after_first = recorded(&store)?;
    assert_eq!(
        after_first.len(),
        3,
        "intent, prepare, and a settled outcome: {after_first:?}"
    );
    let settled = nth(&after_first, 2)?;
    assert!(
        matches!(
            *settled,
            EffectEvent::OutcomeObserved {
                evidence: EffectEvidence::Applied,
                ..
            }
        ),
        "a definite effect settles as applied, which is what makes this a transfer: {settled:?}"
    );
    let first_key = nth(&after_first, 0)?.key();

    // The successor is built over the advanced journal, so its fold includes
    // the settled attempt rather than remaining ignorant of it.
    let mut successor = every_request(
        second_input,
        Records(Rc::clone(&log)),
        scope(identity, Rc::clone(&store))?,
    )?;
    let fired = successor.tick()?;
    assert_eq!(
        fired, 1,
        "a successor that folded the settled handoff is not fenced out"
    );
    assert_eq!(
        log.borrow().as_slice(),
        [EventId::new(1, 1), EventId::new(2, 2)],
        "both receivers ran, in ownership order"
    );
    let after_second = recorded(&store)?;
    assert_eq!(
        after_second.len(),
        6,
        "the successor appended its own ladder: {after_second:?}"
    );
    let successor_key = nth(&after_second, 3)?.key();
    assert_eq!(successor_key, nth(&after_second, 4)?.key());
    assert_eq!(nth(&after_second, 4)?.key(), nth(&after_second, 5)?.key());
    assert_ne!(
        successor_key, first_key,
        "a distinct input is a distinct key, which is what makes the stale-controller \
         falsifier a real one and this transfer a real transfer"
    );
    Ok(())
}

/// How many ticks the continuing controller is driven through.
///
/// Each tick settles one local effect in three events, so at the sixty-four-event
/// trigger below this is a little over four continuations: enough that the
/// controller has crossed a boundary *after* already crossing one, which is the
/// case a fence that stayed on the predecessor's chain would fail.
const CONTINUING_TICKS: u64 = 96;

/// The event trigger the continuing controller's journal is opened with.
const CONTINUING_EVENT_WATERMARK: u64 = 64;

/// The controller continues its own journal on the shipped append path.
///
/// Nothing in this test asks for a continuation. A bot is built over a continuing
/// file journal and ticked, and every continuation is the one `ecs`'s own append
/// takes when the journal's watermark is due (#267). So the assertions are about
/// what a host sees afterwards: every tick dispatched exactly once, the chain of
/// files moved past its base more than once, and a reopen through the original
/// path finds a live journal with nothing left uncertain.
#[test]
fn the_controller_continues_its_own_journal_on_the_shipped_append_path() -> TestResult {
    let dir = crate::journal_fixtures::scratch_dir("controller-continues")?;
    let _guard = crate::journal_fixtures::TempGuard(dir.clone());
    let base = dir.join("run.jrnl");
    let policy = ContinuationPolicy::new(CONTINUING_EVENT_WATERMARK, 4 * 1024 * 1024)?;
    let journal = FileJournal::open_continuing_with(&base, policy)?;
    let identity = identity()?;
    let input = Rc::new(RefCell::new(EventId::new(0, 0)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut bot = every_request(
        Rc::clone(&input),
        Records(Rc::clone(&log)),
        EffectScope::new(identity, broker(identity.environment())?, Box::new(journal)),
    )?;

    for tick in 1..=CONTINUING_TICKS {
        let payload = u32::try_from(tick)?;
        *input.borrow_mut() = EventId::new(tick, payload);
        assert_eq!(
            bot.tick()?,
            1,
            "tick {tick} dispatched its one action across the continuations"
        );
    }
    drop(bot);

    assert_eq!(
        log.borrow().len(),
        usize::try_from(CONTINUING_TICKS)?,
        "every tick entered its action exactly once: no continuation dropped or \
         replayed an effect"
    );
    let live = FileJournal::open_active(&base)?;
    assert!(
        live.generation() > 2,
        "the controller's own appends continued its journal more than once; the live \
         journal is generation {}",
        live.generation()
    );
    assert!(
        live.recover().uncertain().is_empty(),
        "every effect the controller dispatched is settled in the journal it ended on"
    );
    Ok(())
}
