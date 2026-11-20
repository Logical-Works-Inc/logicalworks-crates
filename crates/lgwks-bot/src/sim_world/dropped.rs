//! A dropped transaction leaves the bytes of the world as they were (#1002, ADR 0053 decision 4).

use lgwks_std::hash::Digest;
use lgwks_std::id::Uuid;

use super::{Cols, Driver, Failure, Name, Planned, Pos, below, buffer, mint};
use crate::world::MAX_VALUE_BYTES;
use crate::world::error::{Limit, WorldError};
use crate::world::state::World;
use crate::world::tx::Tx;

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

/// A transaction on `world` holding every planned write, buffered and not committed.
fn buffered<'world>(
    world: &'world mut World,
    cols: Cols,
    planned: &[Planned],
) -> Result<Tx<'world>, Failure> {
    let mut tx = world.begin();
    for write in planned {
        buffer(&mut tx, cols, write)?;
    }
    Ok(tx)
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

/// Every way the system ends, in the order the rounds cycle through them.
const ENDINGS: [Ending; 4] = [
    Ending::Dropped,
    Ending::Refused,
    Ending::Capped,
    Ending::Panicked,
];

/// The discriminating test (the pulled plug, for every N in a seeded range): a system buffers N writes and
/// then drops, is refused, hits a ceiling or panics, and the hash, tick, counts and change feed before and
/// after are equal. The control commits the same writes and must move the hash, so a hash that never moves
/// cannot pass (#1002).
#[test]
fn sim_world_dropped_tx_leaves_bytes() -> Result<(), Failure> {
    let mut driver = Driver::new(0x1002_d209, "acme", 50_000, 1 << 24)?;
    driver.run(2_000)?;
    let cols = driver.cols;
    let mut moved = 0_usize;
    for (round, ending) in ENDINGS.iter().copied().cycle().take(96).enumerate() {
        let count = below(&mut driver.stream, 41)?;
        let before = print(&driver.world)?;
        let planned = driver.plan(count)?;
        match ending {
            Ending::Dropped => {
                let tx = buffered(&mut driver.world, cols, &planned)?;
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
                let mut tx = buffered(&mut driver.world, cols, &planned)?;
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
                let mut tx = buffered(&mut driver.world, cols, &planned)?;
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
                            let Ok(tx) = buffered(world, cols, &planned) else {
                                return;
                            };
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
        buffered(&mut driver.world, cols, &planned)?.commit()?;
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
