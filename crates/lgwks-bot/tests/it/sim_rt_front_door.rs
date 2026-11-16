//! Simulation family: the async tier's documented entry points under a seeded sweep.
//!
//! `rt_front_door` pins one shape per door: four in flight, sixteen inputs, one token. This target
//! asks the same questions of *sizes*: every ceiling against every burst, every fan-out width over
//! every input count, every number of tasks a token has to stop, and every capture ceiling against
//! every amount of output.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `a_burst_never_exceeds_the_supervisor_ceiling_and_every_task_is_reported` | a supervisor admits `min(burst, ceiling)`, refuses the rest by name, and reports each admitted task |
//! | `a_fan_out_returns_every_input_in_input_order_whatever_the_limit` | `join_all_bounded` returns one result per input, in input order, at any limit and any completion order |
//! | `a_cancelled_token_stops_every_task_a_join_set_owns_at_any_size` | one cancel drains every task of a set, however many it owns |
//! | `a_command_captures_exactly_the_smaller_of_its_output_and_the_ceiling` | a capture keeps the first `ceiling` bytes of what the child wrote, and never more |
//! | `a_hostile_input_anywhere_in_a_fan_out_comes_back_as_its_own_typed_panic` | a panicking input is `FanOutError::Panicked` with its own payload, never a short vector |
//! | `a_hostile_tenants_panic_never_reaches_the_other_tenants_fan_out` | one tenant's panicking input leaves a concurrent tenant's fan-out whole |
//!
//! # What is real and what is seeded
//!
//! The runtime, the supervisor, the join set, the token and the child process are the shipped ones.
//! The seed decides the sizes and the order in which inputs finish; no assertion reads a clock, so a
//! replay that differs is a defect in the code under test.
use std::error::Error;

use lgwks_bot::rt::runtime::Runtime;
use lgwks_bot::rt::supervise::{Supervisor, TrySpawnRefusal};
use lgwks_bot::rt::sync::CancellationToken;
use lgwks_bot::rt::task::{FanOutError, JoinError, JoinSet, join_all_bounded};
use lgwks_bot::rt::time::{Clock, Duration};

use crate::sim;

use sim::band_of;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// A supervisor admits `min(burst, ceiling)`, refuses the rest by name, and reports each admitted task.
#[test]
fn a_burst_never_exceeds_the_supervisor_ceiling_and_every_task_is_reported() -> TestResult {
    let runtime = Runtime::new()?;
    sim::assert_replays(band_of(10), |sim| {
        let ceiling = usize::try_from(sim.rng().between(1, 6))?;
        let burst = usize::try_from(sim.rng().between(1, 12))?;
        runtime.block_on(async {
            let mut supervisor = Supervisor::new(ceiling);
            let mut admitted = 0_usize;
            let mut refused = 0_usize;
            for _ in 0..burst {
                // A body that waits for its token keeps its slot, so the ceiling is genuinely full.
                let placed = supervisor.try_spawn(|token| async move {
                    token.cancelled_owned().await;
                });
                match placed {
                    Ok(()) => admitted = admitted.saturating_add(1),
                    Err(TrySpawnRefusal::AtCapacity) => refused = refused.saturating_add(1),
                    Err(other) => return Err(format!("unexpected refusal {other:?}").into()),
                }
            }
            assert_eq!(
                admitted,
                burst.min(ceiling),
                "the ceiling {ceiling} bounds the burst {burst}"
            );
            assert_eq!(
                refused,
                burst.saturating_sub(admitted),
                "the rest are refused by name"
            );
            let counted = supervisor.stats();
            assert_eq!(
                usize::try_from(counted.refused)?,
                refused,
                "each refusal is counted"
            );
            let report = supervisor.shutdown().await;
            assert_eq!(
                usize::try_from(report.stats().spawned)?,
                admitted,
                "only admitted tasks spawn"
            );
            assert_eq!(
                report.outcomes().len(),
                admitted,
                "each admitted task is reported once"
            );
            Ok::<(), Box<dyn Error>>(())
        })?;
        sim.record(&format!("ceiling={ceiling} burst={burst}"));
        Ok(())
    })
}

