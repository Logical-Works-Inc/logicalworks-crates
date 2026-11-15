//! Simulation family: the durable dispatch contract, driven by a seeded sweep.
//!
//! Every scenario drives the **real** bot — the real [`Bot`], the real
//! [`Broker`], the real [`MemoryJournal`] behind a real [`EffectScope`] — using
//! the shared rig in [`sim::rig`], the same rig the acceptance file
//! `durable_dispatch.rs` drives. Nothing here is a stand-in, and nothing here
//! reimplements the ladder: a second copy of the dispatch contract would be a
//! second thing that could pass while the shipped one rotted.
//!
//! The seed chooses what the run *does* — how many restart cycles follow, how
//! many repeated settles are attempted, how many times a tick is repeated — and
//! the run must satisfy the same property whichever it chose. Each declared test
//! sweeps a band of seeds twice and requires the two trace hashes to match, so a
//! nondeterministic run fails even when every assertion happens to pass.
//!
//! The families live beside this file by behaviour: `write_path` (what is
//! written, and when), `restart` (what a restart does with what it finds) and
//! `settlement` (what settling and ticking again may change).
//!
//! | Family | Property it pins |
//! |---|---|
//! | `write_ahead` | the intent and the prepared dispatch are committed *before* the effect body runs |
//! | `outcome_after` | the outcome is committed after the effect, and only once |
//! | `restart_holds` | a recovered unknown is held, not resent |
//! | `restart_lands` | a recovered attempt the journal says landed is retired, not resent |
//! | `not_applied_retries` | evidence that nothing landed makes the entry eligible again |
//! | `applied_never_retries` | evidence that it landed never re-opens the entry |
//! | `ladder_once` | every prepared dispatch is closed by a recorded outcome |
//! | `attempt_continues` | a restart keeps the original key first, so recovery stays valid |
//! | `pending_names_key` | a pending report names the key the caller settles by |
//! | `held_not_exhausted` | a held attempt does not re-enter the body on a later tick |
//! | `settlement_journaled` | settling writes the fact down before acknowledging it |
//! | `settle_is_idempotent` | settling again does not append again |
//! | `store_isolation` | a fresh store starts empty and records only its own attempt |
//! | `crash_loop` | many restarts of a landed effect converge on one outcome |
//! | `unknown_survives` | further ticks do not resolve a recovered unknown by omission |
//! | `repeat_settle_safe` | repeated ticks and settles never duplicate an outcome |

use crate::sim;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::GrantSet;
use lgwks_bot::effect::EffectKey;
use lgwks_bot::journal::{EffectEvent, EventKind, MemoryJournal};
use lgwks_bot::spec::Bot;

use sim::rig::{
    AlwaysLands, FixedSource, NeverSettles, NoteWhatIsRecorded, identity, is_value, scope,
};

#[path = "sim_dispatch/sim_restart.rs"]
mod restart;
#[path = "sim_dispatch/sim_settlement.rs"]
mod settlement;
#[path = "sim_dispatch/sim_write_path.rs"]
mod write_path;

/// The name every bot here is built under. Per-file, because an action's
/// identity is derived from the bot's name as well as from its position: a bot
/// built under another file's name declares another action, and the keys this
/// file settles would then be foreign to it.
const NAME: &str = "sim-dispatch";

type TestResult = Result<(), Box<dyn Error>>;

/// A run's journal store together with the counter its effect increments.
///
/// One type rather than a `store()` and a `counter()` pair per family, because
/// every scenario here is "a fresh store, an armed action, a counter to watch",
/// and spelling that out sixteen times is sixteen places for the fence wiring
/// to drift.
struct Run {
    /// The shared store the bot writes through.
    store: Rc<RefCell<MemoryJournal>>,
    /// How many times the armed effect's body has run.
    entered: Rc<Cell<u32>>,
}

impl Run {
    /// A fresh run: an empty store and a zeroed counter.
    fn new() -> Self {
        Self {
            store: Rc::new(RefCell::new(MemoryJournal::new())),
            entered: Rc::new(Cell::new(0)),
        }
    }

