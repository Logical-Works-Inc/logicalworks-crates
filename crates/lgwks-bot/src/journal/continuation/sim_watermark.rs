//! Seeded simulation of the continuation watermark across the whole `u64` range.
//!
//! Source: #143's settlement headroom ("one fifth of each ceiling is left so an attempt already
//! handed to the outside world can always record its outcome"), and the 2026-11-19 defect this
//! file pins: `watermark_of` computed `limit * 8 / 10` with a saturating multiply, so every
//! ceiling above `u64::MAX / 8` got a watermark near a tenth of itself, not eight tenths. The
//! oracle is the same quotient taken in `u128`, where nothing overflows. The fools are the monkey
//! drawing every bit width, the giant and the empty ceiling, and pairs drawn out of order.

#[cfg(test)]
use std::num::TryFromIntError;

use lgwks_std::seeded::{DrawError, Seeded};

use super::watermark::{SHIPPED_BYTE_CEILING, SHIPPED_EVENT_CEILING, watermark_of};
use super::{ContinuationPolicy, ContinuationWatermark};

/// Seeds each family sweeps.
const SEEDS: u64 = 64;

/// Draws per seed.
const DRAWS: u64 = 512;

/// How a family broke.
#[derive(Debug)]
enum SimFailed {
    /// The seeded stream refused a draw.
    Draw(DrawError),
    /// The wide model did not fit back into `u64`.
    Narrow(TryFromIntError),
    /// A property did not hold.
    Broke {
        /// The ceiling the property was checked at.
        limit: u64,
        /// Which property broke.
        property: &'static str,
    },
}

impl std::fmt::Display for SimFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Draw(ref error) => write!(f, "draw refused: {error}"),
            Self::Narrow(ref error) => write!(f, "model did not narrow: {error}"),
            Self::Broke { limit, property } => write!(f, "at ceiling {limit}: {property}"),
        }
    }
}

impl std::error::Error for SimFailed {}

impl From<DrawError> for SimFailed {
    fn from(error: DrawError) -> Self {
        Self::Draw(error)
    }
}

impl From<TryFromIntError> for SimFailed {
    fn from(error: TryFromIntError) -> Self {
        Self::Narrow(error)
    }
}

/// The watermark the spec names, `floor(limit * 8 / 10)`, taken where it cannot overflow.
fn model(limit: u64) -> Result<u64, SimFailed> {
    Ok(u64::try_from(
        u128::from(limit).saturating_mul(8).div_euclid(10),
    )?)
}

/// A ceiling of every bit width: a full word shifted right by a drawn amount below 64.
fn ceiling(stream: &mut Seeded) -> Result<u64, SimFailed> {
    let shift = u32::try_from(stream.below(64)?)?;
    Ok(stream.next_u64().wrapping_shr(shift))
}

/// Refuses with `property` unless `holds`.
fn require(holds: bool, limit: u64, property: &'static str) -> Result<(), SimFailed> {
    if holds {
        Ok(())
    } else {
        Err(SimFailed::Broke { limit, property })
    }
}

#[test]
fn sim_watermark_monkey_every_bit_width_matches_the_wide_model() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..DRAWS {
            let limit = ceiling(&mut stream)?;
            require(
                watermark_of(limit) == model(limit)?,
                limit,
                "the watermark is 8/10",
            )?;
        }
    }
    Ok(())
}

#[test]
fn sim_watermark_giant_and_empty_ceilings_match_the_wide_model() -> Result<(), SimFailed> {
    let eighth = u64::MAX.div_euclid(8);
    for limit in [
        0,
        1,
        9,
        10,
        11,
        eighth.saturating_sub(1),
        eighth,
        eighth.saturating_add(1),
        u64::MAX.saturating_sub(10),
        u64::MAX.saturating_sub(1),
        u64::MAX,
    ] {
        require(
            watermark_of(limit) == model(limit)?,
            limit,
            "a boundary is 8/10",
        )?;
    }
    require(
        watermark_of(u64::MAX) == 14_757_395_258_967_641_292,
        u64::MAX,
        "the largest ceiling keeps its exact watermark",
    )
}

#[test]
fn sim_watermark_never_falls_as_the_ceiling_rises() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_add(0x5EED_0000));
        for _ in 0..DRAWS {
            let (first, second) = (ceiling(&mut stream)?, ceiling(&mut stream)?);
            let (low, high) = (first.min(second), first.max(second));
            require(
                watermark_of(low) <= watermark_of(high),
                high,
                "the watermark is monotone",
            )?;
        }
    }
    Ok(())
}

#[test]
fn sim_watermark_always_leaves_a_fifth_for_settlement() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_add(0xF1F7_0000));
        for _ in 0..DRAWS {
            let limit = ceiling(&mut stream)?.max(1);
            let mark = watermark_of(limit);
            require(
                mark < limit,
                limit,
                "a continuation is due before the ceiling",
            )?;
            let headroom = limit.saturating_sub(mark);
            require(
                headroom >= limit.div_euclid(5),
                limit,
                "a fifth is left to settle",
            )?;
        }
    }
    Ok(())
}

#[test]
fn sim_watermark_whole_tenths_and_their_remainder_add() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_add(0x7E57_0000));
        for _ in 0..DRAWS {
            let tenths = ceiling(&mut stream)?.div_euclid(10);
            let rest = stream.below(10)?;
            let limit = tenths.saturating_mul(10).saturating_add(rest);
            let split = tenths.saturating_mul(8).saturating_add(watermark_of(rest));
            require(
                watermark_of(limit) == split,
                limit,
                "tenths and remainder add",
            )?;
        }
    }
    Ok(())
}

#[test]
fn sim_watermark_policies_either_side_of_each_ceiling() -> Result<(), SimFailed> {
    let declared = ContinuationPolicy::declared();
    require(
        (declared.events_at(), declared.bytes_at())
            == (model(SHIPPED_EVENT_CEILING)?, model(SHIPPED_BYTE_CEILING)?),
        SHIPPED_EVENT_CEILING,
        "the shipped policy is 8/10 of each shipped ceiling",
    )?;
    let mut stream = Seeded::from_seed(0xCE11_0000);
    for _ in 0..DRAWS {
        let events = stream.below(SHIPPED_EVENT_CEILING.saturating_mul(2))?;
        let bytes = stream.below(SHIPPED_BYTE_CEILING.saturating_mul(2))?;
        let admitted = events < SHIPPED_EVENT_CEILING && bytes < SHIPPED_BYTE_CEILING;
        let built = ContinuationPolicy::new(events, bytes);
        require(
            built.is_ok() == admitted,
            events,
            "a policy is admitted only below both",
        )?;
        let inert = ContinuationWatermark::inert(events, SHIPPED_EVENT_CEILING, bytes, 0);
        require(
            !inert.is_due(),
            events,
            "a journal that does not continue is never due",
        )?;
    }
    Ok(())
}
