//! Seeded telemetry campaigns for audit fixes 3, 5, 6, 7, and 8: a refused
//! start or release is audited, racing starts subscribe once, an append that
//! fails is kept as a typed error, and the ring is bounded.
//!
//! Every run is replayed: the same seed gives the same trace hash and a
//! distinct seed diverges. Thread schedules are never part of the trace: the
//! threaded campaigns judge invariants that hold under every interleaving.

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{AuditAction, BrokerError, Subject};
use drishti::native_host::governance::GovernanceStore;
use drishti::native_host::protocol::JsonValue;
use drishti::native_host::telemetry::{MAX_RING_CAPACITY, TelemetryError, TelemetryTap};

use crate::broker_leaks::{one_denial, since};
use crate::broker_scope::replays;
use crate::broker_shared::{Harness, SESSION_SHARED, register};
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// A tap over a fresh store with `granted` capabilities and a seeded ring.
fn tap_over(
    random: &mut SimRng,
    granted: Vec<Capability>,
) -> Result<(TelemetryTap, GovernanceStore), String> {
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let ring = random.below_len(20).saturating_add(1);
    let tap =
        TelemetryTap::new(store.clone(), granted, ring).map_err(|error| format!("{error}"))?;
    Ok((tap, store))
}

/// The owner unregisters, which cancels its tap; the later `stop` is refused
/// by the broker, audited, and kept as the tap's last error.
fn run_release(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let (tap, _store) = tap_over(&mut random, vec![Capability::SignalSubscribe])?;
    let owner = register(&harness, "widget.tap-owner", "tenant-a", SESSION_SHARED)?;
    tap.start(harness.broker(), &owner)
        .map_err(|error| format!("{error}"))?;
    harness.broker().unregister(&owner);
    ensure(tap.last_error().is_none(), || {
        String::from("a healthy tap has no last error")
    })?;
    let before = harness.audit_events().len();
    tap.stop(harness.broker());
    let refused = Some(TelemetryError::UntapRefused(BrokerError::InvalidHandle));
    ensure(tap.last_error() == refused, || {
        String::from("a refused release must be kept as the last error")
    })?;
    let release = AuditAction::Revoke(Subject::Tap);
    one_denial(&harness, before, &release, &BrokerError::InvalidHandle)?;
    let settled = harness.audit_events().len();
    tap.stop(harness.broker());
    ensure(harness.audit_events().len() == settled, || {
        String::from("a second stop must do nothing")
    })?;
    let heir = register(&harness, "widget.tap-heir", "tenant-a", SESSION_SHARED)?;
    tap.start(harness.broker(), &heir)
        .map_err(|error| format!("{error}"))?;
    let emits = random.below_len(10).saturating_add(1);
    for _ in 0..emits {
        harness
            .broker()
            .emit(&heir, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    ensure(
        usize::try_from(tap.metrics().emits_observed()) == Ok(emits),
        || String::from("a restarted tap must observe every emit"),
    )?;
    trace.mix_len(emits);
    Ok(trace.finish())
}

/// Wrong-order. Bug caught: `stop` after the owner left discards the broker's
/// refusal (`drop(broker.untap(..))`), so the failed release leaves no trace.
#[test]
fn sim_telemetry_wrong_order_refused_release_is_audited_and_kept() -> Result<(), String> {
    replays(run_release, [8, 44, 515], 9)
}

/// A tap without the capability is refused on every start: audited each time
/// as a denied tap registration, never running, never recording.
fn run_denied_starts(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let (tap, store) = tap_over(&mut random, Vec::new())?;
    let owner = register(&harness, "widget.tap-owner", "tenant-a", SESSION_SHARED)?;
    let attempts = random.below_len(8).saturating_add(2);
    for _ in 0..attempts {
        let before = harness.audit_events().len();
        let refusal = tap
            .start(harness.broker(), &owner)
            .err()
            .ok_or_else(|| String::from("a tap without the capability must not start"))?;
        ensure(matches!(refusal, BrokerError::CapabilityDenied(_)), || {
            format!("wrong refusal: {refusal:?}")
        })?;
        let action = AuditAction::Register(Subject::Tap);
        let event = one_denial(&harness, before, &action, &refusal)?;
        ensure(event.caller().scope().tenant_id() == "tenant-a", || {
            String::from("the denial must name the owner's tenant")
        })?;
    }
    let settled = harness.audit_events().len();
    harness
        .broker()
        .emit(&owner, "tick", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    let routed = since(&harness, settled).len();
    let kept = store
        .count("tenant-a", SESSION_SHARED)
        .map_err(|error| format!("{error}"))?;
    let idle = tap.metrics().emits_observed() == 0 && kept == 0 && routed == 1;
    ensure(idle, || {
        String::from("a refused tap must not run: it saw an emit")
    })?;
    trace.mix_len(attempts);
    Ok(trace.finish())
}

/// Saboteur. Bug caught: a tap start refused for a missing capability that
/// never reaches the audit sink (the check ran before the broker was asked).
#[test]
fn sim_telemetry_saboteur_denied_start_is_audited_every_time() -> Result<(), String> {
    replays(run_denied_starts, [13, 67, 905], 14)
}

/// Starters race while emitters emit and a reader reads metrics. Whatever the
/// schedule: one subscription (each emit observed once), every record stored,
/// no append error, no deadlock.
fn run_racing_starts(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let (tap, store) = tap_over(&mut random, vec![Capability::SignalSubscribe])?;
    let owner = register(&harness, "widget.tap-owner", "tenant-a", SESSION_SHARED)?;
    tap.start(harness.broker(), &owner)
        .map_err(|error| format!("{error}"))?;
    tap.stop(harness.broker());
    let starters = random.below_len(10).saturating_add(2);
    let per_emitter = random.below_len(25).saturating_add(5);
    let sent = per_emitter.saturating_mul(3);
    let outcomes: Vec<Result<(), String>> = std::thread::scope(|scope| {
        let mut joined = Vec::new();
        let starts: Vec<_> = (0..starters)
            .map(|_| scope.spawn(|| tap.start(harness.broker(), &owner)))
            .collect();
        for start in starts {
            joined.push(match start.join() {
                Ok(done) => done.map_err(|error| format!("{error}")),
                Err(_) => Err(String::from("a starter panicked")),
            });
        }
        let reads = scope.spawn(|| {
            let mut peak = 0_u64;
            for _ in 0..per_emitter {
                peak = peak.max(tap.metrics().emits_observed());
                ensure(tap.snapshot(None).len() <= sent, || {
                    String::from("the ring held more than was sent")
                })?;
            }
            let seen = usize::try_from(peak).map_err(|error| format!("{error}"))?;
            ensure(seen <= sent, || {
                String::from("the reader saw more than was sent")
            })
        });
        let emits: Vec<_> = (0..3)
            .map(|_| {
                scope.spawn(|| {
                    for _ in 0..per_emitter {
                        // A refused emit is not this campaign's concern.
                        if harness
                            .broker()
                            .emit(&owner, "tick", JsonValue::Null)
                            .is_err()
                        {
                            return false;
                        }
                    }
                    true
                })
            })
            .collect();
        for emit in emits {
            joined.push(match emit.join() {
                Ok(true) => Ok(()),
                Ok(false) => Err(String::from("an emit was refused")),
                Err(_) => Err(String::from("an emitter panicked")),
            });
        }
        joined.push(match reads.join() {
            Ok(done) => done,
            Err(_) => Err(String::from("the reader panicked")),
        });
        joined
    });
    outcomes.into_iter().try_for_each(|outcome| outcome)?;
    let metrics = tap.metrics();
    let kept = store
        .count("tenant-a", SESSION_SHARED)
        .map_err(|error| format!("{error}"))?;
    let settled = usize::try_from(metrics.emits_observed()) == Ok(sent)
        && usize::try_from(kept) == Ok(sent)
        && metrics.append_errors() == 0;
    ensure(settled, || {
        format!(
            "sent {sent}: observed {}, kept {kept}",
            metrics.emits_observed()
        )
    })?;
    trace.mix_len(sent);
    Ok(trace.finish())
}

/// Double-clicker. Bug caught: `start` checks `running` and sets it under
/// separate locks, so two racing starts both subscribe (every emit is seen
/// twice and its second append fails); and an append that holds the state
/// lock blocks metrics readers.
#[test]
fn sim_telemetry_double_clicker_racing_starts_subscribe_once() -> Result<(), String> {
    replays(run_racing_starts, [3, 31, 257], 4)
}

/// Two taps over one store hear every emit; the second append of each signal
/// is a duplicate, counted and kept as a typed `AppendFailed`.
fn run_append_failures(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let granted = vec![Capability::SignalSubscribe];
    let built = |caps: Vec<Capability>| {
        TelemetryTap::new(store.clone(), caps, 4).map_err(|error| format!("{error}"))
    };
    let first = built(granted.clone())?;
    let second = built(granted)?;
    let one = register(&harness, "widget.tap-one", "tenant-a", SESSION_SHARED)?;
    let two = register(&harness, "widget.tap-two", "tenant-a", SESSION_SHARED)?;
    first
        .start(harness.broker(), &one)
        .map_err(|error| format!("{error}"))?;
    second
        .start(harness.broker(), &two)
        .map_err(|error| format!("{error}"))?;
    let emits = random.below_len(30).saturating_add(1);
    for _ in 0..emits {
        harness
            .broker()
            .emit(&one, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    let errors = [
        first.metrics().append_errors(),
        second.metrics().append_errors(),
    ];
    let wanted = u64::try_from(emits).map_err(|error| format!("{error}"))?;
    ensure(errors == [0, wanted] || errors == [wanted, 0], || {
        format!("each duplicate append must count once: {errors:?}")
    })?;
    let failing = if errors[0] == 0 { &second } else { &first };
    let healthy = if errors[0] == 0 { &first } else { &second };
    let kept = failing.last_error();
    let duplicate =
        matches!(kept, Some(TelemetryError::AppendFailed(ref text)) if text.contains("duplicate"));
    ensure(duplicate, || {
        format!("the cause must be kept as AppendFailed, got {kept:?}")
    })?;
    ensure(healthy.last_error().is_none(), || {
        String::from("the healthy tap has no error to keep")
    })?;
    let stored = store
        .count("tenant-a", SESSION_SHARED)
        .map_err(|error| format!("{error}"))?;
    ensure(stored == wanted, || {
        String::from("each signal must be stored once")
    })?;
    trace.mix_len(emits);
    Ok(trace.finish())
}

/// Liar-upstream. Bug caught: `TelemetryError::AppendFailed` with no
/// producer, so a failing store is a bare counter with no cause.
#[test]
fn sim_telemetry_liar_store_failure_is_kept_as_a_typed_cause() -> Result<(), String> {
    replays(run_append_failures, [6, 52, 701], 7)
}

/// Ring capacities around the bound: inside builds, past it is refused.
fn run_ring_bounds(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let mut sizes = vec![
        0,
        1,
        MAX_RING_CAPACITY,
        MAX_RING_CAPACITY.saturating_add(1),
        usize::MAX,
    ];
    for _ in 0..40 {
        let jitter = random.below_len(9);
        sizes.push(MAX_RING_CAPACITY.saturating_add(jitter).saturating_sub(4));
    }
    for size in sizes {
        let built = TelemetryTap::new(store.clone(), Vec::new(), size).map(|_| ());
        let want = match size {
            0 => Err(TelemetryError::InvalidCapacity(0)),
            1..=MAX_RING_CAPACITY => Ok(()),
            past => Err(TelemetryError::CapacityTooLarge(past)),
        };
        ensure(built == want, || {
            format!("ring {size}: got {built:?}, want {want:?}")
        })?;
        trace.mix_len(size);
    }
    Ok(trace.finish())
}

/// Giant-and-empty. Bug caught: a ring capacity with no ceiling, so one tap
/// can reserve unbounded memory (the doc promised 5,000).
#[test]
fn sim_telemetry_giant_ring_capacity_is_refused_past_the_bound() -> Result<(), String> {
    replays(run_ring_bounds, [12, 90, 1_001], 13)
}
