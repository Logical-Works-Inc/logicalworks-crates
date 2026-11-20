//! A recovered unknown is a barrier: it stands in front of a successor whether
//! the source moved, held still or failed to poll (issue #104).

use super::*;
use crate::sim::nth;

/// A source that refuses every poll.
struct FailsToPoll;

impl Observe for FailsToPoll {
    type Output = u32;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u32, EcsError> {
        call.0.check(Observe::required_caps(self))?;
        Err(EcsError::Kernel(KernelError::DomainError {
            domain: "test::fails_to_poll".to_owned(),
            certainty: DispatchCertainty::NotDelivered,
            cause: "the poll refused".to_owned(),
        }))
    }

    fn domain_id(&self) -> &str {
        "test::fails_to_poll"
    }
}

/// `true` only for `1`, so a source that moves to `2` makes it false.
fn is_one(seen: &u32) -> bool {
    *seen == 1
}

/// The handles the #104 reserve-then-publish scenarios watch: one shared
/// store, how often reserve and publish each ran, and the source's value,
/// which starts at `1`.
struct Reserve {
    store: Rc<RefCell<MemoryJournal>>,
    entered: Rc<Cell<u32>>,
    published: Rc<Cell<u32>>,
    value: Rc<Cell<u32>>,
    identity: EffectIdentity,
}

impl Reserve {
    /// Fresh handles under one identity, so every bot built from them resolves
    /// the others' keys.
    fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            store: Rc::new(RefCell::new(MemoryJournal::new())),
            entered: Rc::new(Cell::new(0)),
            published: Rc::new(Cell::new(0)),
            value: Rc::new(Cell::new(1)),
            identity: identity()?,
        })
    }

    /// The reserve-then-publish bot the #104 scenario is written against,
    /// observing this fixture's movable value.
    ///
    /// One chain, two entries: `(value == 1, reserve)` then `(always, publish)`.
    /// Shared by the barrier tests so a drift in the declaration would not be
    /// readable as a difference between them.
    fn bot(&self) -> Result<Bot, Box<dyn Error>> {
        self.bot_over(Shifting(Rc::clone(&self.value)))
    }

    /// [`Reserve::bot`] over any `u32` source, so a scenario whose source fails
    /// declares the very same chain rather than a copy of it.
    fn bot_over<S>(&self, source: S) -> Result<Bot, Box<dyn Error>>
    where
        S: Observe<Output = u32> + 'static,
    {
        Ok(Bot::builder(NAME)
            .observe(source)
            .on(
                is_one,
                NeverSettles {
                    entered: Rc::clone(&self.entered),
                },
            )
            .on(always, Counts(Rc::clone(&self.published)))
            .with_effects(scope(self.identity, Rc::clone(&self.store))?)
            .build(&GrantSet::empty())?)
    }
}

