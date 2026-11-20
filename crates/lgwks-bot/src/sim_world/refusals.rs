//! Each failure path answers with its own exact variant, and a refusal leaves the world as it was (#1002).

use lgwks_std::id::Uuid;
use lgwks_std::wire::{AlignedVec, WireError, from_bytes, to_bytes};

use super::{Drift, Driver, Failure, Hp, Name, Pos, fail, mint, world};
use crate::world::MAX_TX_OPS;
use crate::world::codec::{ChangeSet, Op, Snapshot};
use crate::world::component::{Ceilings, WorldBuilder};
use crate::world::digest::change_digest;
use crate::world::error::{Broken, Defect, WorldError};

/// A snapshot defect to plant, and the refusal it must meet.
struct Case {
    /// What the case plants.
    label: &'static str,
    /// The plant.
    mutate: fn(&mut Snapshot),
    /// Whether the refusal is the right one.
    expect: fn(&WorldError) -> bool,
}

/// The cases: one per defect the snapshot reader names.
const CASES: [Case; 13] = [
    Case {
        label: "a snapshot that starts after another tick",
        mutate: |snapshot| snapshot.since = 5,
        expect: |error| {
            matches!(
                *error,
                WorldError::ChainBroken {
                    expected: 0,
                    found: 5
                }
            )
        },
    },
    Case {
        label: "rows out of order",
        mutate: |snapshot| snapshot.rows.reverse(),
        expect: |error| matches!(*error, WorldError::Malformed(Defect::RowOrder)),
    },
    Case {
        label: "a row past the slot table",
        mutate: |snapshot| {
            let past = snapshot.slots;
            if let Some(row) = snapshot.rows.last_mut() {
                row.index = past;
            }
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::RowIndex)),
    },
    Case {
        label: "a row from the future",
        mutate: |snapshot| {
            let future = snapshot.tick.saturating_add(1);
            if let Some(row) = snapshot.rows.first_mut() {
                row.tick = future;
            }
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::TickRange)),
    },
    Case {
        label: "cells out of order",
        mutate: |snapshot| {
            if let Some(row) = snapshot.rows.iter_mut().find(|row| row.cells.len() > 1) {
                row.cells.reverse();
            }
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::CellOrder)),
    },
    Case {
        label: "a cell in a column nobody registered",
        mutate: |snapshot| {
            if let Some(cell) = snapshot
                .rows
                .iter_mut()
                .flat_map(|row| &mut row.cells)
                .last()
            {
                cell.column = 9;
            }
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::CellColumn)),
    },
    Case {
        label: "a value that is not the canonical bytes of its column",
        mutate: |snapshot| {
            let hp = snapshot
                .rows
                .iter_mut()
                .flat_map(|row| &mut row.cells)
                .find(|cell| cell.column == 2 && cell.value.is_some());
            if let Some(cell) = hp {
                cell.value = Some(vec![1, 2, 3]);
            }
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::NotCanonical)),
    },
    Case {
        label: "a dead row that holds cells",
        mutate: |snapshot| {
            if let Some(row) = snapshot.rows.iter_mut().find(|row| !row.cells.is_empty()) {
                row.alive = false;
            }
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::DeadRowCells)),
    },
    Case {
        label: "a live row whose id is not version 7",
        mutate: |snapshot| {
            if let Some(row) = snapshot.rows.iter_mut().find(|row| row.alive) {
                row.id = [0; 16];
            }
        },
        expect: |error| matches!(*error, WorldError::NotV7 { .. }),
    },
    Case {
        label: "two live rows with one id",
        mutate: |snapshot| {
            let mut alive = snapshot.rows.iter_mut().filter(|row| row.alive);
            if let (Some(first), Some(second)) = (alive.next(), alive.next()) {
                second.id = first.id;
            }
        },
        expect: |error| matches!(*error, WorldError::DuplicateEntity { .. }),
    },
    Case {
        label: "a free list that names a slot the table lacks",
        mutate: |snapshot| {
            let outside = snapshot.slots.saturating_add(5);
            snapshot.free.push(outside);
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::FreeList)),
    },
    Case {
        label: "a slot table past the entity ceiling",
        mutate: |snapshot| snapshot.slots = 100_000,
        expect: |error| matches!(*error, WorldError::Malformed(Defect::SlotTable)),
    },
    Case {
        label: "a span that ends before it starts",
        mutate: |snapshot| {
            snapshot.since = 0;
            snapshot.tick = 0;
        },
        expect: |error| matches!(*error, WorldError::Malformed(Defect::TickRange)),
    },
];

/// The wire form of a snapshot, archived apart from the world's own writer.
pub(super) fn encoded(snapshot: &Snapshot) -> Result<Vec<u8>, Failure> {
    Ok(to_bytes::<WireError>(snapshot)
        .map_err(|cause| Failure::World(WorldError::Wire(cause)))?
        .to_vec())
}

