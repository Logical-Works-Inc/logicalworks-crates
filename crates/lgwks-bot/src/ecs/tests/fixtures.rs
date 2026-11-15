//! The sources and actions the ECS tests share.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::Poll;

use crate::cap::{Auth, Cap};
use crate::ecs::{EcsBot, EcsError, PendingWork, TransitionHold};
use crate::error::{DispatchCertainty, KernelError};
use crate::verb::{EffectLifetime, Execute, Observe};

/// A domain refusal from a test source, recorded before it is returned.
///
/// Every fixture that refuses says the same three things — which domain, that
/// nothing was delivered, and why — so they say it here once.
pub(super) fn domain_refusal<T>(domain: &str, cause: &str) -> Result<T, EcsError> {
    let refusal = Err(EcsError::Kernel(KernelError::DomainError {
        domain: domain.into(),
        certainty: DispatchCertainty::NotDelivered,
        cause: cause.into(),
    }));
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "a test source refused its poll");
    refusal
}

/// A scripted source: yields the next value from `values` on each poll,
/// and refuses a poll past the end of its script rather than inventing one.
///
/// Deliberately `Rc`-based and therefore `!Send`, which is the property the
/// verbs actually have and the reason this whole module routes through
/// `NonSend` resources.
pub(super) struct Script {
    pub(super) values: Rc<RefCell<Vec<u16>>>,
    pub(super) cursor: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl Script {
    pub(super) fn new(values: Vec<u16>) -> Self {
        Self {
            values: Rc::new(RefCell::new(values)),
            cursor: Rc::new(Cell::new(0)),
            caps: vec![Cap::net()],
        }
    }
}

impl Observe for Script {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        let index = self.cursor.get();
        self.cursor.set(index.saturating_add(1));
        match self.values.borrow().get(index).copied() {
            Some(value) => Ok(value),
            None => domain_refusal("test::script", "polled past the end of its script"),
        }
    }

    fn domain_id(&self) -> &str {
        "test::script"
    }
}

/// A source that reports one value and then holds it for every later poll.
///
/// The counterpart to [`Script`], which advances on every call. A chain over
/// a scripted source moves on every tick, so it cannot tell "the chain ran
/// once" from "the chain runs every tick" — both look like work. This one
/// moves exactly once and then stands still, which is the shape that makes a
/// chain re-running settled work visible as a count rather than as a
/// coincidence.
pub(super) struct Holds {
    pub(super) value: u16,
    pub(super) caps: Vec<Cap>,
}

impl Holds {
    pub(super) fn new(value: u16) -> Self {
        Self {
            value,
            caps: vec![Cap::net()],
        }
    }
}

impl Observe for Holds {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        Ok(self.value)
    }

    fn domain_id(&self) -> &str {
        "test::holds"
    }
}

/// A source that counts how often it is asked for a value, and can answer
/// "has anything moved?" without being asked at all.
///
/// The counter is the point. `Holds` proves *what* a settled chain does;
/// this proves *how much it costs* — specifically that a quiet tick does
/// not call `poll`, which is the difference between the tick paying for a
/// value and the tick paying for a `u128`.
pub(super) struct Counted {
    pub(super) value: Rc<Cell<u16>>,
    pub(super) polls: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl Counted {
    pub(super) fn new(value: u16, polls: Rc<Cell<usize>>) -> Self {
        Self {
            value: Rc::new(Cell::new(value)),
            polls,
            caps: vec![Cap::net()],
        }
    }
}

impl Observe for Counted {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        self.polls.set(self.polls.get().saturating_add(1));
        Ok(self.value.get())
    }

    fn fingerprint(&self) -> Option<u128> {
        Some(u128::from(self.value.get()))
    }

    fn domain_id(&self) -> &str {
        "test::counted"
    }
}