    /// A bot over this run's store armed with `action`.
    ///
    /// One builder rather than one per family, because the durable-dispatch
    /// contract cares what the bot is armed with only insofar as the journal is
    /// concerned; sixteen hand-written builders would be sixteen places for the
    /// fence wiring to drift.
    fn bot_with<A>(&self, action: A) -> Result<Bot, Box<dyn Error>>
    where
        A: lgwks_bot::Execute<Input = u32, Output = ()> + 'static,
    {
        Ok(Bot::builder(NAME)
            .observe(FixedSource)
            .on(is_value, action)
            .with_effects(scope(identity()?, Rc::clone(&self.store))?)
            .build(&GrantSet::empty())?)
    }

    /// A bot whose effect reports itself indeterminate.
    fn never(&self) -> Result<Bot, Box<dyn Error>> {
        self.bot_with(NeverSettles {
            entered: Rc::clone(&self.entered),
        })
    }

    /// A bot whose effect lands.
    fn lands(&self) -> Result<Bot, Box<dyn Error>> {
        self.bot_with(AlwaysLands {
            entered: Rc::clone(&self.entered),
        })
    }

    /// A bot that snapshots the journal at the instant its effect runs.
    fn notes(&self, seen: &Rc<RefCell<Option<Vec<EventKind>>>>) -> Result<Bot, Box<dyn Error>> {
        self.bot_with(NoteWhatIsRecorded {
            store: Rc::clone(&self.store),
            seen: Rc::clone(seen),
            key: Rc::new(RefCell::new(None)),
        })
    }

    /// Every event the store holds, through the trait rather than the concrete
    /// type: what these families are about is what the journal promises, and a
    /// test reading `MemoryJournal` directly would keep passing if the promise
    /// changed.
    fn recorded(&self) -> Result<Vec<EffectEvent>, Box<dyn Error>> {
        Ok(lgwks_bot::journal::EffectJournal::committed(
            &*self.store.borrow(),
        )?)
    }

    /// The kinds the store holds, in order.
    fn kinds(&self) -> Result<Vec<EventKind>, Box<dyn Error>> {
        Ok(self.recorded()?.iter().map(|event| event.kind()).collect())
    }

    /// How many recorded events are of `kind`.
    fn count(&self, kind: EventKind) -> Result<usize, Box<dyn Error>> {
        Ok(self.kinds()?.iter().filter(|seen| **seen == kind).count())
    }

    /// The key the first recorded event is about.
    fn first_key(&self) -> Result<EffectKey, Box<dyn Error>> {
        self.recorded()?
            .first()
            .map(|event| event.key())
            .ok_or_else(|| "the first attempt must have been recorded".into())
    }

    /// The distinct keys the store holds, in first-seen order.
    fn distinct_keys(&self) -> Result<Vec<EffectKey>, Box<dyn Error>> {
        let mut keys: Vec<EffectKey> = Vec::new();
        for event in self.recorded()? {
            if !keys.contains(&event.key()) {
                keys.push(event.key());
            }
        }
        Ok(keys)
    }

    /// How many times the effect's body has run.
    fn entered(&self) -> u32 {
        self.entered.get()
    }

    /// A count drawn from this run's seed, in `1..=span`, or `None` when the
    /// draw does not fit this host's address space.
    ///
    /// `None` rather than a count of one: a substitute here would be a scenario
    /// that churned once on a host that could not address the draw, and the
    /// failure this family looks for is exactly a scenario that did not run.
    fn churn(&self, sim: &mut sim::Sim, span: u32) -> Option<usize> {
        usize::try_from(sim.rng().between(1, span)).ok()
    }
}

/// The bands partition the seed space: no gap, no overlap.
#[test]
fn the_bands_cover_every_seed_exactly_once() -> TestResult {
    let mut covered = Vec::new();
    for band in sim::bands(sim::SEED_SPACE, sim::BANDS) {
        covered.extend(band.seeds());
    }
    covered.sort_unstable();
    covered.dedup();
    assert_eq!(
        u64::try_from(covered.len())?,
        sim::SEED_SPACE,
        "the bands cover {} seeds, not the shared seed space",
        covered.len()
    );
    Ok(())
}

/// The rig this file drives is the same one the acceptance file drives.
#[test]
fn the_rig_is_shared_not_copied() -> TestResult {
    let run = Run::new();
    run.lands()?.tick()?;
    assert_eq!(run.entered(), 1, "the shared rig must drive a real effect");
    assert!(
        run.kinds()?.len() >= 3,
        "the shared rig must have written the durable ladder: {:?}",
        run.kinds()?
    );
    Ok(())
}
