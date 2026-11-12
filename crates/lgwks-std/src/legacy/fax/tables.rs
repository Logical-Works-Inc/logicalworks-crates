//! T.4 run tables and T.6 mode codes.
//!
//! Every entry is `(bits, length, run)`: the code right-aligned in `bits`,
//! its bit `length`, and the run it stands for. Transcribed from ITU-T T.4
//! Table 2 (terminating codes), Table 3a (make-up codes) and Table 3b (the
//! extended make-up codes both colours share), and ITU-T T.6 Table 1 (mode
//! codes). The `fax` unit tests prove each colour's code set prefix-free and
//! decode known-answer streams worked by hand from those tables.
//!
//! The decoders read through lookups built once from these same entries:
//! one slot per value of the next 13 bits for each colour's runs and per
//! value of the next 7 bits for the modes, so a code resolves in one index
//! instead of a scan per bit, and the tables stay the one source.

use std::sync::LazyLock;

/// The T.4 end-of-line code: eleven zeros and a one (ITU-T T.4 §4.1.2).
pub(crate) const EOL_BITS: u16 = 0x0001;

/// The T.4 end-of-line code length.
pub(crate) const EOL_LEN: u8 = 12;

/// The T.6 pass mode code `0001` (ITU-T T.6 Table 1).
pub(crate) const MODE_PASS: (u16, u8) = (0x0001, 4);

/// The T.6 horizontal mode code `001`.
pub(crate) const MODE_HORIZ: (u16, u8) = (0x0001, 3);

/// The T.6 vertical mode codes, indexed by offset plus three: `VL3` `0000010`
/// through `V0` `1` to `VR3` `0000011`.
pub(crate) const MODE_VERTICAL: [(u16, u8); 7] = [
    (0x02, 7),
    (0x02, 6),
    (0x02, 3),
    (0x01, 1),
    (0x03, 3),
    (0x03, 6),
    (0x03, 7),
];

/// White terminating codes for runs 0-63 (ITU-T T.4 Table 2).
pub(crate) const WHITE_TERM: [(u16, u8, u16); 64] = [
    (0x35, 8, 0),
    (0x07, 6, 1),
    (0x07, 4, 2),
    (0x08, 4, 3),
    (0x0b, 4, 4),
    (0x0c, 4, 5),
    (0x0e, 4, 6),
    (0x0f, 4, 7),
    (0x13, 5, 8),
    (0x14, 5, 9),
    (0x07, 5, 10),
    (0x08, 5, 11),
    (0x08, 6, 12),
    (0x03, 6, 13),
    (0x34, 6, 14),
    (0x35, 6, 15),
    (0x2a, 6, 16),
    (0x2b, 6, 17),
    (0x27, 7, 18),
    (0x0c, 7, 19),
    (0x08, 7, 20),
    (0x17, 7, 21),
    (0x03, 7, 22),
    (0x04, 7, 23),
    (0x28, 7, 24),
    (0x2b, 7, 25),
    (0x13, 7, 26),
    (0x24, 7, 27),
    (0x18, 7, 28),
    (0x02, 8, 29),
    (0x03, 8, 30),
    (0x1a, 8, 31),
    (0x1b, 8, 32),
    (0x12, 8, 33),
    (0x13, 8, 34),
    (0x14, 8, 35),
    (0x15, 8, 36),
    (0x16, 8, 37),
    (0x17, 8, 38),
    (0x28, 8, 39),
    (0x29, 8, 40),
    (0x2a, 8, 41),
    (0x2b, 8, 42),
    (0x2c, 8, 43),
    (0x2d, 8, 44),
    (0x04, 8, 45),
    (0x05, 8, 46),
    (0x0a, 8, 47),
    (0x0b, 8, 48),
    (0x52, 8, 49),
    (0x53, 8, 50),
    (0x54, 8, 51),
    (0x55, 8, 52),
    (0x24, 8, 53),
    (0x25, 8, 54),
    (0x58, 8, 55),
    (0x59, 8, 56),
    (0x5a, 8, 57),
    (0x5b, 8, 58),
    (0x4a, 8, 59),
    (0x4b, 8, 60),
    (0x32, 8, 61),
    (0x33, 8, 62),
    (0x34, 8, 63),
];