/// The wire form of a change set, archived apart from the world's own writer.
pub(super) fn encoded_change(change: &ChangeSet) -> Result<Vec<u8>, Failure> {
    Ok(to_bytes::<WireError>(change)
        .map_err(|cause| Failure::World(WorldError::Wire(cause)))?
        .to_vec())
}

/// A logged change set read back, by a reader apart from the world's own.
pub(super) fn decoded_change(real: &[u8]) -> Result<ChangeSet, Failure> {
    let mut aligned = AlignedVec::<16>::with_capacity(real.len());
    aligned.extend_from_slice(real);
    from_bytes::<ChangeSet, WireError>(&aligned)
        .map_err(|cause| Failure::World(WorldError::Wire(cause)))
}

/// Catches a snapshot reader that keeps what it should refuse (the liar upstream): each planted defect meets
/// its own variant, the untouched snapshot restores to the donor's hash, and a refusal leaves a world with
/// the hash of the empty one (#1002).
#[test]
fn restore_meets_each_defect_with_its_variant() -> Result<(), Failure> {
    let mut donor = Driver::new(0x1002_5a47, "acme", 50_000, 1 << 24)?;
    donor.run(1_500)?;
    let full = donor.world.snapshot_since(0)?;
    let plain: Snapshot = from_bytes::<Snapshot, WireError>(&full)
        .map_err(|cause| Failure::World(WorldError::Wire(cause)))?;
    let (mut clean, _cols) = world("acme", 50_000, 1 << 24)?;
    clean.restore(&[full.as_slice()])?;
    assert_eq!(
        clean.hash()?,
        donor.world.hash()?,
        "the untouched snapshot restores"
    );
    for case in &CASES {
        let mut planted = plain.clone();
        (case.mutate)(&mut planted);
        assert_ne!(planted, plain, "{} changed nothing", case.label);
        let (mut target, _cols) = world("acme", 50_000, 1 << 24)?;
        let empty = target.hash()?;
        let outcome = target.restore(&[encoded(&planted)?]);
        assert!(
            matches!(outcome, Err(ref error) if (case.expect)(error)),
            "{} met {outcome:?}",
            case.label
        );
        assert_eq!(target.hash()?, empty, "{} moved the world", case.label);
    }
    Ok(())
}

/// Catches a restore that lets a slot's generation run backwards, and a tick that cannot advance (#1002).
#[test]
fn restore_refuses_a_generation_that_runs_backwards_and_a_tick_that_cannot_advance()
-> Result<(), Failure> {
    let mut donor = Driver::new(0x1002_6e17, "acme", 50_000, 1 << 24)?;
    donor.run(1_500)?;
    let full = donor.world.snapshot_since(0)?;
    let mut plain: Snapshot = from_bytes::<Snapshot, WireError>(&full)
        .map_err(|cause| Failure::World(WorldError::Wire(cause)))?;
    let (mut target, _cols) = world("acme", 50_000, 1 << 24)?;
    target.restore(&[full.as_slice()])?;
    let mut back = plain
        .rows
        .iter()
        .find(|row| row.generation > 0)
        .cloned()
        .ok_or(Failure::Check("the donor reused a slot"))?;
    back.generation = 0;
    back.alive = false;
    back.cells.clear();
    back.tick = plain.tick.saturating_add(1);
    plain.since = plain.tick;
    plain.tick = plain.tick.saturating_add(1);
    plain.rows = vec![back];
    let before = target.hash()?;
    assert!(
        matches!(
            target.restore(&[encoded(&plain)?]),
            Err(WorldError::Malformed(Defect::Generation))
        ),
        "a generation behind the slot is refused"
    );
    assert_eq!(target.hash()?, before, "the refusal moved the world");

    let (mut worn, _worn_cols) = world("acme", 8, 1 << 20)?;
    plain.since = 0;
    plain.tick = u64::MAX;
    plain.slots = 0;
    plain.free.clear();
    plain.rows.clear();
    // The snapshot names the hash of the world it makes. The forger learns it from the refusal the stale one
    // meets, as a forger holding the hash function would compute it.
    let Err(WorldError::HashMismatch { found }) = worn.restore(&[encoded(&plain)?]) else {
        return fail(Failure::Check("a snapshot with a stale hash was kept"));
    };
    plain.after = found;
    worn.restore(&[encoded(&plain)?])?;
    let mut tx = worn.begin();
    tx.spawn(mint(1, 1))?;
    assert!(
        matches!(tx.commit(), Err(WorldError::TickExhausted)),
        "the tick cannot pass its last value"
    );
    assert_eq!(worn.live(), 0, "the refused commit moved the world");
    Ok(())
}

