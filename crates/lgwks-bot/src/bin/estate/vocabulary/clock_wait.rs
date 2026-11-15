//! The public wait on a declared clock, `Clock::sleep`, on the shipped binary:
//! the clock half of the bounds and deadlines the words check at.
//!
//! A virtual clock parks a sleep until an advance reaches its deadline, and a
//! wall clock waits real time through the driver. The smoke polls the virtual
//! sleep by hand, the way an executor polls it, so a sleep that wakes early or
//! stays parked past its deadline fails the smoke.

use std::pin::pin;
use std::task::{Context, Waker};
use std::time::Duration;

use lgwks_bot::rt::runtime::Handle;
use lgwks_bot::rt::time::Clock;

use super::super::say;
use super::{Section, VocabularyError, refuse};

/// The virtual sleep the smoke parks.
const PARKED: Duration = Duration::from_millis(5);

/// Move `clock` forward by `by`; a refused advance is logged and fails the
/// section.
fn advance(clock: &Clock, by: Duration) -> Result<(), VocabularyError> {
    match clock.advance(by) {
        Ok(_) => Ok(()),
        Err(error) => {
            lgwks_std::trace::warn!(%error, "vocabulary: the virtual clock refused an advance");
            refuse(
                Section::Bounds,
                "a virtual clock advances by what it is given",
            )
        }
    }
}

/// Prove `Clock::sleep` on both sources.
///
/// # Errors
///
/// The first check that failed, in the bounds section.
pub(super) fn run(driver: &Handle) -> Result<(), VocabularyError> {
    let clock = Clock::virtual_at(Duration::ZERO);
    let mut sleep = pin!(clock.sleep(PARKED));
    let mut context = Context::from_waker(Waker::noop());
    if sleep.as_mut().poll(&mut context).is_ready() {
        return refuse(Section::Bounds, "a virtual sleep parks until an advance");
    }
    advance(&clock, PARKED.saturating_sub(Duration::from_nanos(1)))?;
    if sleep.as_mut().poll(&mut context).is_ready() {
        return refuse(
            Section::Bounds,
            "a virtual sleep stays parked one nanosecond short of its deadline",
        );
    }
    advance(&clock, Duration::from_nanos(1))?;
    if sleep.as_mut().poll(&mut context).is_pending() {
        return refuse(
            Section::Bounds,
            "a virtual sleep ends on the advance that reaches its deadline",
        );
    }
    // The wall half: a sleep of zero on the driver's timer ends at once.
    driver.block_on(Clock::wall().sleep(Duration::ZERO));
    say(&format!(
        "bounds: a virtual sleep of {PARKED:?} ended on the advance that reached it, \
         a wall sleep of zero at once"
    ))?;
    Ok(())
}
