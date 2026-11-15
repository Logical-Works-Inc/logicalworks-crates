//! The durability an external effect needs: the handoff is admitted on the real
//! path, and a refusal leaves no false barrier behind (issue #100).

use super::scripted_journal::ScriptedJournal;
use super::*;

/// An action that would leave the process, and records whether it was entered.
struct ExternalMarker(Rc<Cell<bool>>);

impl Execute for ExternalMarker {
    type Input = EventId<u32>;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    // The default is already `External`; stated so the test's subject is
    // visible at the declaration rather than inferred from a missing override.
    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::External
    }

    async fn execute_action(&self, call: (Auth, &EventId<u32>)) -> Result<(), EcsError> {
        call.0.check(Execute::required_caps(self))?;
        self.0.set(true);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::external_marker"
    }
}

/// A bot whose one action is an external handoff that marks `entered`,
/// recorded through `journal`, under a broker that admits the identity's
/// environment: everything the handoff checks is the journal's answer.
fn handing_off(entered: &Rc<Cell<bool>>, journal: ScriptedJournal) -> Result<Bot, Box<dyn Error>> {
    let identity = identity()?;
    let mut broker = Broker::new();
    broker.register(identity.environment())?;
    every_request(
        Rc::new(RefCell::new(EventId::new(1, 1))),
        ExternalMarker(Rc::clone(entered)),
        EffectScope::new(identity, broker, Box::new(journal)),
    )
}

/// Whether the named kind was appended for any key.
///
/// The store's own error propagates: a read that failed is not a journal with no
/// such entry, and answering "no" here would let a dispatch test pass on a store
/// it could not read.
fn journal_has(
    store: &Rc<RefCell<MemoryJournal>>,
    kind: EventKind,
) -> Result<bool, Box<dyn std::error::Error>> {
    let committed = EffectJournal::committed(&*store.borrow())?;
    Ok(committed.iter().any(|event| event.kind() == kind))
}

/// An ephemeral scope refuses an external effect before the action runs.
///
/// The counterexample in issue #100: the in-memory ladder accepted the two
/// write-ahead events and the adapter was entered, so its filesystem effect
/// outlived the process despite the advertised refusal. The refusal is now on
/// the handoff path, and the marker is the receiver that must never appear.
#[test]
fn an_ephemeral_scope_refuses_an_external_effect_before_it_runs() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(EventId::new(1, 1)));
    let entered = Rc::new(Cell::new(false));
    let identity = identity()?;

    let mut bot = every_request(
        Rc::clone(&seen),
        ExternalMarker(Rc::clone(&entered)),
        scope(identity, store)?,
    )?;

    match bot.tick() {
        Err(EcsError::Kernel(KernelError::EffectRefused { .. })) => {}
        other => {
            return Err(format!("expected a handoff refusal, got {other:?}").into());
        }
    }
    assert!(
        !entered.get(),
        "the external action must not be entered when the journal cannot outlive the process"
    );
    Ok(())
}

/// A local effect still runs on an ephemeral scope.
///
/// The other half of the contract: `MemoryJournal` stays useful for explicitly
/// local work (issue #100).
#[test]
fn an_ephemeral_scope_runs_a_local_effect() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let seen = Rc::new(RefCell::new(EventId::new(1, 1)));
    let log = Rc::new(RefCell::new(Vec::new()));
    let identity = identity()?;

    let mut bot = every_request(
        Rc::clone(&seen),
        Records(Rc::clone(&log)),
        scope(identity, store)?,
    )?;

    assert_eq!(bot.tick()?, 1, "a declared-local action runs");
    assert_eq!(*log.borrow(), vec![EventId::new(1, 1)]);
    Ok(())
}

/// A journal whose acknowledgment is weaker than its advertisement is refused.
///
/// The handoff checks the per-append promise, not `durability()`: a store that
/// claims `ProcessCrash` and acks `Ephemeral` is weaker than it says (issue
/// #100).
#[test]
fn an_ack_weaker_than_the_advertised_grade_is_refused() -> TestResult {
    let entered = Rc::new(Cell::new(false));
    let mut bot = handing_off(
        &entered,
        ScriptedJournal::over(Rc::new(RefCell::new(MemoryJournal::new())))
            .advertising(DurabilityPromise::ProcessCrash)
            .acking(DurabilityPromise::Ephemeral),
    )?;

    match bot.tick() {
        Err(EcsError::Kernel(KernelError::EffectRefused { .. })) => {}
        other => {
            return Err(format!("expected the weak ack to be refused, got {other:?}").into());
        }
    }
    assert!(!entered.get(), "the action did not run on a weak ack");
    Ok(())
}