/// `join_all_bounded` returns one result per input, in input order, at any limit and any completion order.
#[test]
fn a_fan_out_returns_every_input_in_input_order_whatever_the_limit() -> TestResult {
    let runtime = Runtime::new()?;
    sim::assert_replays(band_of(11), |sim| {
        let limit = usize::try_from(sim.rng().between(1, 8))?;
        let inputs = sim.rng().between(0, 24);
        // A drawn delay per input scrambles the order in which they finish.
        let delays: Vec<u64> = (0..inputs).map(|_| u64::from(sim.rng().below(3))).collect();
        let clock = Clock::wall();
        let results = runtime.block_on(join_all_bounded(
            limit,
            delays.iter().copied().enumerate().map(|(index, delay)| {
                let clock = clock.clone();
                async move {
                    clock.sleep(Duration::from_millis(delay)).await;
                    index.saturating_mul(3)
                }
            }),
        ))?;
        let expected: Vec<usize> = (0..delays.len())
            .map(|index| index.saturating_mul(3))
            .collect();
        assert_eq!(
            results,
            expected,
            "{} inputs at limit {limit}",
            delays.len()
        );
        sim.record(&format!("limit={limit} inputs={}", delays.len()));
        Ok(())
    })
}

/// The text a panicked input carried, whichever of the two payload types `assert!` raised.
fn panic_text(error: JoinError) -> Option<String> {
    let payload = error.into_panic();
    match payload.downcast_ref::<String>() {
        Some(text) => Some(text.clone()),
        None => payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned()),
    }
}

/// One fan-out of `inputs` futures under `limit`, where input `hostile` panics with its own name
/// and every other input answers its index plus `tag`.
async fn hostile_fan_out(
    limit: usize,
    inputs: usize,
    hostile: Option<usize>,
    tag: usize,
) -> Result<Vec<usize>, FanOutError> {
    join_all_bounded(
        limit,
        (0..inputs).map(move |index| async move {
            lgwks_bot::rt::task::yield_now().await;
            assert!(
                Some(index) != hostile,
                "tenant {tag} input {index} exploded"
            );
            index.saturating_add(tag)
        }),
    )
    .await
}

/// A fan-out holding one hostile input, at any position, width and limit, returns that input's
/// own panic as a typed `FanOutError::Panicked` and never a shorter vector or a process abort.
#[test]
fn a_hostile_input_anywhere_in_a_fan_out_comes_back_as_its_own_typed_panic() -> TestResult {
    let runtime = Runtime::new()?;
    sim::assert_replays(band_of(46), |sim| {
        let limit = usize::try_from(sim.rng().between(1, 8))?;
        let inputs = usize::try_from(sim.rng().between(1, 32))?;
        let hostile = usize::try_from(sim.rng().below(u32::try_from(inputs)?))?;
        let outcome = runtime.block_on(hostile_fan_out(limit, inputs, Some(hostile), 0));
        let text = match outcome {
            Err(FanOutError::Panicked(error)) => panic_text(error),
            other => {
                return Err(format!("{inputs} inputs, {hostile} hostile: got {other:?}").into());
            }
        };
        let expected = format!("tenant 0 input {hostile} exploded");
        assert_eq!(
            text.as_deref(),
            Some(expected.as_str()),
            "the panic is the hostile input's own"
        );
        sim.record(&format!("limit={limit} inputs={inputs} hostile={hostile}"));
        Ok(())
    })
}

