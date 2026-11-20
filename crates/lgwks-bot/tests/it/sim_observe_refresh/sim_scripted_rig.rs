//! The scripted source and the bots built over it, shared by the stall and
//! scripted-wave families.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::spec::Bot;
use lgwks_bot::verb::{Observe, RefreshReason};

use super::{ChainHandles, Tenant, domain_of, logs, seeded_refusal, tenant_scope};

/// The poll deadline every stall and scripted family builds its bot under.
///
/// A wedged source costs exactly this budget, in real time, on every tick it is
/// wedged: the poll deadline is a real-time watchdog by design (see
/// `bounded_wave`), so a realistic 30-second default would turn a band into
/// minutes of waiting. What the families observe does not depend on the number —
/// every assertion is about *which* chain was reported and whether the others
/// still committed — and the deadline is declared rather than faked, so the same
/// machinery under test is the one `Bot::tick` runs in production.
///
/// One millisecond rather than more because nothing here races it: a source
/// that is not wedged answers on its first turn, and a first turn is always
/// polled however early the wave's reaper fires. A source that yields once is
/// read on the turn after its `Pending` whenever that turn lands, because an
/// expired wave turns every poll once more before cutting it off; only a
/// yielder's *first* turn can fall behind a deadline an earlier `Pending` chain
/// armed, which is why the replay family arms a yield only ahead of every
/// other `Pending` chain (`yield_rule`). At 40 ms the stall families spent ~12 s
/// a band waiting.
pub(super) const SIM_DEADLINE: Duration = Duration::from_millis(1);

/// The cell value that means "never resolves".
///
/// A named constant rather than a `bool` next to the value cell so the two
/// channels cannot be confused by a call site that sets the wrong one: the value
/// cell is a `u32` because the source's output is one, and a `bool` there would
/// compile and mean nothing.
pub(super) const WEDGED_PACE: u32 = u32::MAX;

/// The width of one observation wave, as this file reads it.
///
/// A copy of the substrate's own fan-out rather than a re-derivation, exactly
/// as the corresponding non-simulation fixture does: it is the number that
/// decides how many chains share one deadline watchdog, and a family that
/// derived it would follow a change to the fan-out rather than noticing one.
pub(super) const WAVE_WIDTH: usize = 32;

/// A source whose entire answer is the scenario's: it can answer, yield once,
/// park forever, refuse, and declare its own cached baseline unsound, all from
/// the cells one [`ChainHandles`] carries.
///
/// One source type rather than the two the bands use, because a builder's type
/// follows its source: a family that varies the chain count at runtime needs
/// every chain to be the same concrete type, and a source that can express
/// every shape these families need is what makes the count a draw instead of a
/// constant.
pub(super) struct Scripted {
    /// The value this poll reports once it answers.
    value: Rc<Cell<u32>>,
    /// What `cache_state` answers.
    declared: Rc<Cell<Option<RefreshReason>>>,
    /// Whether this poll refuses instead of reading.
    refusing: Rc<Cell<bool>>,
    /// [`WEDGED_PACE`] parks the poll forever; anything else lets it answer.
    pace: Rc<Cell<u32>>,
    /// How many polls still return `Pending` before answering.
    pending: Rc<Cell<u32>>,
    /// The chain's index, for the domain identity a report names it by.
    chain: usize,
    /// The tenant this source belongs to, for the same reason.
    tenant: u32,
}

impl Scripted {
    /// The poll a source that is not refusing makes: wedged without resolving,
    /// pending for its paced polls, then ready with its value.
    async fn read(&self) -> Result<u32, EcsError> {
        if self.pace.get() == WEDGED_PACE {
            // Parks without ever resolving, which is the shape the per-poll
            // deadline exists to bound. A yield is a *different* shape and is
            // handled below, so a fixture cannot confuse "slow" with "stopped".
            std::future::pending::<()>().await;
        }
        let pending = Rc::clone(&self.pending);
        let value = Rc::clone(&self.value);
        std::future::poll_fn(move |context: &mut Context<'_>| {
            if pending.get() > 0 {
                pending.set(pending.get().saturating_sub(1));
                // Wakes itself, so the wave re-polls without waiting out the
                // budget: the poll made no progress, but it did not stop.
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(Ok(value.get()))
            }
        })
        .await
    }
}

