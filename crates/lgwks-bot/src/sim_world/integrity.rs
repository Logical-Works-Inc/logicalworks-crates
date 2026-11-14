//! Integrity: bytes that were altered, forged, or taken from another history are refused, and a refusal moves
//! nothing (#1002).

use lgwks_std::wire::{WireError, from_bytes};

use super::refusals::{decoded_change, encoded, encoded_change};
use super::{Driver, Failure, below, mint, world};
use crate::world::codec::{Op, Snapshot};
use crate::world::error::{Defect, Limit, WorldError};
use crate::world::state::CELL_COST;
use crate::world::{MAX_BYTES, MAX_ENTITIES};

/// Step `driver` until its world stands on `tick`.
fn reach(driver: &mut Driver, tick: u64) -> Result<(), Failure> {
    while driver.world.tick() < tick {
        driver.step()?;
    }
    Ok(())
}

/// A snapshot read back by a reader apart from the world's own.
fn decoded_snapshot(bytes: &[u8]) -> Result<Snapshot, Failure> {
    from_bytes::<Snapshot, WireError>(bytes)
        .map_err(|cause| Failure::World(WorldError::Wire(cause)))
}

/// Catches a snapshot that restores silently after a byte changed (the saboteur): every byte of a snapshot is
/// flipped in turn, on a seeded bit, and each flip is a refusal that leaves the target as it was, or a restore
/// that still reaches the donor's hash. A restore that keeps a different world fails the run (#1002).
#[test]
fn sim_world_saboteur_flipped_byte_in_snapshot_refused() -> Result<(), Failure> {
    let mut donor = Driver::new(0x1002_f11b, "acme", 5_000, 1 << 22)?;
    reach(&mut donor, 40)?;
    let bytes = donor.world.snapshot_since(0)?.to_vec();
    let want = donor.world.hash()?;
    let (empty, _cols) = world("acme", 5_000, 1 << 22)?;
    let before = empty.hash()?;
    let mut refused = 0_usize;
    for at in 0..bytes.len() {
        let bit = below(&mut donor.stream, 8)?;
        let mut flipped = bytes.clone();
        if let Some(byte) = flipped.get_mut(at) {
            *byte ^= 1_u8.rotate_left(u32::try_from(bit)?);
        }
        let mut target = empty.clone();
        match target.restore(&[flipped]) {
            Err(_refusal) => {
                refused = refused.saturating_add(1);
                assert_eq!(
                    target.hash()?,
                    before,
                    "a refusal at byte {at} moved the world"
                );
            }
            Ok(()) => assert_eq!(
                target.hash()?,
                want,
                "byte {at} flipped and the restore kept a different world"
            ),
        }
    }
    // The bytes that change nothing are the archive's padding; the rest are each a refusal.
    assert!(
        refused.saturating_mul(2) > bytes.len(),
        "only {refused} of {} flips were refused",
        bytes.len()
    );
    Ok(())
}

/// Catches a reader that takes another history for its own (the liar upstream): a delta snapshot and a next
/// change set from a world of the same tenant, layout and tick but another history are each refused, and the
/// world they were offered to is as it was (#1002).
#[test]
fn sim_world_foreign_history_at_the_same_tick_refused() -> Result<(), Failure> {
    let mut mine = Driver::new(0x1002_0a11, "acme", 5_000, 1 << 22)?;
    let mut theirs = Driver::new(0x1002_0b22, "acme", 5_000, 1 << 22)?;
    reach(&mut mine, 30)?;
    reach(&mut theirs, 30)?;
    assert_ne!(
        mine.world.hash()?,
        theirs.world.hash()?,
        "two seeds gave one history"
    );
    let before = mine.world.hash()?;
    reach(&mut theirs, 31)?;
    let next = theirs
        .log
        .last()
        .ok_or(Failure::Check("their history logged"))?;
    assert!(
        matches!(mine.world.replay(next), Err(WorldError::Diverged)),
        "their change set continued another history"
    );
    reach(&mut theirs, 40)?;
    let delta = theirs.world.snapshot_since(30)?;
    let outcome = mine.world.restore(&[delta.as_slice()]);
    assert!(
        outcome.is_err(),
        "a delta from another history at the same tick restored"
    );
    assert_eq!(mine.world.hash()?, before, "a refusal moved the world");
    Ok(())
}