/// White make-up codes for runs 64-1728 (ITU-T T.4 Table 3a).
pub(crate) const WHITE_MAKEUP: [(u16, u8, u16); 27] = [
    (0x1b, 5, 64),
    (0x12, 5, 128),
    (0x17, 6, 192),
    (0x37, 7, 256),
    (0x36, 8, 320),
    (0x37, 8, 384),
    (0x64, 8, 448),
    (0x65, 8, 512),
    (0x68, 8, 576),
    (0x67, 8, 640),
    (0xcc, 9, 704),
    (0xcd, 9, 768),
    (0xd2, 9, 832),
    (0xd3, 9, 896),
    (0xd4, 9, 960),
    (0xd5, 9, 1024),
    (0xd6, 9, 1088),
    (0xd7, 9, 1152),
    (0xd8, 9, 1216),
    (0xd9, 9, 1280),
    (0xda, 9, 1344),
    (0xdb, 9, 1408),
    (0x98, 9, 1472),
    (0x99, 9, 1536),
    (0x9a, 9, 1600),
    (0x18, 6, 1664),
    (0x9b, 9, 1728),
];

/// Black terminating codes for runs 0-63 (ITU-T T.4 Table 2).
pub(crate) const BLACK_TERM: [(u16, u8, u16); 64] = [
    (0x37, 10, 0),
    (0x02, 3, 1),
    (0x03, 2, 2),
    (0x02, 2, 3),
    (0x03, 3, 4),
    (0x03, 4, 5),
    (0x02, 4, 6),
    (0x03, 5, 7),
    (0x05, 6, 8),
    (0x04, 6, 9),
    (0x04, 7, 10),
    (0x05, 7, 11),
    (0x07, 7, 12),
    (0x04, 8, 13),
    (0x07, 8, 14),
    (0x18, 9, 15),
    (0x17, 10, 16),
    (0x18, 10, 17),
    (0x08, 10, 18),
    (0x67, 11, 19),
    (0x68, 11, 20),
    (0x6c, 11, 21),
    (0x37, 11, 22),
    (0x28, 11, 23),
    (0x17, 11, 24),
    (0x18, 11, 25),
    (0xca, 12, 26),
    (0xcb, 12, 27),
    (0xcc, 12, 28),
    (0xcd, 12, 29),
    (0x68, 12, 30),
    (0x69, 12, 31),
    (0x6a, 12, 32),
    (0x6b, 12, 33),
    (0xd2, 12, 34),
    (0xd3, 12, 35),
    (0xd4, 12, 36),
    (0xd5, 12, 37),
    (0xd6, 12, 38),
    (0xd7, 12, 39),
    (0x6c, 12, 40),
    (0x6d, 12, 41),
    (0xda, 12, 42),
    (0xdb, 12, 43),
    (0x54, 12, 44),
    (0x55, 12, 45),
    (0x56, 12, 46),
    (0x57, 12, 47),
    (0x64, 12, 48),
    (0x65, 12, 49),
    (0x52, 12, 50),
    (0x53, 12, 51),
    (0x24, 12, 52),
    (0x37, 12, 53),
    (0x38, 12, 54),
    (0x27, 12, 55),
    (0x28, 12, 56),
    (0x58, 12, 57),
    (0x59, 12, 58),
    (0x2b, 12, 59),
    (0x2c, 12, 60),
    (0x5a, 12, 61),
    (0x66, 12, 62),
    (0x67, 12, 63),
];

/// Black make-up codes for runs 64-1728 (ITU-T T.4 Table 3a).
pub(crate) const BLACK_MAKEUP: [(u16, u8, u16); 27] = [
    (0x0f, 10, 64),
    (0xc8, 12, 128),
    (0xc9, 12, 192),
    (0x5b, 12, 256),
    (0x33, 12, 320),
    (0x34, 12, 384),
    (0x35, 12, 448),
    (0x6c, 13, 512),
    (0x6d, 13, 576),
    (0x4a, 13, 640),
    (0x4b, 13, 704),
    (0x4c, 13, 768),
    (0x4d, 13, 832),
    (0x72, 13, 896),
    (0x73, 13, 960),
    (0x74, 13, 1024),
    (0x75, 13, 1088),
    (0x76, 13, 1152),
    (0x77, 13, 1216),
    (0x52, 13, 1280),
    (0x53, 13, 1344),
    (0x54, 13, 1408),
    (0x55, 13, 1472),
    (0x5a, 13, 1536),
    (0x5b, 13, 1600),
    (0x64, 13, 1664),
    (0x65, 13, 1728),
];

/// Extended make-up codes for runs 1792-2560, shared by both colours
/// (ITU-T T.4 Table 3b).
pub(crate) const EXTENDED_MAKEUP: [(u16, u8, u16); 13] = [
    (0x08, 11, 1792),
    (0x0c, 11, 1856),
    (0x0d, 11, 1920),
    (0x12, 12, 1984),
    (0x13, 12, 2048),
    (0x14, 12, 2112),
    (0x15, 12, 2176),
    (0x16, 12, 2240),
    (0x17, 12, 2304),
    (0x1c, 12, 2368),
    (0x1d, 12, 2432),
    (0x1e, 12, 2496),
    (0x1f, 12, 2560),
];