impl Observe for Scripted {
    type Output = u32;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u32, EcsError> {
        call.0.check(Observe::required_caps(self))?;
        if self.refusing.get() {
            Err(seeded_refusal(self.tenant, self.chain))
        } else {
            self.read().await
        }
    }

    fn cache_state(&self) -> Option<RefreshReason> {
        self.declared.get()
    }

    fn domain_id(&self) -> &str {
        domain_of(self.tenant, self.chain)
    }
}

/// Build one scripted source over `held`, as chain `chain` of `tenant`.
pub(super) fn scripted(held: &ChainHandles, chain: usize, tenant: u32) -> Scripted {
    Scripted {
        value: Rc::clone(&held.value),
        declared: Rc::clone(&held.declared),
        refusing: Rc::clone(&held.refusing),
        pace: Rc::clone(&held.pace),
        pending: Rc::clone(&held.pending),
        chain,
        tenant,
    }
}

/// Arm one chain for one tick: its value, its pace, its declaration and how many
/// polls it yields for.
///
/// One helper rather than a field write per call site, because every family here
/// arms the same four cells and a site that forgot one would arm a chain the
/// previous tick left wedged.
pub(super) fn arm_chain(
    held: &ChainHandles,
    value: u32,
    pace: u32,
    declared: Option<RefreshReason>,
    pending: u32,
) {
    held.value.set(value);
    held.pace.set(pace);
    held.declared.set(declared);
    held.pending.set(pending);
    held.refusing.set(false);
}

/// Build one tenant's bot over `width` scripted chains under `deadline`.
///
/// The chain count is a parameter rather than a constant because every chain is
/// the same concrete [`Scripted`] type, which is what lets the builder be
/// extended in a loop: the erasure boundary the band families run into only
/// binds when the source types differ.
pub(super) fn scripted_bot(
    name: &str,
    tenant: u32,
    width: usize,
    deadline: Duration,
    holds: &Rc<Cell<bool>>,
) -> Result<Tenant, Box<dyn Error>> {
    assert!(width >= 1, "a scripted bot needs at least one chain");
    let ran = Rc::new(RefCell::new(Vec::new()));
    let first = ChainHandles::new();
    let mut builder = Bot::builder(name)
        .with_poll_deadline(deadline)
        .observe(scripted(&first, 0, tenant))
        .on(|observed: &u32| *observed > 0, logs(&ran, 0, holds));
    let mut handles = vec![first];
    for chain in 1..width {
        let held = ChainHandles::new();
        builder = builder
            .observe(scripted(&held, chain, tenant))
            .on(|observed: &u32| *observed > 0, logs(&ran, chain, holds));
        handles.push(held);
    }
    let bot = builder
        .with_effects(tenant_scope(tenant)?)
        .build(&GrantSet::empty())?;
    Ok(Tenant {
        bot,
        handles,
        ran,
        tenant,
    })
}

/// Build one tenant's bot over `width` scripted chains and run one clean
/// baseline tick, so every chain has a committed value before it is armed.
///
/// Shared by the two families that need a pre-stall baseline, because "arm
/// every chain to a distinct value and tick once" is the same three-step setup
/// in both and a copy would drift the moment one of them changed what it
/// considered a baseline.
pub(super) fn baseline_bot(
    name: &str,
    width: usize,
    deadline: Duration,
    holds: &Rc<Cell<bool>>,
) -> Result<Tenant, Box<dyn Error>> {
    let mut subject = scripted_bot(name, 0, width, deadline, holds)?;
    for (index, held) in subject.handles.iter().enumerate() {
        let value = u32::try_from(index)?.saturating_add(1);
        arm_chain(held, value, 0, None, 0);
    }
    let _baseline = subject.bot.tick();
    Ok(subject)
}