/// The registration half of the check below: a bad tenant, a column twice, a lying codec.
fn registration_meets_its_variants(ceilings: Ceilings) -> Result<(), Failure> {
    assert!(
        matches!(
            WorldBuilder::new("", ceilings),
            Err(WorldError::Tenant { len: 0 })
        ),
        "an empty tenant name"
    );
    assert!(
        matches!(
            WorldBuilder::new(&"t".repeat(129), ceilings),
            Err(WorldError::Tenant { len: 129 })
        ),
        "a tenant name past 128 bytes"
    );
    let mut builder = WorldBuilder::new("acme", ceilings)?;
    builder.column(&Pos { x: 0, y: 0 })?;
    assert!(
        matches!(
            builder.column(&Pos { x: 1, y: 1 }),
            Err(WorldError::DuplicateColumn { name: "pos" })
        ),
        "a column registered twice"
    );
    assert!(
        matches!(
            builder.column(&Drift(3)),
            Err(WorldError::CodecDrift { column: "drift" })
        ),
        "a codec that does not reproduce its bytes"
    );
    Ok(())
}

/// Catches registration and write refusals that name the wrong cause: a column twice, a lying codec, a bad
/// tenant, a handle from another world, an id that is not version 7, one alive twice, one not alive (#1002).
#[test]
fn registration_and_writes_meet_their_own_variants() -> Result<(), Failure> {
    let ceilings = Ceilings::new(4, 8, 1 << 10)?;
    registration_meets_its_variants(ceilings)?;
    let (mut mine, cols) = world("acme", 8, 1 << 10)?;
    let mut other = WorldBuilder::new("acme", ceilings)?;
    other.column(&Name(String::new()))?;
    let mut foreign = other.build();
    let id = mint(1, 1);
    let mut tx = foreign.begin();
    tx.spawn(id)?;
    assert!(
        matches!(
            tx.set(cols.pos, id, &Pos { x: 1, y: 1 }),
            Err(WorldError::ColumnType {
                index: 0,
                expected: "pos"
            })
        ),
        "a handle for another component"
    );
    assert!(
        matches!(
            tx.set(cols.hp, id, &Hp(1)),
            Err(WorldError::UnknownColumn { index: 2 })
        ),
        "a handle past the registered columns"
    );
    drop(tx);

    let mut tx = mine.begin();
    let not_seven = Uuid::from([0x40; 16]);
    assert!(
        matches!(tx.spawn(not_seven), Err(WorldError::NotV7 { .. })),
        "version 4 bytes"
    );
    tx.spawn(id)?;
    assert!(
        matches!(tx.spawn(id), Err(WorldError::DuplicateEntity { .. })),
        "alive twice"
    );
    let absent = mint(2, 2);
    assert!(
        matches!(tx.despawn(absent), Err(WorldError::NoSuchEntity { .. })),
        "despawn of nobody"
    );
    assert!(
        matches!(
            tx.remove(cols.hp, absent),
            Err(WorldError::NoSuchEntity { .. })
        ),
        "remove on nobody"
    );
    assert!(!tx.remove(cols.hp, id)?, "an empty cell is not removed");
    assert!(
        matches!(tx.get(cols.hp, &id), Err(WorldError::NoSuchEntity { .. })),
        "a buffered spawn is not visible before commit"
    );
    tx.commit()?;
    assert!(
        matches!(
            mine.get(cols.hp, &absent),
            Err(WorldError::NoSuchEntity { .. })
        ),
        "read of nobody"
    );
    assert_eq!(mine.get(cols.hp, &id)?, None, "an empty cell reads empty");
    assert!(
        matches!(
            mine.raw(7, &id),
            Err(WorldError::UnknownColumn { index: 7 })
        ),
        "raw past the columns"
    );
    assert_eq!(
        mine.entity(&id).map(|entity| entity.generation),
        Some(0),
        "a first life"
    );
    assert!(
        matches!(
            mine.snapshot_since(9),
            Err(WorldError::FutureTick { asked: 9, now: 1 })
        ),
        "a snapshot from a tick ahead"
    );
    Ok(())
}