/// A recovered unknown is a barrier in front of a false condition.
///
/// Issue #104's concrete public scenario: the first bot observes `1`, reserve
/// reports `EffectIndeterminate`, and publish must not run. The journal is
/// rebuilt while the source yields `2`, so reserve's condition is now false.
/// The plan used to emit `Decision::Skip`, the walk used to mark the entry
/// `Skipped` before it ever consulted `effects.blocks`, and publish ran. A
/// changed source is not evidence that the earlier action did not occur.
#[test]
fn a_recovered_unknown_blocks_a_false_condition_and_its_successor() -> TestResult {
    let rig = Reserve::new()?;

    let mut first = rig.bot()?;

    expect_indeterminate(&mut first, "expected indeterminate")?;
    assert_eq!(
        rig.entered.get(),
        1,
        "reserve ran once, on the first attempt"
    );
    assert_eq!(
        rig.published.get(),
        0,
        "publish is behind reserve and must not have run"
    );
    let at_rest = recorded(&rig.store)?;
    assert_eq!(
        at_rest.len(),
        2,
        "intent and prepared dispatch, no outcome: {at_rest:?}"
    );
    let key = nth(&at_rest, 0)?.key();

    // The restart, with the source moved: reserve's condition is false now.
    rig.value.set(2);
    let mut second = rig.bot()?;

    // Immediately after reconstruction, before any poll: the barrier is
    // durable and does not wait for a transition to open. The successor is
    // not reported yet — it has no transition to be NotStarted in — and that
    // is the one moment the two can disagree with the post-tick list.
    let before = second.pending();
    assert_eq!(
        before.len(),
        1,
        "the recovered unknown is already reported: {before:?}"
    );
    let barrier = nth(&before, 0)?;
    assert_eq!(
        barrier.key(),
        Some(key),
        "and it names the key to settle by"
    );
    assert!(
        matches!(
            barrier.hold(),
            TransitionHold::UnsettledByRecovery { action } if *action == key.action()
        ),
        "held because an earlier attempt never settled: {:?}",
        barrier.hold()
    );

    match second.tick() {
        Err(EcsError::PendingTransition { work, outstanding }) => {
            // The barrier *and* the entry it blocks, which is the same pair
            // an abandonment reports. A count of 1 beside a two-entry hold
            // would read as "one thing is stuck", which is not what happened.
            assert_eq!(
                outstanding, 2,
                "the count and the report agree: the barrier and the entry behind it"
            );
            assert_eq!(
                work.id(),
                barrier.id(),
                "the tick names the barrier, not the successor behind it"
            );
            assert!(
                matches!(
                    work.hold(),
                    TransitionHold::UnsettledByRecovery { action } if *action == key.action()
                ),
                "the tick names the recovery hold, not a skip and not a fresh attempt: {:?}",
                work.hold()
            );
        }
        other => {
            return Err(format!("a false condition must not produce {other:?}").into());
        }
    }
    assert_eq!(
        rig.published.get(),
        0,
        "the successor did not run behind a recovered unknown"
    );
    assert_eq!(
        rig.entered.get(),
        1,
        "and the unknown attempt was not re-entered"
    );

    // A quiet tick does not erase the report.
    let quiet = second.tick();
    assert!(
        matches!(
            quiet,
            Err(EcsError::PendingTransition { outstanding: 2, .. })
        ),
        "a still source leaves the barrier standing: {quiet:?}"
    );
    let after = second.pending();
    assert_eq!(after.len(), 2, "still reported, barrier first: {after:?}");
    let first_held = nth(&after, 0)?;
    assert_eq!(first_held.key(), Some(key));
    assert!(
        matches!(
            first_held.hold(),
            TransitionHold::UnsettledByRecovery { .. }
        ),
        "{:?}",
        first_held.hold()
    );
    Ok(())
}
/// The same barrier in front of a *true* condition.
///
/// The other half of "independent of current condition": a condition that
/// holds would have planned an Attempt, and `run_chain`'s `blocks` guard
/// already stopped that. What it did not stop is the plan treating the
/// recovered work as fresh — and a tick that returned `EffectUnsettled` from
/// `begin` is still a re-entry of the dispatch path rather than a held report.
/// Either way publish must not run and the hold must be the recovery one.
#[test]
fn a_recovered_unknown_blocks_a_true_condition_and_its_successor() -> TestResult {
    let rig = Reserve::new()?;

    let mut first = rig.bot()?;
    expect_indeterminate(&mut first, "expected indeterminate")?;

    // Same value: reserve's condition still holds.
    let mut second = rig.bot()?;
    match second.tick() {
        Err(EcsError::PendingTransition { work, outstanding }) => {
            assert_eq!(
                outstanding, 2,
                "the barrier and the entry it blocks are both still reported"
            );
            assert!(
                matches!(work.hold(), TransitionHold::UnsettledByRecovery { .. }),
                "held for recovery, not re-planned as a fresh attempt: {:?}",
                work.hold()
            );
        }
        other => return Err(format!("expected the recovery hold, got {other:?}").into()),
    }
    assert_eq!(rig.published.get(), 0, "the successor did not run");
    assert_eq!(rig.entered.get(), 1, "reserve was not re-entered");
    Ok(())
}

/// A poll failure does not erase a recovered unknown.
///
/// The barrier has to survive a tick that never opens a transition at all:
/// the walk stops at the poll, and the only thing that still knows the effect
/// may be live is the journal.
#[test]
fn a_poll_failure_does_not_erase_a_recovered_unknown() -> TestResult {
    let rig = Reserve::new()?;

    let mut first = rig.bot()?;
    expect_indeterminate(&mut first, "expected indeterminate")?;
    let key = nth(&recorded(&rig.store)?, 0)?.key();

    let mut second = rig.bot_over(FailsToPoll)?;

    match second.tick() {
        Err(EcsError::Kernel(KernelError::DomainError { .. })) => {}
        other => return Err(format!("expected the poll's own failure, got {other:?}").into()),
    }
    let held = second.pending();
    assert_eq!(
        held.len(),
        1,
        "the recovered unknown survives the failed poll: {held:?}"
    );
    let entry = nth(&held, 0)?;
    assert_eq!(entry.key(), Some(key));
    assert!(
        matches!(entry.hold(), TransitionHold::UnsettledByRecovery { .. }),
        "{:?}",
        entry.hold()
    );
    assert_eq!(rig.published.get(), 0);
    Ok(())
}
