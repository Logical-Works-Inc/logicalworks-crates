//! A running bot spends the fence: the broker's bound holds on the real dispatch path.
//!
//! Source: the Gap 2 issue's "a mutating reach spends one". A count nothing on the
//! run path calls would pass every broker test and limit nothing, so this drives a
//! bot through its public tick and counts how many times the action was entered.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lgwks_bot::GrantSet;
use lgwks_bot::broker::Broker;
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::spec::{Bot, EffectScope};

use crate::sim::rig::{AlwaysLands, Requests, identity};

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A bot, what its source reports next, and how many times its action ran.
struct Rig {
    /// The bot under test.
    bot: Bot,
    /// What its source reports next.
    source: Rc<RefCell<u32>>,
    /// How many times its action ran.
    entered: Rc<Cell<u32>>,
}

/// A bot whose broker allows `bound` reaches.
fn bounded(bound: u32) -> Outcome<Rig> {
    let identity = identity()?;
    let mut broker = Broker::new();
    broker.register(identity.environment())?;
    broker.set_offline_bound(identity.environment(), bound)?;
    let source = Rc::new(RefCell::new(1));
    let entered = Rc::new(Cell::new(0));
    let bot = Bot::builder("broker-fence-dispatch")
        .observe(Requests(Rc::clone(&source)))
        .on(
            |_seen: &u32| true,
            AlwaysLands {
                entered: Rc::clone(&entered),
            },
        )
        .with_effects(EffectScope::new(
            identity,
            broker,
            Box::new(MemoryJournal::new()),
        ))
        .build(&GrantSet::empty())?;
    Ok(Rig {
        bot,
        source,
        entered,
    })
}

impl Rig {
    /// Report `value` and tick once, returning any refusal.
    fn next_work(&mut self, value: u32) -> Option<String> {
        *self.source.borrow_mut() = value;
        self.bot.tick().err().map(|refusal| format!("{refusal:?}"))
    }
}

/// Catches `Broker::spend` left off the dispatch path: the second attempt would land.
#[test]
fn a_running_bot_stops_reaching_when_its_offline_bound_is_spent() -> Outcome {
    let mut rig = bounded(1)?;
    assert_eq!(rig.next_work(1), None);
    assert_eq!(rig.entered.get(), 1, "the one allowed reach landed");
    let refusal = rig.next_work(2).ok_or("the second reach was allowed")?;
    assert!(refusal.contains("OfflineExposureExhausted"), "{refusal}");
    assert_eq!(rig.entered.get(), 1, "the refused attempt never ran");
    Ok(())
}

/// Catches a kill switch that only the broker's own tests can see.
#[test]
fn a_zero_bound_stops_a_running_bot_before_its_first_reach() -> Outcome {
    let mut rig = bounded(0)?;
    let refusal = rig
        .next_work(1)
        .ok_or("the kill switch let a reach through")?;
    assert!(refusal.contains("OfflineExposureExhausted"), "{refusal}");
    assert_eq!(rig.entered.get(), 0);
    Ok(())
}
