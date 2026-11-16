//! The flood family: a child flooding stdout is drained against a slow reader on one worker without exceeding its bound.

use super::*;

/// A child flooding stdout is drained against a slow reader without exceeding its
/// ceiling, on a runtime whose single worker is the whole executor.
///
/// The seeded sweep's own version of `sys_process_binding`'s flood case, kept
/// here so the frame and size families are never observed without it. One worker
/// is the point: the reader and the exit observation share the thread, so the
/// reader's cost decides when the child's next write is serviced.
fn a_seeded_flood_stays_bounded_on_one_worker(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let ceiling = usize::try_from(sim.rng().between(4096, 65_536))?;
        // Large enough that the child cannot fit it in a pipe buffer, which is
        // what makes "the reader drained it while the child wrote" observable.
        let size = sim.rng().between(4 * 1024 * 1024, 6 * 1024 * 1024);
        let runtime = Builder::new()
            .worker_threads(NonZeroUsize::new(1))
            .build()?;
        let outcome = runtime.block_on(async {
            let mut supervisor = Supervisor::new(1);
            supervisor
                .run_process(&captured(&format!("head -c {size} /dev/zero"), ceiling)?)
                .await
                .map_err(Box::<dyn Error>::from)
        })?;

        assert_eq!(
            outcome.stdout().bytes().len(),
            ceiling,
            "a flood against one worker must retain exactly its ceiling"
        );
        assert_eq!(
            outcome.stdout().total_bytes(),
            u64::from(size),
            "every byte the flooding child wrote must be counted"
        );
        assert!(
            outcome.stdout().truncated(),
            "a child writing {size} bytes past a {ceiling}-byte ceiling must report truncation"
        );
        assert!(
            outcome.stdout().retained_capacity() <= ceiling,
            "the retained buffer must not grow past the ceiling"
        );
        record_capture(sim, "flood", &outcome);
        sim.trace.record_number("ceiling", ceiling);
        sim.trace.record_number("size", u64::from(size));
        Ok(())
    })
}

/// A seeded sweep of the one-worker flood over seeds 56..64.
#[test]
fn a_seeded_flood_stays_bounded_on_one_worker_band_00() -> TestResult {
    a_seeded_flood_stays_bounded_on_one_worker(Band::new(56, 8))
}

/// A seeded sweep of the one-worker flood over seeds 64..72.
#[test]
fn a_seeded_flood_stays_bounded_on_one_worker_band_01() -> TestResult {
    a_seeded_flood_stays_bounded_on_one_worker(Band::new(64, 8))
}