/// A handoff refused before the action ran must not leave a false
/// `Unrecorded` barrier, and a later tick must not treat the entry as
/// "may be live" (issue #100).
#[test]
fn a_refused_handoff_leaves_no_unrecorded_barrier() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let entered = Rc::new(Cell::new(false));
    let mut bot = handing_off(
        &entered,
        ScriptedJournal::over(Rc::clone(&store))
            .advertising(DurabilityPromise::Ephemeral)
            .acking(DurabilityPromise::Ephemeral),
    )?;

    match bot.tick() {
        Err(EcsError::Kernel(KernelError::EffectRefused { .. })) => {}
        other => return Err(format!("expected EffectRefused, got {other:?}").into()),
    }
    assert!(!entered.get(), "the action did not run");
    // `MemoryJournal` advertises `Ephemeral`, so admission refuses before the
    // intent is written. Nothing may be prepared either way.
    assert!(
        !journal_has(&store, EventKind::DispatchPrepared)?,
        "a handoff that cannot outlive the process must not be prepared"
    );

    match bot.tick() {
        Ok(_) => Ok(()),
        Err(EcsError::PendingTransition { work, outstanding }) => {
            let hold = format!("{:?}", work.hold());
            if hold.contains("Unrecorded")
                || hold.contains("OutcomeUnknown")
                || hold.contains("UnsettledByRecovery")
            {
                Err(format!(
                    "false barrier after handoff refusal: hold={hold} outstanding={outstanding}"
                )
                .into())
            } else {
                // A real hold is allowed; a false "may be live" is not.
                Ok(())
            }
        }
        // Refusing again is the configuration error still being true. It is
        // not the defect; the defect is a false unknown barrier.
        Err(EcsError::Kernel(KernelError::EffectRefused { .. })) => Ok(()),
        Err(other) => Err(format!("unexpected second tick error: {other:?}").into()),
    }
}

/// A weak per-append ack must be refused before `DispatchPrepared` is
/// committed, so recovery cannot fold a dispatch that never happened as an
/// unknown outcome (issue #100).
#[test]
fn a_weak_ack_does_not_record_dispatch_prepared() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let entered = Rc::new(Cell::new(false));
    let mut bot = handing_off(
        &entered, // Advertised as durable enough for an external handoff, but
        // every acknowledgment comes back `Ephemeral`.
        ScriptedJournal::over(Rc::clone(&store))
            .advertising(DurabilityPromise::ProcessCrash)
            .acking(DurabilityPromise::Ephemeral),
    )?;

    match bot.tick() {
        Err(EcsError::Kernel(KernelError::EffectRefused { .. })) => {}
        other => return Err(format!("expected EffectRefused, got {other:?}").into()),
    }
    assert!(!entered.get(), "the action did not run on a weak ack");
    assert!(
        journal_has(&store, EventKind::IntentAdmitted)?,
        "the intent is the write-ahead fact that was actually committed"
    );
    assert!(
        !journal_has(&store, EventKind::DispatchPrepared)?,
        "no DispatchPrepared for a handoff refused on its acknowledgment"
    );

    match bot.tick() {
        Ok(_) => Ok(()),
        Err(EcsError::PendingTransition { work, .. }) => {
            let hold = format!("{:?}", work.hold());
            if hold.contains("Unrecorded")
                || hold.contains("OutcomeUnknown")
                || hold.contains("UnsettledByRecovery")
            {
                Err(format!("weak ack left a false unknown barrier: {hold}").into())
            } else {
                Ok(())
            }
        }
        Err(EcsError::Kernel(KernelError::EffectRefused { .. })) => Ok(()),
        Err(other) => Err(format!("unexpected second tick error: {other:?}").into()),
    }
}