/// Catches a change set that a tenant, a layout, a count or a value can slip past replay (#1002).
#[test]
fn replay_meets_each_defect_with_its_variant() -> Result<(), Failure> {
    let mut donor = Driver::new(0x1002_4e51, "acme", 5_000, 1 << 22)?;
    donor.run(60)?;
    let real = donor
        .log
        .first()
        .ok_or(Failure::Check("the donor logged"))?;
    let mut target = world("acme", 5_000, 1 << 22)?.0;
    let plain = decoded_change(real)?;

    let mut stranger = plain.clone();
    stranger.tenant = "globex".to_owned();
    assert!(
        matches!(
            target.replay(&encoded_change(&stranger)?),
            Err(WorldError::TenantMismatch)
        ),
        "another tenant's change set"
    );
    let mut relaid = plain.clone();
    relaid.layout = [7; 32];
    assert!(
        matches!(
            target.replay(&encoded_change(&relaid)?),
            Err(WorldError::LayoutMismatch)
        ),
        "another layout's change set"
    );
    let mut crowd = plain.clone();
    crowd.ops = (1..=u64::try_from(MAX_TX_OPS)?.saturating_add(1))
        .map(|seq| Op::Spawn {
            id: *mint(seq, 1).as_bytes(),
        })
        .collect();
    assert!(
        matches!(
            target.replay(&encoded_change(&crowd)?),
            Err(WorldError::Malformed(Defect::TooManyOps))
        ),
        "more operations than a transaction holds"
    );
    let id = mint(1, 1);
    let mut crooked = plain;
    crooked.ops = vec![
        Op::Spawn { id: *id.as_bytes() },
        Op::Set {
            column: 2,
            id: *id.as_bytes(),
            value: vec![1, 2, 3],
        },
    ];
    crooked.digest = change_digest(&crooked)?;
    assert!(
        matches!(
            target.replay(&encoded_change(&crooked)?),
            Err(WorldError::Malformed(Defect::NotCanonical))
        ),
        "a value that is not canonical for its column"
    );
    assert!(
        matches!(target.replay(b"not an archive"), Err(WorldError::Wire(_))),
        "bytes that are no archive"
    );
    assert_eq!(target.live(), 0, "no refusal moved the world");
    Ok(())
}

/// Catches an arm of `advance` that changes state before it has checked (the pulled plug inside the world): a
/// spawn of a live id, a despawn and a write to an id that is not alive, and a write to a column that does not
/// exist each answer with their own named variant and leave the hash and the tick as they were. Then two new
/// spawns prove the tables did not move: the first takes the one freed slot and the second opens slot two, so a
/// refusal that took the free slot or grew the table is caught by index. `Tx` never lets these writes reach
/// `advance`, so the sim drives it directly. It lives here, not in `world/`, because the `world-bench` bin
/// compiles `world/` by path and a test there ran under two binaries with one name (#1002).
#[test]
fn sim_world_refused_arm_of_advance_leaves_the_world_as_it_was() -> Result<(), Failure> {
    let mut bare = WorldBuilder::new("acme", Ceilings::new(1, 8, 1 << 10)?)?.build();
    let (live, gone, absent) = ([7_u8; 16], [8_u8; 16], [9_u8; 16]);
    bare.advance(&[Op::Spawn { id: live }, Op::Spawn { id: gone }], [1; 32])?;
    bare.advance(&[Op::Despawn { id: gone }], [2; 32])?;
    let (before, tick) = (bare.hash()?, bare.tick());
    let cases = [
        (Op::Spawn { id: live }, Broken::SpawnOfLiveId),
        (Op::Despawn { id: absent }, Broken::DespawnOfAbsentId),
        (
            Op::Set {
                column: 0,
                id: absent,
                value: vec![1],
            },
            Broken::WriteToAbsentId,
        ),
        (
            Op::Remove {
                column: 0,
                id: absent,
            },
            Broken::WriteToAbsentId,
        ),
        (
            Op::Set {
                column: 0,
                id: live,
                value: vec![1],
            },
            Broken::ColumnMissing,
        ),
    ];
    for (op, broken) in cases {
        let outcome = bare.advance(std::slice::from_ref(&op), [3; 32]);
        assert!(
            matches!(outcome, Err(WorldError::Inconsistent(found)) if found == broken),
            "{op:?} met {outcome:?}, not {broken:?}"
        );
        assert_eq!(bare.hash()?, before, "{op:?} moved the world");
        assert_eq!(bare.tick(), tick, "{op:?} moved the tick");
        assert_eq!(
            bare.entity(&Uuid::from(live)).map(|held| held.index),
            Some(0),
            "{op:?} moved the live entity"
        );
    }
    assert_eq!(
        bare.changed_since(0).count(),
        2,
        "a refusal grew the slot table"
    );
    let (reuse, fresh) = ([10_u8; 16], [11_u8; 16]);
    bare.advance(&[Op::Spawn { id: reuse }, Op::Spawn { id: fresh }], [4; 32])?;
    let index = |id: [u8; 16]| bare.entity(&Uuid::from(id)).map(|held| held.index);
    assert_eq!(index(reuse), Some(1), "a refusal took the free slot");
    assert_eq!(index(fresh), Some(2), "a refusal grew the slot table");
    Ok(())
}
