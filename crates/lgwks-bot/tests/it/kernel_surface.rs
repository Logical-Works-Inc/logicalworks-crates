//! `lgwks_bot::kernel` names one supported item from each module it promises.
//!
//! Source: #845, a downstream consumer moving its runtime onto this crate asked for a
//! supported path to these eight modules. Compiling is half the proof: a removed item
//! fails this file. The other half is that a kernel path and a support path are one
//! type, because a consumer holding both must not meet two clocks.

use std::time::Duration;

use lgwks_bot::kernel::broker::Broker;
use lgwks_bot::kernel::clock::{Clock, ClockError, EpochClock, RecordedInstant};
use lgwks_bot::kernel::dist_lease::HostId;
use lgwks_bot::kernel::effect::EffectKey;
use lgwks_bot::kernel::gate::GrantSet;
use lgwks_bot::kernel::journal::{Durable, JournalPosition, MemoryJournal};
use lgwks_bot::kernel::retry::{RetryDecision, RetryFacts};
use lgwks_bot::kernel::skew::SkewBound;

/// Takes the support path's clock, so a kernel clock passing here is the same type.
fn support_reading(clock: &lgwks_bot::clock::Clock) -> Duration {
    clock.now()
}

#[test]
fn a_clock_named_through_the_kernel_is_the_support_paths_clock() {
    let clock = Clock::virtual_at(Duration::from_secs(3));
    assert_eq!(support_reading(&clock), Duration::from_secs(3));
}

#[test]
fn the_skew_bound_named_through_the_kernel_carries_its_allowance() {
    let bound = SkewBound::new(Duration::from_millis(5));
    assert_eq!(bound.get(), Duration::from_millis(5));
}

#[test]
fn a_recorded_instant_is_durable_through_the_kernel() -> Result<(), lgwks_std::wire::WireError> {
    let instant = RecordedInstant::from_unix_nanos(7);
    assert_eq!(
        RecordedInstant::from_record(&instant.to_record()?)?,
        instant
    );
    Ok(())
}

#[test]
fn a_live_epoch_clock_refuses_a_callers_advance() -> Result<(), ClockError> {
    let clock = EpochClock::system()?;
    assert_eq!(
        clock.advance(Duration::from_secs(1)),
        Err(ClockError::NotVirtual)
    );
    Ok(())
}

/// Every module the kernel promises is held by a name here, so removing an item from
/// the surface stops this file compiling.
#[test]
fn every_promised_module_has_a_named_type() {
    let names = [
        std::any::type_name::<Broker>(),
        std::any::type_name::<HostId>(),
        std::any::type_name::<EffectKey>(),
        std::any::type_name::<GrantSet>(),
        std::any::type_name::<JournalPosition>(),
        std::any::type_name::<MemoryJournal>(),
        std::any::type_name::<RetryFacts>(),
        std::any::type_name::<RetryDecision>(),
    ];
    assert!(names.iter().all(|name| name.starts_with("lgwks_bot::")));
}
