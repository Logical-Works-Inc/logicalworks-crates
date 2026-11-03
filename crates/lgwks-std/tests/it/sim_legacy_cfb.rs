//! Seeded round-trip and hostile simulations of the compound-file codec.
//!
//! One seed draws every stream, a failure names the seed that produced it,
//! and the same seed must produce the same trace hash (MS-CFB §2: magic,
//! version 3 or 4, FAT chains with cycle detection, the directory and
//! streams; MS-OVBA §2.4.1: the VBA container signature, chunk sizes and
//! copy tokens; hostile bytes answer typed errors, never a panic).

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::cfb::{CfbError, CfbFile, build_image, vba};
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, next_bytes};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, assert_verdict_replays,
    fold_usize, initial_trace,
};

/// The bounds every sweep here runs under.
const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

/// The stream-name alphabet the sweeps draw from.
const NAME_BYTES: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

/// A seeded stream name of 1-16 drawn bytes.
fn draw_name(state: &mut Seeded) -> String {
    let mut out = String::new();
    for _ in 0..below(state, 16).saturating_add(1) {
        let Some(&byte) = NAME_BYTES.get(below(state, NAME_BYTES.len())) else {
            continue;
        };
        out.push(char::from(byte));
    }
    if out.is_empty() {
        out.push('S');
    }
    out
}

/// Runs the seeded round-trip sweep and returns its trace.
fn cfb_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..16 {
        let first = draw_name(&mut state);
        let second = draw_name(&mut state);
        let first_data_len = below(&mut state, 2048);
        let first_data = next_bytes(&mut state, first_data_len);
        let second_data_len = below(&mut state, 512);
        let second_data = next_bytes(&mut state, second_data_len);
        let streams = vec![
            (first.as_str(), first_data.as_slice()),
            (second.as_str(), second_data.as_slice()),
        ];
        let built = build_image(&streams, &BOUNDS);
        if first.eq_ignore_ascii_case(&second) {
            assert!(
                matches!(built, Err(CfbError::BadName { .. })),
                "seed {seed}: sibling names equal once uppercased must be refused"
            );
            fold_usize(&mut trace, first.len());
            continue;
        }
        let read = built
            .and_then(|image| CfbFile::open(&image, &BOUNDS).map(|file| (image.len(), file)))
            .and_then(|(len, file)| {
                Ok((
                    len,
                    file.read_stream(&first, &BOUNDS)?,
                    file.read_stream(&second, &BOUNDS)?,
                ))
            });
        assert!(
            read.is_ok(),
            "seed {seed}: two legal streams must build, open and read: {read:?}"
        );
        let Ok((image_len, first_back, second_back)) = read else {
            continue;
        };
        assert_eq!(
            first_back, first_data,
            "seed {seed}: the first stream must read back"
        );
        assert_eq!(
            second_back, second_data,
            "seed {seed}: the second stream must read back"
        );
        fold_usize(&mut trace, image_len);
    }
    trace
}

#[test]
/// A seeded image opens and every stream reads back byte for byte.
fn a_seeded_image_round_trips() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..8 {
            let name = draw_name(&mut state);
            let data_len = below(&mut state, 1024);
            let data = next_bytes(&mut state, data_len);
            let streams = vec![(name.as_str(), data.as_slice())];
            let Ok(image) = build_image(&streams, &BOUNDS) else {
                continue;
            };
            let Ok(file) = CfbFile::open(&image, &BOUNDS) else {
                continue;
            };
            assert_eq!(
                file.read_stream(&name, &BOUNDS).as_deref(),
                Ok(data.as_slice()),
                "seed {seed}: the stream must read back"
            );
        }
    }
}

#[test]
/// Hostile images never hang or panic: flips, cuts and tight bounds answer
/// typed errors, and a zeroed header is a bad magic.
fn hostile_images_answer_typed_errors() {
    let tight = Limits::new(512, 1, 1, 1, 64);
    assert!(
        CfbFile::open(&[0_u8; 512], &BOUNDS).is_err(),
        "a zeroed header must be refused"
    );
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let name = draw_name(&mut state);
            let data_len = below(&mut state, 512).saturating_add(1);
            let data = next_bytes(&mut state, data_len);
            let streams = vec![(name.as_str(), data.as_slice())];
            let Ok(image) = build_image(&streams, &BOUNDS) else {
                continue;
            };
            let mut hostile = image.clone();
            let position = below(&mut state, hostile.len().max(1));
            if let Some(slot) = hostile.get_mut(position) {
                *slot = slot.wrapping_add(1);
            }
            assert_verdict_replays(seed, "flipped compound file", || {
                CfbFile::open(&hostile, &BOUNDS).and_then(|file| file.read_stream(&name, &BOUNDS))
            });
            let cut = below(&mut state, image.len());
            if let Some(prefix) = image.get(..cut) {
                assert!(
                    CfbFile::open(prefix, &BOUNDS)
                        .and_then(|file| file.read_stream(&name, &BOUNDS))
                        .is_err(),
                    "seed {seed}: a stream cut to {cut} of {} bytes must not read back",
                    image.len()
                );
            }
            assert!(
                CfbFile::open(&image, &tight).is_err() || data.is_empty(),
                "seed {seed}: a tiny ceiling must refuse a real image"
            );
        }
    }
}

#[test]
/// VBA source compresses and decompresses back to itself, and truncations
/// answer typed errors.
fn vba_source_round_trips_and_refuses_truncation() -> Result<(), vba::VbaError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let source_len = below(&mut state, 2048).saturating_add(1);
            let source = next_bytes(&mut state, source_len);
            let coded = vba::compress(&source, &BOUNDS)?;
            assert_eq!(
                vba::decompress(&coded, &BOUNDS)?,
                source,
                "seed {seed}: VBA source must round-trip"
            );
            let cut = below(&mut state, coded.len());
            if let Some(prefix) = coded.get(..cut)
                && cut > 1
            {
                assert!(
                    vba::decompress(prefix, &BOUNDS).is_err(),
                    "seed {seed}: a container cut to {cut} of {} bytes must be refused",
                    coded.len()
                );
            }
        }
    }
    Ok(())
}

#[test]
/// The replay oracle: one seed, one trace.
fn the_same_seed_replays_to_the_same_cfb_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(cfb_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_cfb_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(cfb_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
