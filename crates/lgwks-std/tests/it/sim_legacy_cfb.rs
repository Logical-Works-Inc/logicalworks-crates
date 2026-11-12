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

/// The FAT marker that ends a chain (MS-CFB §2.3.1).
const ENDOFCHAIN: u32 = 0xffff_fffe;

/// The little-endian `u32` at `at`, if the image holds it.
fn u32_at(image: &[u8], at: usize) -> Option<u32> {
    let bytes = image.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// Overwrites the little-endian `u32` at `at`, answering whether it fit.
fn set_u32(image: &mut [u8], at: usize, value: u32) -> bool {
    let Some(end) = at.checked_add(4) else {
        return false;
    };
    image
        .get_mut(at..end)
        .map(|slot| slot.copy_from_slice(&value.to_le_bytes()))
        .is_some()
}

/// Where an image's layout puts things, read from its header (MS-CFB §2.2):
/// the sector length, the first directory sector and the first FAT sector.
struct Layout {
    /// Bytes per sector: `1 << sector shift`.
    sector_len: usize,
    /// The first directory sector.
    directory: u32,
    /// The first FAT sector, the head of the DIFAT array.
    fat: u32,
}

impl Layout {
    /// Reads the three fields from a header.
    fn of(image: &[u8]) -> Option<Self> {
        let shift = u32::from(u16::from_le_bytes(image.get(0x1e..0x20)?.try_into().ok()?));
        Some(Self {
            sector_len: 1_usize.checked_shl(shift)?,
            directory: u32_at(image, 0x30)?,
            fat: u32_at(image, 0x4c)?,
        })
    }

    /// The byte offset of `sector`: the 512-byte header, then sectors.
    fn sector(&self, sector: u32) -> Option<usize> {
        usize::try_from(sector)
            .ok()?
            .checked_mul(self.sector_len)?
            .checked_add(512)
    }

    /// The byte offset of `sector`'s FAT entry, in the first FAT sector.
    fn fat_entry(&self, sector: u32) -> Option<usize> {
        self.sector(self.fat)?
            .checked_add(usize::try_from(sector).ok()?.checked_mul(4)?)
    }

    /// The chain from `start` to its end marker, read from the first FAT
    /// sector, stopping at `limit` links.
    fn chain(&self, image: &[u8], start: u32, limit: usize) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut at = start;
        while at != ENDOFCHAIN && chain.len() < limit {
            chain.push(at);
            let Some(next) = self.fat_entry(at).and_then(|entry| u32_at(image, entry)) else {
                break;
            };
            at = next;
        }
        chain
    }
}

/// The start sector of the directory entry named `name`: the directory
/// holds the root, then the streams, and an entry keeps its start at 0x74.
fn start_of(image: &[u8], layout: &Layout, name: &str) -> Option<u32> {
    let directory = layout.sector(layout.directory)?;
    let wanted: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    (1..=2_usize).find_map(|index| {
        let entry = directory.checked_add(index.checked_mul(128)?)?;
        let held = image.get(entry..entry.checked_add(wanted.len())?)?;
        if held == wanted.as_slice() {
            u32_at(image, entry.checked_add(0x74)?)
        } else {
            None
        }
    })
}

#[test]
/// A stream chain whose last link returns to any earlier sector is refused
/// as a cycle naming a sector on it, never a page-ceiling overrun or a
/// hang, and the other stream of the same image still reads (MS-CFB §2.3:
/// a reader must detect a cyclic FAT chain). The seed draws the chain
/// length and the sector the cycle returns to, so a walk that parked its
/// marker only at the start would miss every cycle that skips the head.
fn sim_a_cyclic_stream_chain_is_refused_naming_a_sector_on_it() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..12 {
            let sectors = below(&mut state, 64).saturating_add(1);
            let data = next_bytes(&mut state, sectors.saturating_mul(512));
            let other = next_bytes(&mut state, 600);
            let streams = vec![("Cycled", data.as_slice()), ("Kept", other.as_slice())];
            let built = build_image(&streams, &BOUNDS);
            assert!(
                built.is_ok(),
                "seed {seed}: two legal streams must build: {built:?}"
            );
            let Ok(mut image) = built else { continue };
            let layout = Layout::of(&image);
            assert!(layout.is_some(), "seed {seed}: a built header must read");
            let Some(layout) = layout else { continue };
            let start = start_of(&image, &layout, "Cycled");
            assert!(
                start.is_some(),
                "seed {seed}: the Cycled entry must be found"
            );
            let Some(start) = start else { continue };
            let chain = layout.chain(&image, start, 4096);
            assert_eq!(
                chain.len(),
                sectors,
                "seed {seed}: the chain must span the stream"
            );
            let back_to = below(&mut state, chain.len());
            let (Some(&last), Some(on_cycle)) = (chain.last(), chain.get(back_to..)) else {
                continue;
            };
            let Some(&target) = on_cycle.first() else {
                continue;
            };
            let patched = layout
                .fat_entry(last)
                .is_some_and(|entry| set_u32(&mut image, entry, target));
            assert!(patched, "seed {seed}: the last link must be writable");
            let file = CfbFile::open(&image, &BOUNDS);
            assert!(
                file.is_ok(),
                "seed {seed}: the directory chain is untouched: {file:?}"
            );
            let Ok(file) = file else { continue };
            let read = file.read_stream("Cycled", &BOUNDS);
            assert!(
                matches!(read, Err(CfbError::FatCycle { sector }) if on_cycle.contains(&sector)),
                "seed {seed}: a {sectors}-sector chain cycling back to link {back_to} must be refused naming a sector of {on_cycle:?}, got {read:?}"
            );
            assert_eq!(
                file.read_stream("Kept", &BOUNDS).as_deref(),
                Ok(other.as_slice()),
                "seed {seed}: the other stream must still read"
            );
        }
    }
}

#[test]
/// A directory sector whose FAT entry names itself is the shortest cycle,
/// and opening refuses it naming that sector.
fn a_self_linked_directory_sector_is_a_cycle_on_open() {
    let built = build_image(&[("Only", b"payload".as_slice())], &BOUNDS);
    assert!(built.is_ok(), "one legal stream must build: {built:?}");
    let Ok(mut image) = built else { return };
    let layout = Layout::of(&image);
    assert!(layout.is_some(), "a built header must read");
    let Some(layout) = layout else { return };
    let directory = layout.directory;
    let patched = layout
        .fat_entry(directory)
        .is_some_and(|entry| set_u32(&mut image, entry, directory));
    assert!(patched, "the directory's FAT entry must be writable");
    assert_eq!(
        CfbFile::open(&image, &BOUNDS).err(),
        Some(CfbError::FatCycle { sector: directory }),
        "a self-linked directory sector must be refused as a cycle on itself"
    );
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