/// Catches bytes of an unknown format and a change set that is not the one its digest names (the saboteur):
/// each is its own variant, and the world is as it was (#1002).
#[test]
fn sim_world_unknown_format_and_forged_digest_refused() -> Result<(), Failure> {
    let mut donor = Driver::new(0x1002_d16e, "acme", 5_000, 1 << 22)?;
    reach(&mut donor, 10)?;
    let real = donor
        .log
        .first()
        .ok_or(Failure::Check("the donor logged"))?;
    let plain = decoded_change(real)?;
    let (mut target, _cols) = world("acme", 5_000, 1 << 22)?;
    let before = target.hash()?;

    let mut future = plain.clone();
    future.format = 2;
    assert!(
        matches!(
            target.replay(&encoded_change(&future)?),
            Err(WorldError::UnsupportedFormat { found: 2 })
        ),
        "a change set in format 2"
    );
    let mut forged = plain;
    forged.ops.push(Op::Spawn {
        id: *mint(999, 1).as_bytes(),
    });
    assert!(
        matches!(
            target.replay(&encoded_change(&forged)?),
            Err(WorldError::DigestMismatch)
        ),
        "a change set with a write its digest does not name"
    );
    let mut snapshot = decoded_snapshot(&donor.world.snapshot_since(0)?)?;
    snapshot.format = 9;
    assert!(
        matches!(
            target.restore(&[encoded(&snapshot)?]),
            Err(WorldError::UnsupportedFormat { found: 9 })
        ),
        "a snapshot in format 9"
    );
    assert_eq!(target.hash()?, before, "a refusal moved the world");
    Ok(())
}

/// Catches a restore that allocates what a snapshot says before it checks what the snapshot carries (the
/// liar upstream): a snapshot that claims the most slots a world may hold and names none is a `SlotTable`
/// refusal; one that names every slot it claims, past what the bytes ceiling gives the cell tables, is a
/// `Ceiling` refusal with the exact ask; and a refusal leaves the world as it was (#1002).
#[test]
fn sim_world_liar_upstream_snapshot_claims_huge_slots_refused_before_alloc() -> Result<(), Failure>
{
    let mut donor = Driver::new(0x1002_a110, "acme", 5_000, 1 << 22)?;
    reach(&mut donor, 5)?;
    let plain = decoded_snapshot(&donor.world.snapshot_since(0)?)?;

    // The most slots a world may hold, with nothing behind the claim. The wide world is empty, so the claim
    // is the whole of what the restore would have grown by.
    let (mut wide, _cols) = world("acme", MAX_ENTITIES, MAX_BYTES)?;
    let before = wide.hash()?;
    let mut claim = plain.clone();
    claim.slots = u32::try_from(MAX_ENTITIES)?;
    claim.free.clear();
    claim.rows.clear();
    assert!(
        matches!(
            wide.restore(&[encoded(&claim)?]),
            Err(WorldError::Malformed(Defect::SlotTable))
        ),
        "a claim of {MAX_ENTITIES} slots with no rows and no free list"
    );
    assert_eq!(wide.hash()?, before, "the claim moved the world");

    // Every claimed slot is named, and the cell tables of that many slots do not fit the bytes ceiling.
    let named = 2_000_u32.saturating_add(u32::try_from(below(&mut donor.stream, 1_000)?)?);
    let ceiling = 1_usize << 16;
    let (mut tight, _cols) = world("acme", 5_000, ceiling)?;
    let before = tight.hash()?;
    let mut crowd = plain;
    crowd.slots = named;
    crowd.free = (0..named).collect();
    crowd.rows.clear();
    let table = usize::try_from(named)?
        .saturating_mul(3)
        .saturating_mul(CELL_COST);
    assert!(
        matches!(
            tight.restore(&[encoded(&crowd)?]),
            Err(WorldError::Ceiling { what: Limit::Bytes, limit, asked })
                if limit == ceiling && asked == table
        ),
        "{named} named slots under a ceiling of {ceiling} bytes"
    );
    assert_eq!(tight.hash()?, before, "the crowd moved the world");
    Ok(())
}
