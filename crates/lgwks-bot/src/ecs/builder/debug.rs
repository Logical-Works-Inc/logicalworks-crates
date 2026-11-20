//! `Debug` for the assembled bot and its two builders.
//!
//! Written as impls rather than derives because `EcsObserveBuilder<S>` and
//! `EcsBuilder` hold `dyn` seams that no derive can cross. Each rendering reports
//! the shape a caller can act on (the name, how many chains or entries are
//! staged, the retry policy) and stops short of the engine's own state through
//! `finish_non_exhaustive`, which marks the omission instead of hiding it.

use super::{EcsBot, EcsBuilder, EcsObserveBuilder};

impl core::fmt::Debug for EcsBot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EcsBot")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for EcsBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EcsBuilder")
            .field("name", &self.name)
            .field("chains", &self.chains.len())
            .field("policy", &self.policy)
            .finish()
    }
}

impl<S> core::fmt::Debug for EcsObserveBuilder<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EcsObserveBuilder")
            .field("name", &self.name)
            .field("prior", &self.prior.len())
            .field("entries", &self.entries.len())
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// The builder's admission checks, tested beside it because `builder.rs` is at
/// the size law; a child module still reaches the private `admitted_parts`.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::super::EcsError;
    use super::super::super::watchdog::MAX_POLL_DEADLINE;
    use super::super::admitted_parts;

    /// The poll deadline is bounded on both sides and the ceiling itself is
    /// admitted: a zero deadline and one past the ceiling are refused, and a
    /// deadline exactly at the ceiling reaches the next check, the effects.
    #[test]
    fn builder_admits_the_poll_ceiling_and_refuses_past_it() {
        assert!(matches!(
            admitted_parts("bot", MAX_POLL_DEADLINE, None),
            Err(EcsError::IncompleteSpec {
                field: "effects",
                ..
            })
        ));
        let past = MAX_POLL_DEADLINE.saturating_add(Duration::from_nanos(1));
        assert!(matches!(
            admitted_parts("bot", past, None),
            Err(EcsError::PollDeadlineExceeded { .. })
        ));
        assert!(matches!(
            admitted_parts("bot", Duration::ZERO, None),
            Err(EcsError::PollDeadlineUnbounded { .. })
        ));
        assert!(matches!(
            admitted_parts("", MAX_POLL_DEADLINE, None),
            Err(EcsError::IncompleteSpec { field: "name", .. })
        ));
    }
}
