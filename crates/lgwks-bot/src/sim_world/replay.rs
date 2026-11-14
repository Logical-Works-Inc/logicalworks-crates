//! Replay: a world built only from the logged change sets equals the live one, and bad logs are refused (#1002).

use super::{Driver, Failure, below, world};
use crate::world::MAX_WIRE_BYTES;
use crate::world::error::{Limit, WorldError};

/// Catches a commit whose bytes do not carry the whole transaction (the liar upstream is the log): a second
/// world fed only the logged change sets has the live world's hash at every checkpoint and at the end, and a
/// change set replayed twice, or out of order, is a typed `Stale` that moves nothing (#1002).
#[test]
fn sim_world_replay_equals_live() -> Result<(), Failure> {
    let mut live = Driver::new(0x1002_0e1a, "acme", 50_000, 1 << 24)?;
    let (mut twin, _cols) = world("acme", 50_000, 1 << 24)?;
    for step in 0..600_usize {
        live.step()?;
        let bytes = live
            .log
            .last()
            .ok_or(Failure::Check("a step logs a change set"))?;
        twin.replay(bytes)?;
        if step.is_multiple_of(25) {
            assert_eq!(
                twin.hash()?,
                live.world.hash()?,
                "the worlds differ after step {step}"
            );
        }
    }
    assert_eq!(
        twin.hash()?,
        live.world.hash()?,
        "the worlds differ at the end"
    );
    assert_eq!(twin.tick(), live.world.tick(), "the ticks differ");
    let first = live
        .log
        .first()
        .ok_or(Failure::Check("the log has a first entry"))?;
    let last = live
        .log
        .last()
        .ok_or(Failure::Check("the log has a last entry"))?;
    let before = twin.hash()?;
    assert!(
        matches!(twin.replay(first), Err(WorldError::Stale { .. })),
        "a change set replayed twice is refused"
    );
    let (mut fresh, _cols) = world("acme", 50_000, 1 << 24)?;
    assert!(
        matches!(fresh.replay(last), Err(WorldError::Stale { now: 0, .. })),
        "a change set with a gap before it is refused"
    );
    assert_eq!(twin.hash()?, before, "a refused replay moved the world");
    Ok(())
}

/// Catches a reader that trusts the log (the weird-bytes fool): every truncation of a change set is refused,
/// seeded bit flips are refused or leave a world whose totals match a recount, an oversize input is refused
/// before it is read, and a refusal leaves the world as it was (#1002).
#[test]
fn sim_world_hostile_bytes_refused() -> Result<(), Failure> {
    let mut live = Driver::new(0x1002_b17e, "acme", 50_000, 1 << 24)?;
    let (mut twin, _cols) = world("acme", 50_000, 1 << 24)?;
    for _ in 0..80 {
        live.step()?;
    }
    let (last, earlier) = live
        .log
        .split_last()
        .ok_or(Failure::Check("the log has entries"))?;
    for bytes in earlier {
        twin.replay(bytes)?;
    }
    let before = twin.hash()?;
    for length in 0..last.len() {
        let mut attempt = twin.clone();
        let cut = last
            .get(..length)
            .ok_or(Failure::Check("a prefix exists"))?;
        assert!(
            attempt.replay(cut).is_err(),
            "a prefix of {length} bytes was read"
        );
        assert_eq!(attempt.hash()?, before, "a refused prefix moved the world");
    }
    let mut accepted = 0_usize;
    for _ in 0..512 {
        let at = below(&mut live.stream, last.len())?;
        let bit = below(&mut live.stream, 8)?;
        let mut flipped = last.clone();
        if let Some(byte) = flipped.get_mut(at) {
            *byte ^= 1_u8.rotate_left(u32::try_from(bit)?);
        }
        let mut attempt = twin.clone();
        match attempt.replay(&flipped) {
            Err(_refusal) => assert_eq!(attempt.hash()?, before, "a refusal moved the world"),
            Ok(()) => {
                accepted = accepted.saturating_add(1);
                let (alive, bytes) = attempt.recount();
                assert_eq!(
                    (alive, bytes),
                    (attempt.live(), attempt.bytes()),
                    "a flipped log left inconsistent totals"
                );
            }
        }
    }
    assert!(
        accepted < 512,
        "some flips were refused (accepted {accepted})"
    );
    let oversize = vec![0_u8; MAX_WIRE_BYTES.saturating_add(1)];
    assert!(
        matches!(
            twin.replay(&oversize),
            Err(WorldError::Ceiling {
                what: Limit::WireBytes,
                ..
            })
        ),
        "an oversize change set is refused before it is read"
    );
    assert_eq!(twin.hash()?, before, "the hostile inputs moved the world");
    Ok(())
}
