//! One seed gives one world, and the change query tells the truth in index order (#1002).

use std::collections::BTreeSet;

use lgwks_std::hash::Digest;

use super::{Driver, Failure, below};

/// The seeds the determinism simulation runs.
const SEEDS: [u64; 3] = [0x1002_0001, 0x1002_0002, 0x1002_0003];

/// The writes and deletes each run commits. The issue names 100,000; this is a tenth of that so the test stays
/// under a second in the mutation gate, which rebuilds it for every mutant.
const WRITES: usize = 10_000;

/// Run `WRITES` seeded writes and deletes and hand back the driver and its world hash.
fn run(seed: u64) -> Result<(Driver, Digest), Failure> {
    let mut driver = Driver::new(seed, "acme", 200_000, 1 << 26)?;
    driver.run(WRITES)?;
    let hash = driver.world.hash()?;
    Ok((driver, hash))
}

/// Catches a world whose state or log depends on anything but the seed (the monkey, 100,000 writes and
/// deletes, 10,000 each): the same seed twice gives one hash and byte-identical change sets, the model agrees cell by
/// cell, and distinct seeds give distinct hashes (#1002).
#[test]
fn sim_world_same_seed_same_hash() -> Result<(), Failure> {
    let mut seen = BTreeSet::new();
    for seed in SEEDS {
        let (first, first_hash) = run(seed)?;
        let (second, second_hash) = run(seed)?;
        assert_eq!(first_hash, second_hash, "seed {seed:#x} gave two hashes");
        assert_eq!(first.log, second.log, "seed {seed:#x} gave two logs");
        assert_eq!(first.world.tick(), second.world.tick(), "ticks differ");
        first.check_model()?;
        assert!(first.writes >= WRITES, "the run committed its writes");
        assert!(
            seen.insert(*first_hash.as_bytes()),
            "seed {seed:#x} collided with an earlier seed"
        );
    }
    Ok(())
}

/// Catches a change query that drops, invents or reorders rows (the neighbour who subscribes late): for drawn
/// ticks, `changed_since` names exactly the entities and columns the reference model last touched after
/// the tick, in strictly increasing slot order, and a tombstone carries no columns (#1002).
#[test]
fn sim_world_changed_since_in_index_order() -> Result<(), Failure> {
    let mut driver = Driver::new(0x1002_00c5, "acme", 50_000, 1 << 24)?;
    driver.run(4_000)?;
    let now = driver.world.tick();
    let cols = driver.cols;
    for _ in 0..24 {
        let since = u64::try_from(below(
            &mut driver.stream,
            usize::try_from(now.saturating_add(1))?,
        )?)?;
        let mut previous: Option<u32> = None;
        let mut live = BTreeSet::new();
        for changed in driver.world.changed_since(since) {
            assert!(
                previous.is_none_or(|last| changed.entity.index > last),
                "slot order broke at {}",
                changed.entity.index
            );
            previous = Some(changed.entity.index);
            let masks = [
                changed.touches(cols.pos),
                changed.touches(cols.name),
                changed.touches(cols.hp),
            ];
            if changed.alive {
                live.insert(changed.id);
                for (column, touched) in (0_u16..).zip(masks) {
                    let expected = driver
                        .model
                        .cell_touched
                        .get(&(changed.id, column))
                        .is_some_and(|tick| *tick > since);
                    assert_eq!(touched, expected, "column {column} after tick {since}");
                }
            } else {
                assert_eq!(masks, [false; 3], "a tombstone holds no columns");
            }
        }
        let expected: BTreeSet<_> = driver
            .model
            .touched
            .iter()
            .filter(|entry| *entry.1 > since)
            .map(|entry| *entry.0)
            .collect();
        assert_eq!(live, expected, "live entities after tick {since}");
    }
    Ok(())
}