/// The longest run one make-up code carries; longer runs repeat it.
pub(crate) const MAX_MAKEUP: usize = 2560;

/// The longest code in any table, in bits: a longer prefix with no hit is
/// corrupt.
pub(crate) const MAX_CODE_LEN: u8 = 13;

/// One code table: `(bits, length, run)` entries.
pub(crate) type CodeTable = &'static [(u16, u8, u16)];

/// Bits one run lookup reads: the longest run code, so every code of a
/// colour resolves in one index.
pub(crate) const RUN_LOOKUP_BITS: u8 = MAX_CODE_LEN;

/// Slots in one run lookup: `2^RUN_LOOKUP_BITS`.
const RUN_LOOKUP_SLOTS: usize = 8192;

/// Bits one mode lookup reads: the longest T.6 mode code.
pub(crate) const MODE_LOOKUP_BITS: u8 = 7;

/// Slots in the mode lookup: `2^MODE_LOOKUP_BITS`.
const MODE_LOOKUP_SLOTS: usize = 128;

/// One lookup slot, packed in a `u16`: the code length in bits 0-3 (zero
/// for no code), a make-up flag in bit 4, and the value from bit 5: a
/// terminating run, a make-up run in units of 64, or a mode number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slot(u16);

impl Slot {
    /// Packs a code of `len` bits standing for `value`.
    fn new(len: u8, makeup: bool, value: u16) -> Self {
        Self(u16::from(len) | if makeup { 0x10 } else { 0 } | value.saturating_mul(32))
    }

    /// The code length in bits, or zero when no code starts with the bits
    /// that indexed this slot.
    pub(crate) const fn len(self) -> u8 {
        let [low, _] = self.0.to_le_bytes();
        low & 0x0f
    }

    /// Whether the code is a make-up code, whose run a terminating code
    /// must follow.
    pub(crate) const fn is_makeup(self) -> bool {
        self.0 & 0x10 != 0
    }

    /// The value: a run in pixels for a run code, or a mode number.
    pub(crate) fn value(self) -> usize {
        let raw = usize::from(self.0.saturating_div(32));
        if self.is_makeup() {
            raw.saturating_mul(64)
        } else {
            raw
        }
    }
}

/// Writes `slot` into every lookup index whose leading `len` bits are
/// `bits`: the code's prefix, then every tail the spare bits can hold.
fn fill(table: &mut [Slot], lookup_bits: u8, code: (u16, u8), slot: Slot) {
    let (bits, len) = code;
    let Some(spare) = lookup_bits.checked_sub(len) else {
        return;
    };
    let first = usize::from(bits).checked_shl(u32::from(spare));
    let count = 1_usize.checked_shl(u32::from(spare));
    if let (Some(first), Some(count)) = (first, count)
        && let Some(slots) = table.get_mut(first..first.saturating_add(count))
    {
        slots.fill(slot);
    }
}

/// One colour's run lookup: its terminating and make-up codes and the
/// shared extended make-ups, each colour's set being prefix-free.
fn run_lookup(terms: CodeTable, makeup: CodeTable) -> Vec<Slot> {
    let mut table = vec![Slot(0); RUN_LOOKUP_SLOTS];
    for &(bits, len, run) in terms {
        fill(
            &mut table,
            RUN_LOOKUP_BITS,
            (bits, len),
            Slot::new(len, false, run),
        );
    }
    for &(bits, len, run) in makeup.iter().chain(EXTENDED_MAKEUP.iter()) {
        let units = run.saturating_div(64);
        fill(
            &mut table,
            RUN_LOOKUP_BITS,
            (bits, len),
            Slot::new(len, true, units),
        );
    }
    table
}

/// The white run lookup, built once from the T.4 white tables.
static WHITE_LOOKUP: LazyLock<Vec<Slot>> = LazyLock::new(|| run_lookup(&WHITE_TERM, &WHITE_MAKEUP));

/// The black run lookup, built once from the T.4 black tables.
static BLACK_LOOKUP: LazyLock<Vec<Slot>> = LazyLock::new(|| run_lookup(&BLACK_TERM, &BLACK_MAKEUP));

/// The mode number of pass mode in [`mode_lookup`].
pub(crate) const MODE_NUMBER_PASS: usize = 1;

