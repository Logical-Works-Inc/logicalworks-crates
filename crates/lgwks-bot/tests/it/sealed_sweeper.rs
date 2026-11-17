//! The rt sweeper over the real sealed run store (#991).
//!
//! The kernel owns what a sweep is (`SweepTarget`); `rt::sweep::Sweeper` owns
//! when it runs. These cases drive the driver through the run store's own
//! `SweepTarget` impl, on an injected virtual clock: the clock is moved past an
//! expiry, never slept to, and the wait for the sweep is a bounded poll of
//! `read_sealed`, the observable the store already offers. The harness helpers
//! are the ones every sealed family shares (`sealed_harness.rs`).

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::clock::Clock;
use lgwks_bot::effect::RunId;
use lgwks_bot::rt::runtime::block_on;
use lgwks_bot::rt::sweep::Sweeper;
use lgwks_bot::rt::task::yield_now;
use lgwks_bot::rt::time::timeout;
use lgwks_bot::script::StepKey;
use lgwks_bot::sealed::keys::KeyProvider;
use lgwks_bot::task::{RunStore, SealedStepRead};

use crate::scratch::Scratch;
use crate::sealed_harness::{
    FaultingProvider, StepSpec, append_step, erased_step, open_store, require_live,
    test_definition, test_key, test_run,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// How many polls one wait may make.
const POLLS: u64 = 100_000_000;
/// The wall bound on one wait, so a sweeper that never sweeps fails the test
/// instead of hanging it.
const WAIT: Duration = Duration::from_secs(5);

/// Poll `read_sealed` until `key` reads as the tombstone, within [`POLLS`] and
/// [`WAIT`].
async fn wait_erased(store: &RunStore, tenant: &str, run: RunId, key: StepKey) -> bool {
    let polled = timeout(WAIT, async {
        for _ in 0..POLLS {
            if let Ok(SealedStepRead::Erased { .. }) = store.read_sealed(tenant, run, key) {
                return true;
            }
            yield_now().await;
        }
        false
    })
    .await;
    matches!(polled, Ok(true))
}

/// A sealed store in `scratch` with its provider.
fn sealed_store(scratch: &Scratch) -> Result<RunStore, Box<dyn std::error::Error>> {
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(scratch.path())?);
    open_store(scratch.path(), provider)
}

/// One record of `tenant` filed under `path`, expiring at `expires_at`.
fn seal(
    store: &RunStore,
    tenant: &str,
    run: RunId,
    path: &str,
    bytes: &[u8],
    expires_at: Duration,
) -> Result<StepKey, Box<dyn std::error::Error>> {
    let key = test_key(tenant, path)?;
    append_step(
        store,
        StepSpec {
            tenant,
            run,
            key,
            path,
            definition: &test_definition(),
            bytes: bytes.to_vec(),
            expires_at: Some(expires_at),
        },
    )?;
    Ok(key)
}

/// A virtual clock at one second, moved ten seconds on: past a five-second
/// expiry and short of a five-hundred-second one.
fn moved_clock() -> Result<Clock, Box<dyn std::error::Error>> {
    let clock = Clock::virtual_at(Duration::from_secs(1));
    clock.advance(Duration::from_secs(10))?;
    Ok(clock)
}

#[test]
fn the_rt_sweeper_shreds_only_the_due_record() -> TestResult {
    let scratch = Scratch::new("rt-sweeper-due")?;
    let store = sealed_store(&scratch)?;
    let run = test_run(1)?;
    let due = seal(
        &store,
        "acme",
        run,
        "due",
        b"due secret",
        Duration::from_secs(5),
    )?;
    let live = seal(
        &store,
        "acme",
        run,
        "live",
        b"live secret",
        Duration::from_secs(500),
    )?;
    let clock = moved_clock()?;
    let watched = store.clone();
    let erased = block_on(async move {
        // A zero interval is raised to the floor, so the sweeper does not spin.
        let sweeper = Sweeper::spawn(watched.clone(), clock, Duration::ZERO);
        let seen = wait_erased(&watched, "acme", run, due).await;
        sweeper.shutdown().await;
        seen
    });
    assert!(erased, "the due record was never shredded within {WAIT:?}");
    erased_step(&store, "acme", run, due)?;
    require_live(&store, "acme", run, live, b"live secret", "the live record")?;
    Ok(())
}

#[test]
fn the_rt_sweeper_keeps_the_other_tenants_live_record() -> TestResult {
    let scratch = Scratch::new("rt-sweeper-tenants")?;
    let store = sealed_store(&scratch)?;
    let (run_a, run_b) = (test_run(1)?, test_run(2)?);
    let due = seal(
        &store,
        "tenant-a",
        run_a,
        "shared",
        b"a secret",
        Duration::from_secs(5),
    )?;
    let kept = seal(
        &store,
        "tenant-b",
        run_b,
        "shared",
        b"b secret",
        Duration::from_secs(500),
    )?;
    let clock = moved_clock()?;
    let watched = store.clone();
    let erased = block_on(async move {
        let sweeper = Sweeper::spawn(watched.clone(), clock, Duration::from_millis(1));
        let seen = wait_erased(&watched, "tenant-a", run_a, due).await;
        sweeper.shutdown().await;
        seen
    });
    assert!(erased, "tenant-a's due record was never shredded");
    erased_step(&store, "tenant-a", run_a, due)?;
    require_live(
        &store,
        "tenant-b",
        run_b,
        kept,
        b"b secret",
        "tenant-b's record",
    )?;
    Ok(())
}

#[test]
fn the_rt_sweeper_shutdown_ends_the_wait_at_once() -> TestResult {
    let scratch = Scratch::new("rt-sweeper-shutdown")?;
    let store = sealed_store(&scratch)?;
    let run = test_run(1)?;
    let due = seal(
        &store,
        "acme",
        run,
        "due",
        b"due secret",
        Duration::from_secs(5),
    )?;
    let clock = moved_clock()?;
    // The wait is one hour: shutdown returns without it, inside WAIT, and no
    // sweep ran. A shutdown that waited out the interval fails here.
    let joined = block_on(async {
        let sweeper = Sweeper::spawn(store.clone(), clock, Duration::from_secs(3600));
        timeout(WAIT, sweeper.shutdown()).await
    });
    assert!(
        joined.is_ok(),
        "shutdown did not end the one-hour wait within {WAIT:?}"
    );
    require_live(
        &store,
        "acme",
        run,
        due,
        b"due secret",
        "no sweep ran before shutdown",
    )?;
    Ok(())
}
