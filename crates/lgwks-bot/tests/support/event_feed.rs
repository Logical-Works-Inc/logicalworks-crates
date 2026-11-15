//! An event source and the action that records what it delivered.
//!
//! T09's two families (the walked rows in `observe_refresh` and the seeded ones
//! in `sim_observe_refresh`) both need a source that hands out queued
//! [`EventId`]s one per poll, and an action that logs the ids it ran with. One
//! definition, because the identity claim they test is "the id decides, never
//! the payload", and two feeds that popped or logged differently could each pass
//! it for a different reason.
//!
//! Declared once in `tests/it/main.rs`; a module `use crate::event_feed::...`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lgwks_bot::effect::EventId;
use lgwks_bot::error::KernelError;
use lgwks_bot::{Auth, Cap, DispatchCertainty, EcsError, EffectLifetime, Execute, Observe};

use crate::poll::admit_poll;

/// An event source: the next [`EventId`] the test queued, from the back of the
/// queue, so a queue built in reverse delivers in draw order.
pub(crate) struct EventFeed {
    /// The events still to be delivered.
    pub(crate) queue: Rc<RefCell<Vec<EventId<u32>>>>,
    /// How many times the body ran.
    pub(crate) polls: Rc<Cell<u32>>,
    /// The domain identity a report names this source by.
    pub(crate) domain: &'static str,
}

impl Observe for EventFeed {
    type Output = EventId<u32>;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<EventId<u32>, EcsError> {
        admit_poll(&call.0, Observe::required_caps(self), &self.polls)?;
        self.queue
            .borrow_mut()
            .pop()
            .ok_or(EcsError::Kernel(KernelError::DomainError {
                domain: self.domain.to_owned(),
                certainty: DispatchCertainty::NotDelivered,
                cause: "the event feed is drained".to_owned(),
            }))
    }

    fn domain_id(&self) -> &str {
        self.domain
    }
}

/// An action that lands over an event, recording the event id it ran with.
///
/// A separate type from a value-landing action rather than a generic one:
/// `Execute` is the crate's public verb and a generic action would make every
/// call site carry a type parameter for the difference between "a value moved"
/// and "an event arrived", which is the difference T09 is about.
pub(crate) struct LandsEvents {
    /// Every event id this action ran with, in the order it ran.
    pub(crate) seen: Rc<RefCell<Vec<u64>>>,
    /// The domain identity the action reports itself under.
    pub(crate) domain: &'static str,
}

impl Execute for LandsEvents {
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
        self.seen.borrow_mut().push(call.1.id());
        Ok(())
    }

    fn domain_id(&self) -> &str {
        self.domain
    }
}