/// The mode number of horizontal mode in [`mode_lookup`].
pub(crate) const MODE_NUMBER_HORIZ: usize = 2;

/// The mode number of `VL3`; the vertical modes follow in `MODE_VERTICAL`
/// order.
pub(crate) const MODE_VERTICAL_BASE: usize = 3;

/// The T.6 mode lookup, built once from the mode codes.
static MODE_LOOKUP: LazyLock<Vec<Slot>> = LazyLock::new(|| {
    let mut table = vec![Slot(0); MODE_LOOKUP_SLOTS];
    fill(
        &mut table,
        MODE_LOOKUP_BITS,
        MODE_PASS,
        Slot::new(MODE_PASS.1, false, 1),
    );
    fill(
        &mut table,
        MODE_LOOKUP_BITS,
        MODE_HORIZ,
        Slot::new(MODE_HORIZ.1, false, 2),
    );
    for (number, &code) in (3_u16..).zip(MODE_VERTICAL.iter()) {
        fill(
            &mut table,
            MODE_LOOKUP_BITS,
            code,
            Slot::new(code.1, false, number),
        );
    }
    table
});

/// The run lookup of one colour, indexed by the next
/// [`RUN_LOOKUP_BITS`] bits of a strip.
pub(crate) fn run_lookup_for(white: bool) -> &'static [Slot] {
    if white { &WHITE_LOOKUP } else { &BLACK_LOOKUP }
}

/// The mode lookup, indexed by the next [`MODE_LOOKUP_BITS`] bits.
pub(crate) fn mode_lookup() -> &'static [Slot] {
    &MODE_LOOKUP
}

/// The terminating and make-up tables of one colour.
pub(crate) const fn colour_tables(white: bool) -> (CodeTable, CodeTable) {
    if white {
        (&WHITE_TERM, &WHITE_MAKEUP)
    } else {
        (&BLACK_TERM, &BLACK_MAKEUP)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EXTENDED_MAKEUP, MODE_HORIZ, MODE_LOOKUP_BITS, MODE_PASS, MODE_VERTICAL, RUN_LOOKUP_BITS,
        Slot, colour_tables, mode_lookup, run_lookup_for,
    };

    /// The slot a code indexes when the spare bits after it are all zeros
    /// or all ones.
    fn slot_at(table: &[Slot], code: (u16, u8), width: u8, tail_ones: bool) -> Option<Slot> {
        let (bits, len) = code;
        let spare = u32::from(width.saturating_sub(len));
        let filler = if tail_ones {
            1_usize.checked_shl(spare).map(|top| top.saturating_sub(1))
        } else {
            Some(0)
        };
        usize::from(bits)
            .checked_shl(spare)
            .zip(filler)
            .and_then(|(head, rest)| table.get(head | rest).copied())
    }

    /// Every transcribed code, each colour's runs and every mode, resolves
    /// through its lookup to its own length and value whatever bits follow
    /// it, so the lookups and the T.4/T.6 tables cannot drift apart.
    #[test]
    fn every_table_code_decodes_through_its_lookup() {
        for white in [true, false] {
            let (terms, makeup) = colour_tables(white);
            let codes = terms.iter().map(|entry| (entry, false)).chain(
                makeup
                    .iter()
                    .chain(EXTENDED_MAKEUP.iter())
                    .map(|entry| (entry, true)),
            );
            for (&(bits, len, run), makes_up) in codes {
                for tail_ones in [false, true] {
                    let slot = slot_at(
                        run_lookup_for(white),
                        (bits, len),
                        RUN_LOOKUP_BITS,
                        tail_ones,
                    );
                    assert_eq!(
                        slot.map(|found| (found.len(), found.is_makeup(), found.value())),
                        Some((len, makes_up, usize::from(run))),
                        "white={white} code {bits:#x}/{len} for run {run}, tail ones={tail_ones}"
                    );
                }
            }
        }
        let modes = [(MODE_PASS, 1_usize), (MODE_HORIZ, 2)]
            .into_iter()
            .chain(MODE_VERTICAL.iter().copied().zip(3_usize..));
        for (code, number) in modes {
            for tail_ones in [false, true] {
                let slot = slot_at(mode_lookup(), code, MODE_LOOKUP_BITS, tail_ones);
                assert_eq!(
                    slot.map(|found| (found.len(), found.value())),
                    Some((code.1, number)),
                    "mode code {code:?} must resolve to mode {number}, tail ones={tail_ones}"
                );
            }
        }
        assert_eq!(
            mode_lookup().first().map(|slot| slot.len()),
            Some(0),
            "seven zeros start no mode"
        );
    }
}
