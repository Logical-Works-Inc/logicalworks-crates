//! `lgwks_bot::kernel` names one supported item from each module it promises.
//!
//! Source: #845, a downstream consumer moving its runtime onto this crate asked for a
//! supported path to these eight modules. Compiling is half the proof: a removed item
//! fails this file. The other half is that a kernel path and a support path are one
//! type, because a consumer holding both must not meet two clocks.

use std::time::Duration;

/// The consumer-shaped half of #1016: a flow and an app built through the supported paths only.
#[path = "supported_surface.rs"]
mod supported_surface;

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

/// #1016: the crate root stopped exporting `Cap`, `Host` and `RunStore`, so the
/// kernel carries them. A consumer granting an app a capability and hosting a
/// task names both through here, and each is the support path's one type.
#[cfg(feature = "script")]
#[test]
fn the_kernel_carries_the_task_door_and_the_capabilities_the_root_gave_up() {
    use lgwks_bot::kernel::cap::Cap;
    use lgwks_bot::kernel::task::{Host, Report, RunStore, Task};

    fn support_cap(cap: lgwks_bot::cap::Cap) -> lgwks_bot::cap::Cap {
        cap
    }
    let cap: Cap = support_cap(Cap::net());
    assert_eq!(cap.as_str(), Cap::NET);
    let names = [
        std::any::type_name::<Host>(),
        std::any::type_name::<RunStore>(),
        std::any::type_name::<Report<()>>(),
        std::any::type_name::<Task<()>>(),
    ];
    assert!(
        names
            .iter()
            .all(|name| name.starts_with("lgwks_bot::task::")),
        "a kernel path is the support path's type, never a copy: {names:?}"
    );
}

/// #1016 moved `sealed` under the kernel: the sealed-at-rest key seam is named here.
#[cfg(feature = "sealed")]
#[test]
fn the_kernel_carries_the_sealed_records() {
    let name = std::any::type_name::<dyn lgwks_bot::kernel::sealed::keys::KeyProvider>();
    assert!(
        name.contains("lgwks_bot::sealed::keys::KeyProvider"),
        "{name}"
    );
}