/// Two tenants fan out at once on one runtime and one of them holds a hostile input: the hostile
/// tenant gets its own typed panic and the other tenant gets every one of its outputs, in order.
/// `rt::join!` is the `macros` feature's, so the family is compiled where that feature is on.
#[cfg(feature = "macros")]
#[test]
fn a_hostile_tenants_panic_never_reaches_the_other_tenants_fan_out() -> TestResult {
    let runtime = Runtime::new()?;
    sim::assert_replays(band_of(47), |sim| {
        let limit = usize::try_from(sim.rng().between(1, 6))?;
        let inputs = usize::try_from(sim.rng().between(1, 24))?;
        let hostile = usize::try_from(sim.rng().below(u32::try_from(inputs)?))?;
        let (bad, good) = runtime.block_on(async {
            lgwks_bot::rt::join!(
                hostile_fan_out(limit, inputs, Some(hostile), 1_000),
                hostile_fan_out(limit, inputs, None, 2_000),
            )
        });
        let text = match bad {
            Err(FanOutError::Panicked(error)) => panic_text(error),
            other => return Err(format!("the hostile tenant got {other:?}").into()),
        };
        let expected = format!("tenant 1000 input {hostile} exploded");
        assert_eq!(
            text.as_deref(),
            Some(expected.as_str()),
            "tenant 1000 owns its panic"
        );
        let mine: Vec<usize> = (0..inputs)
            .map(|index| index.saturating_add(2_000))
            .collect();
        assert_eq!(
            good?, mine,
            "tenant 2000 keeps every output of its own fan-out"
        );
        sim.record(&format!("limit={limit} inputs={inputs} hostile={hostile}"));
        Ok(())
    })
}

/// One cancel drains every task of a set, however many it owns.
#[test]
fn a_cancelled_token_stops_every_task_a_join_set_owns_at_any_size() -> TestResult {
    let runtime = Runtime::new()?;
    sim::assert_replays(band_of(12), |sim| {
        let owned = usize::try_from(sim.rng().between(1, 16))?;
        let joined = runtime.block_on(async {
            let token = CancellationToken::new();
            let mut tasks = JoinSet::new();
            for _ in 0..owned {
                let worker = token.clone();
                tasks.spawn(async move {
                    worker.cancelled_owned().await;
                });
            }
            token.cancel();
            let mut joined = 0_usize;
            while let Some(done) = tasks.join_next().await {
                done?;
                joined = joined.saturating_add(1);
            }
            Ok::<usize, JoinError>(joined)
        })?;
        assert_eq!(
            joined, owned,
            "every owned task was drained by the one cancel"
        );
        sim.record(&format!("owned={owned}"));
        Ok(())
    })
}

/// A capture keeps the first `ceiling` bytes of what the child wrote, and never more.
#[cfg(all(unix, feature = "process"))]
#[test]
fn a_command_captures_exactly_the_smaller_of_its_output_and_the_ceiling() -> TestResult {
    use std::num::NonZeroUsize;

    use lgwks_bot::rt::process::ProcessSpec;

    let runtime = Runtime::new()?;
    sim::assert_replays(band_of(13), |sim| {
        let written = usize::try_from(sim.rng().between(0, 200))?;
        let ceiling = usize::try_from(sim.rng().between(1, 200))?;
        let captured = runtime.block_on(async {
            let mut spec = ProcessSpec::new("sh");
            spec.arg("-c").arg(format!("head -c {written} /dev/zero"));
            spec.capture_stdout(NonZeroUsize::new(ceiling).ok_or("a non-zero ceiling")?)
                .deadline(Duration::from_secs(10));
            let mut supervisor = Supervisor::new(1);
            let run = supervisor.run_process(&spec).await?;
            Ok::<Vec<u8>, Box<dyn Error>>(run.stdout().bytes().to_vec())
        })?;
        assert_eq!(
            captured.len(),
            written.min(ceiling),
            "{written} bytes written against a ceiling of {ceiling}"
        );
        assert!(
            captured.iter().all(|byte| *byte == 0),
            "the bytes are the child's own"
        );
        sim.record(&format!("written={written} ceiling={ceiling}"));
        Ok(())
    })
}
