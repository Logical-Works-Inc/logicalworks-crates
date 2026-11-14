//! Snapshots round-trip: a chain of snapshots restores a world with the donor's hash (#1002).
//!
//! The property draws a seed, a number of steps and which steps cut a snapshot. The mutant it must catch is a
//! restore that skips the last snapshot of the chain.

use proptest::collection::vec;
use proptest::prelude::{Strategy, any};
use proptest::test_runner::TestCaseError;

use super::{Driver, world};

#[path = "../../../lgwks-std/tests/support/prop.rs"]
mod prop;

/// The seed of the property's corpus: one seed, one replay.
const SEED: u64 = 0x1002_5afe;

/// The most steps a drawn run takes.
const MOST_STEPS: usize = 24;

/// One drawn run: a seed for the writes, a step count, and the steps after which a snapshot is cut.
#[derive(Debug, Clone)]
struct Plan {
    /// The seed the donor's writes are drawn from.
    seed: u64,
    /// How many transactions the donor commits.
    steps: usize,
    /// Whether a snapshot is cut after step `i`; the last step always cuts one.
    cuts: Vec<bool>,
}

/// The strategy that draws a [`Plan`].
fn plans() -> impl Strategy<Value = Plan> {
    (any::<u64>(), 1..MOST_STEPS, vec(any::<bool>(), MOST_STEPS))
        .prop_map(|(seed, steps, cuts)| Plan { seed, steps, cuts })
}

/// Run `plan` on a donor, cut its chain, restore a fresh world from the chain and compare hashes. With
/// `mutant` set, the restore skips the last snapshot, which is the defect the property must catch.
fn roundtrip(plan: &Plan, mutant: bool) -> Result<(), TestCaseError> {
    let mut donor = prop::setup(Driver::new(plan.seed, "acme", 2_000, 1 << 22))?;
    let mut chain: Vec<Vec<u8>> = Vec::new();
    let mut since = 0_u64;
    for (step, cut) in plan.cuts.iter().copied().enumerate().take(plan.steps) {
        prop::setup(donor.step())?;
        if cut || step.saturating_add(1) == plan.steps {
            let snapshot = prop::setup(donor.world.snapshot_since(since))?;
            chain.push(snapshot.to_vec());
            since = donor.world.tick();
        }
    }
    if mutant {
        chain.pop();
    }
    let (mut target, _cols) = prop::setup(world("acme", 2_000, 1 << 22))?;
    prop::setup(target.restore(&chain))?;
    let (want, got) = (
        prop::setup(donor.world.hash())?,
        prop::setup(target.hash())?,
    );
    prop::check(want == got, || {
        format!(
            "{} snapshots of {} steps restored a world that differs from the donor",
            chain.len(),
            plan.steps
        )
    })?;
    prop::check(target.tick() == donor.world.tick(), || {
        format!(
            "restored tick {} differs from {}",
            target.tick(),
            donor.world.tick()
        )
    })
}

/// Catches a snapshot chain that loses state (the liar upstream is the checkpoint): for drawn seeds, step
/// counts and cut points, the chain restores to the donor's hash and tick; and the same property fails on a
/// restore that skips the last snapshot, with a shrunk input that the real restore passes (#1002).
#[test]
fn prop_snapshot_roundtrip() -> prop::Outcome {
    prop::runner(SEED, 64, file!()).run(&plans(), |plan| roundtrip(&plan, false))?;
    let minimal = prop::shrunk(SEED, &plans(), |plan| roundtrip(&plan, true))?;
    assert!(
        roundtrip(&minimal, false).is_ok(),
        "the real restore passes the input that beat the mutant: {minimal:?}"
    );
    assert!(
        roundtrip(&minimal, true).is_err(),
        "the shrunk input still beats the mutant: {minimal:?}"
    );
    Ok(())
}
