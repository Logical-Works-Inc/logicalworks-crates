//! A dropped transaction leaves the bytes of the world as they were (#1002, ADR 0053 decision 4).

use lgwks_std::hash::Digest;
use lgwks_std::id::Uuid;

use super::{Driver, Failure, Name, Planned, Pos, below, buffer, mint};
use crate::world::MAX_VALUE_BYTES;
use crate::world::error::{Limit, WorldError};
use crate::world::state::World;

/// Everything about a world a dropped transaction must leave alone.
#[derive(Debug, PartialEq, Eq)]
struct Print {
    /// The world hash over the canonical column form.
    hash: Digest,
    /// The tick.
    tick: u64,
    /// Live entities.
    live: usize,
    /// Live value bytes.
    bytes: usize,
    /// Entities `changed_since(0)` names.
    changed: usize,
}

/// The print of `world`.
fn print(world: &World) -> Result<Print, Failure> {
    Ok(Print {
        hash: world.hash()?,
        tick: world.tick(),
        live: world.live(),
        bytes: world.bytes(),
        changed: world.changed_since(0).count(),
    })
}

/// How the system under test ends after its writes.
#[derive(Debug, Clone, Copy)]
enum Ending {
    /// It returns without committing.
    Dropped,
    /// A write of its own is refused and it gives up.
    Refused,
    /// A write passes a ceiling and it gives up.
    Capped,
    /// It panics on a thread of its own, and the transaction drops during the unwind.
    Panicked,
}

/// The discriminating test (the pulled plug, for every N in a seeded range): a system buffers N writes and
/// then drops, is refused, hits a ceiling or panics, and the hash, tick, counts and change feed before and
/// after are equal. The control commits the same writes and must move the hash, so a hash that never moves
/// cannot pass (#1002).
#[test]
fn sim_world_dropped_tx_leaves_bytes() -> Result<(), Failure> {
    let mut driver = Driver::new(0x1002_d209, "acme", 50_000, 1 << 24)?;
    driver.run(2_000)?;
    let cols = driver.cols;
    let endings = [
        Ending::Dropped,
        Ending::Refused,
        Ending::Capped,
        Ending::Panicked,
    ];
    let mut moved = 0_usize;
    for (round, ending) in endings.iter().copied().cycle().take(96).enumerate() {
        let count = below(&mut driver.stream, 41)?;
        let before = print(&driver.world)?;
        let planned = driver.plan(count)?;
        match ending {
            Ending::Dropped => {
                let mut tx = driver.world.begin();
                for write in &planned {
                    buffer(&mut tx, cols, write)?;
                }
                let removes = planned
                    .iter()
                    .filter(|write| matches!(write, Planned::Remove(..)))
                    .count();
                assert!(
                    tx.pending() <= planned.len()
                        && tx.pending() >= planned.len().saturating_sub(removes),
                    "{} writes were buffered of {} planned, {removes} of them removes",
                    tx.pending(),
                    planned.len()
                );
            }
            Ending::Refused => {
                let mut tx = driver.world.begin();
                for write in &planned {
                    buffer(&mut tx, cols, write)?;
                }
                let unissued = Uuid::from([0_u8; 16]);
                assert!(
                    matches!(tx.spawn(unissued), Err(WorldError::NotV7 { .. })),
                    "an id that is not version 7 is refused"
                );
                let stranger = mint(u64::MAX, 7);
                assert!(
                    matches!(
                        tx.set(cols.pos, stranger, &Pos { x: 1, y: 1 }),
                        Err(WorldError::NoSuchEntity { .. })
                    ),
                    "a write to an entity that is not alive is refused"
                );
            }
            Ending::Capped => {
                let mut tx = driver.world.begin();
                for write in &planned {
                    buffer(&mut tx, cols, write)?;
                }
                let id = mint(u64::MAX, 9);
                tx.spawn(id)?;
                let huge = Name("x".repeat(MAX_VALUE_BYTES.saturating_add(1)));
                assert!(
                    matches!(
                        tx.set(cols.name, id, &huge),
                        Err(WorldError::Ceiling {
                            what: Limit::ValueBytes,
                            ..
                        })
                    ),
                    "a value past its ceiling is refused"
                );
            }
            Ending::Panicked => {
                let world = &mut driver.world;
                let outcome = std::thread::scope(|scope| {
                    scope
                        .spawn(|| {
                            let mut tx = world.begin();
                            for write in &planned {
                                if buffer(&mut tx, cols, write).is_err() {
                                    return;
                                }
                            }
                            assert_eq!(
                                tx.pending(),
                                usize::MAX,
                                "the system under test fails after its writes"
                            );
                        })
                        .join()
                });
                assert!(outcome.is_err(), "the system panicked");
            }
        }
        assert_eq!(
            print(&driver.world)?,
            before,
            "round {round} ({ending:?}, {count} writes) changed the world"
        );
        let mut tx = driver.world.begin();
        for write in &planned {
            buffer(&mut tx, cols, write)?;
        }
        tx.commit()?;
        let tick = driver.world.tick();
        for write in &planned {
            driver.model.apply(write, tick);
        }
        if tick != before.tick {
            moved = moved.saturating_add(1);
            assert_ne!(
                driver.world.hash()?,
                before.hash,
                "the control commit of round {round} left the hash alone"
            );
        }
    }
    assert!(moved > 60, "the control moved the world in {moved} rounds");
    driver.check_model()
}