/// A source that reports a change tick as well as a value.
pub(super) struct Rev {
    pub(super) value: Rc<Cell<u64>>,
    pub(super) rev: Rc<Cell<u64>>,
    pub(super) polls: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl Observe for Rev {
    type Output = u64;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u64, EcsError> {
        call.0.check(&self.caps)?;
        self.polls.set(self.polls.get().saturating_add(1));
        Ok(self.value.get())
    }

    fn revision(&self) -> Option<u64> {
        Some(self.rev.get())
    }

    fn domain_id(&self) -> &str {
        "test::rev"
    }
}

/// An action over a `u64`, counting its own calls.
pub(super) struct CountU64(pub(super) Rc<Cell<usize>>);

impl Execute for CountU64 {
    type Input = u64;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u64)) -> Result<(), EcsError> {
        call.0.check(&[])?;
        self.0.set(self.0.get().saturating_add(1));
        Ok(())
    }

    fn domain_id(&self) -> &'static str {
        "test::count_u64"
    }
}

/// A source that fails once its script runs out.
pub(super) struct Exhausting {
    pub(super) remaining: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl Observe for Exhausting {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        let left = self.remaining.get();
        self.remaining.set(left.saturating_sub(1));
        if left == 0 {
            return domain_refusal("test::exhausting", "script exhausted");
        }
        Ok(200)
    }

    fn domain_id(&self) -> &str {
        "test::exhausting"
    }
}

/// A source with a digest that can fail on demand and counts its polls.
///
/// The fingerprint is pure and answers even while `fail` is set, which is
/// the contract `Observe::fingerprint` states and the shape that makes the
/// sibling-poll bug reachable: the digest moves (or never committed), the
/// poll runs and fails, and a cache that published the digest early would
/// skip the source on every later tick.
pub(super) struct Switched {
    pub(super) value: Rc<Cell<u16>>,
    pub(super) fail: Rc<Cell<bool>>,
    pub(super) polls: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl Switched {
    pub(super) fn new(value: u16, fail: bool, polls: Rc<Cell<usize>>) -> Self {
        Self {
            value: Rc::new(Cell::new(value)),
            fail: Rc::new(Cell::new(fail)),
            polls,
            caps: vec![Cap::net()],
        }
    }
}

impl Observe for Switched {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        self.polls.set(self.polls.get().saturating_add(1));
        if self.fail.get() {
            return domain_refusal("test::switched", "switch is off");
        }
        Ok(self.value.get())
    }

    fn fingerprint(&self) -> Option<u128> {
        // No digest while the switch is off: a source that cannot produce a
        // value has none to offer a cheap key for, and `None` is the
        // contract's "must poll" answer. A stable digest across a failure
        // would let the cache skip the source and swallow the error.
        if self.fail.get() {
            None
        } else {
            Some(u128::from(self.value.get()))
        }
    }

    fn domain_id(&self) -> &str {
        "test::switched"
    }
}

