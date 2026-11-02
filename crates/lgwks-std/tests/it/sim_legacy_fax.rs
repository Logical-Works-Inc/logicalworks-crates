//! Seeded round-trip and hostile simulations of the fax codec.
//!
//! One seed draws every bitmap, a failure names the seed that produced it,
//! and the same seed must produce the same trace hash (TIFF 6.0 Part 1
//! baseline tags; ITU-T T.4 §4 1D runs for G3; ITU-T T.6 §2 2D references
//! for G4; corrupt strips name the page and row).

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::fax::{Compression, TiffImage, decode_tiff, encode_tiff};
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, fold_bytes, next_byte};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, assert_verdict_replays,
    initial_trace,
};

/// The bounds every sweep here runs under.
const BOUNDS: Limits = Limits::new(8_388_608, 1024, 4096, 8, 33_554_432);

/// Draws a seeded bitmap: random bytes, stripes and solids mixed.
fn draw_bitmap(state: &mut Seeded, width: usize, height: usize) -> Vec<u8> {
    let row_bytes = width.div_ceil(8);
    let mut out = vec![0_u8; height.saturating_mul(row_bytes)];
    let pattern = below(state, 4);
    for (index, slot) in out.iter_mut().enumerate() {
        let byte = next_byte(state);
        *slot = match pattern {
            0 => byte,
            1 => {
                if index.rem_euclid(2) == 0 {
                    0xff
                } else {
                    0x00
                }
            }
            2 => 0xff,
            _ => byte & 0xf0,
        };
    }
    out
}

/// Runs the seeded round-trip sweep and returns its trace.
fn fax_trace(seed: u64, compression: Compression) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..12 {
        let width = below(&mut state, 248).saturating_add(8);
        let height = below(&mut state, 12).saturating_add(1);
        let bitmap = draw_bitmap(&mut state, width, height);
        let Ok(width_narrow) = u32::try_from(width) else {
            continue;
        };
        let Ok(height_narrow) = u32::try_from(height) else {
            continue;
        };
        let Ok(image) = TiffImage::from_bitmap(width_narrow, height_narrow, &bitmap) else {
            continue;
        };
        let coded = encode_tiff(core::slice::from_ref(&image), compression, &BOUNDS);
        let back = coded
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|bytes| decode_tiff(bytes, &BOUNDS));
        assert_eq!(
            back.as_ref()
                .map(|pages| pages.iter().map(TiffImage::bitmap).collect::<Vec<_>>()),
            Ok(vec![image.bitmap()]),
            "seed {seed}: a {width}x{height} {compression:?} page must return its canonical bitmap"
        );
        if let Ok(bytes) = coded {
            fold_bytes(&mut trace, &bytes);
        }
    }
    trace
}

/// The raw round-trip trace.
fn fax_raw_trace(seed: u64) -> u64 {
    fax_trace(seed, Compression::Raw)
}

/// The G3 round-trip trace.
fn fax_g3_trace(seed: u64) -> u64 {
    fax_trace(seed, Compression::G3)
}

/// The G4 round-trip trace.
fn fax_g4_trace(seed: u64) -> u64 {
    fax_trace(seed, Compression::G4)
}

#[test]
/// A seeded bitmap survives raw, G3 and G4 round trips.
fn a_seeded_bitmap_round_trips_every_compression() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let bitmap = draw_bitmap(&mut state, 64, 8);
        let Ok(image) = TiffImage::from_bitmap(64, 8, &bitmap) else {
            continue;
        };
        assert_eq!(image.width(), 64, "seed {seed}: the width must read back");
        assert_eq!(image.height(), 8, "seed {seed}: the height must read back");
        assert_eq!(
            image.row_bytes(),
            8,
            "seed {seed}: eight bytes must pack one row"
        );
        assert_eq!(
            image.row(0),
            bitmap.get(..8),
            "seed {seed}: the first row must read back"
        );
        assert_eq!(
            image.row(8),
            None,
            "seed {seed}: a row past the height must be absent"
        );
        for compression in [Compression::Raw, Compression::G3, Compression::G4] {
            let back = encode_tiff(core::slice::from_ref(&image), compression, &BOUNDS)
                .and_then(|coded| decode_tiff(&coded, &BOUNDS));
            assert_eq!(
                back.as_ref()
                    .map(|pages| pages.iter().map(TiffImage::bitmap).collect::<Vec<_>>()),
                Ok(vec![bitmap.as_slice()]),
                "seed {seed}: {compression:?} must return the one page bit for bit"
            );
        }
    }
}

#[test]
/// Hostile strips never panic: flips and cuts answer typed errors or
/// still-valid images, and a zeroed file is refused.
fn hostile_strips_answer_typed_errors() {
    assert!(
        decode_tiff(&[0_u8; 64], &BOUNDS).is_err(),
        "a zeroed file must be refused"
    );
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let bitmap = draw_bitmap(&mut state, 64, 8);
        let Ok(image) = TiffImage::from_bitmap(64, 8, &bitmap) else {
            continue;
        };
        let Ok(coded) = encode_tiff(&[image], Compression::G3, &BOUNDS) else {
            continue;
        };
        let mut hostile = coded.clone();
        let position = below(&mut state, hostile.len().max(1));
        if let Some(slot) = hostile.get_mut(position) {
            *slot = slot.wrapping_add(1);
        }
        assert_verdict_replays(seed, "flipped fax", || decode_tiff(&hostile, &BOUNDS));
        let cut = below(&mut state, coded.len());
        if let Some(prefix) = coded.get(..cut) {
            assert!(
                decode_tiff(prefix, &BOUNDS).is_err(),
                "seed {seed}: a fax cut to {cut} of {} bytes must be refused",
                coded.len()
            );
        }
    }
}

#[test]
/// The replay oracle: one seed, one trace, per compression.
fn the_same_seed_replays_to_the_same_fax_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(fax_raw_trace, seed);
        assert_same_seed_replays(fax_g3_trace, seed);
        assert_same_seed_replays(fax_g4_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_fax_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(fax_raw_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
    assert_distinct_seeds_diverge(fax_g3_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
    assert_distinct_seeds_diverge(fax_g4_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