/// A source that returns `Pending` once and then answers, with a digest.
///
/// Exists so a tick can be dropped mid-poll without hanging the test: the
/// first `poll` of the source's future parks, the caller drops the tick,
/// and the second tick finds the yield already spent and completes.
pub(super) struct Yielding {
    pub(super) value: u16,
    pub(super) yielded: Rc<Cell<bool>>,
    pub(super) polls: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl Yielding {
    pub(super) fn new(value: u16, yielded: Rc<Cell<bool>>, polls: Rc<Cell<usize>>) -> Self {
        Self {
            value,
            yielded,
            polls,
            caps: vec![Cap::net()],
        }
    }
}

impl Observe for Yielding {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        self.polls.set(self.polls.get().saturating_add(1));
        let flag = Rc::clone(&self.yielded);
        std::future::poll_fn(move |cx| {
            if flag.get() {
                Poll::Ready(())
            } else {
                flag.set(true);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        Ok(self.value)
    }

    fn fingerprint(&self) -> Option<u128> {
        Some(u128::from(self.value))
    }

    fn domain_id(&self) -> &str {
        "test::yielding"
    }
}

/// A source whose poll deliberately spans two externally controlled state
/// changes. Its digest remains an honest read of the live value; the test
/// uses the pauses to place those reads around the A → B → A schedule.
pub(super) struct AbaSource {
    pub(super) value: Rc<Cell<u16>>,
    pub(super) phase: Rc<Cell<u8>>,
    pub(super) polls: Rc<Cell<usize>>,
    pub(super) caps: Vec<Cap>,
}

impl AbaSource {
    pub(super) fn new(value: Rc<Cell<u16>>, phase: Rc<Cell<u8>>, polls: Rc<Cell<usize>>) -> Self {
        Self {
            value,
            phase,
            polls,
            caps: vec![Cap::net()],
        }
    }
}

impl Observe for AbaSource {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        self.polls.set(self.polls.get().saturating_add(1));
        let value = Rc::clone(&self.value);
        let phase = Rc::clone(&self.phase);
        let captured = Rc::new(Cell::new(0));
        let captured = std::future::poll_fn(move |cx| match phase.get() {
            0 => {
                phase.set(1);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            1 => {
                captured.set(value.get());
                phase.set(2);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            _ => {
                phase.set(0);
                Poll::Ready(captured.get())
            }
        })
        .await;
        Ok(captured)
    }

    fn fingerprint(&self) -> Option<u128> {
        Some(u128::from(self.value.get()))
    }

    fn domain_id(&self) -> &str {
        "test::aba"
    }
}

/// An action that counts how many times it ran.
pub(super) struct Count(pub(super) Rc<Cell<usize>>);

impl Execute for Count {
    type Input = u16;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u16)) -> Result<(), EcsError> {
        call.0.check(&[])?;
        self.0.set(self.0.get().saturating_add(1));
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::count"
    }
}

/// An action that records what it saw, so a test can assert on a
/// *persistent* effect rather than on a call count.
pub(super) struct Record(pub(super) Rc<RefCell<Vec<u16>>>);

impl Execute for Record {
    type Input = u16;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u16)) -> Result<(), EcsError> {
        call.0.check(&[])?;
        self.0.borrow_mut().push(*call.1);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::record"
    }
}

/// An action that fails a fixed number of times and then records: the
/// retry half of a chain, with a persistent effect to assert on.
pub(super) struct Flaky {
    pub(super) remaining: Rc<Cell<usize>>,
    pub(super) log: Rc<RefCell<Vec<u16>>>,
    pub(super) kind: FlakyKind,
}

/// Which failure the action reports, because the two are not the same
/// question and the substrate must treat them differently.
#[derive(Clone, Copy)]
pub(super) enum FlakyKind {
    /// The effect definitely did not happen.
    Refused,
    /// The effect may or may not have happened.
    Indeterminate,
}

impl Execute for Flaky {
    type Input = u16;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u16)) -> Result<(), EcsError> {
        call.0.check(&[])?;
        let left = self.remaining.get();
        if left == 0 {
            self.log.borrow_mut().push(*call.1);
            return Ok(());
        }
        self.remaining.set(left.saturating_sub(1));
        Err(match self.kind {
            FlakyKind::Refused => EcsError::Kernel(KernelError::DomainError {
                domain: "test::flaky".into(),
                certainty: DispatchCertainty::NotDelivered,
                cause: "refused, and the effect did not happen".into(),
            }),
            FlakyKind::Indeterminate => EcsError::Kernel(KernelError::EffectIndeterminate {
                domain: "test::flaky".into(),
                cause: "the acknowledgment never arrived".into(),
            }),
        })
    }

    fn domain_id(&self) -> &str {
        "test::flaky"
    }
}

/// The `(chain, entry)` identity of a pending report.
pub(super) fn identity(work: &PendingWork) -> (usize, usize) {
    (work.id().chain(), work.id().entry())
}

/// The identities of every pending report, in order.
pub(super) fn identities(bot: &EcsBot) -> Vec<(usize, usize)> {
    bot.pending().iter().map(identity).collect()
}

/// What is holding the first pending report, if anything is pending.
pub(super) fn first_hold(bot: &EcsBot) -> Option<TransitionHold> {
    bot.pending().first().map(|work| work.hold().clone())
}
