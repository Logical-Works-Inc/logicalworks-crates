//! Shared deterministic simulation core.
//!
//! One seed drives every choice; FNV-1a accumulates the observable trace so
//! the same seed always yields the same hash. No wall clock, no OS
//! randomness, no threads. Every helper here is used by at least one sim in
//! the unified `sim` target, so the crate's forbid lints stay silent.
//!
//! Fallible narrowing never substitutes a sentinel: conversions use `match`
//! with a documented branch, and remainders use `checked_rem` after the
//! zero guard, so the only defaults in this file sit on provably unreachable
//! arms.
//!
//! Motion-suite helpers (named curves, bit comparison, tween graphs) live in
//! the shared `fixtures` module so every suite that needs them proves them
//! through the fixture-validity test; this module keeps only the seed stream
//! and the trace hash.

/// Deterministic xorshift64 stream.
pub(crate) struct SimRng {
    /// Generator state; never zero.
    state: u64,
}

impl SimRng {
    /// Seed the stream. Zero remaps to a fixed non-zero constant.
    pub(crate) fn seed(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// Next 64-bit value.
    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut word = self.state;
        word ^= word.wrapping_shl(13);
        word ^= word.wrapping_shr(7);
        word ^= word.wrapping_shl(17);
        self.state = word;
        word
    }

    /// Value in `0..bound` (zero bound yields zero).
    pub(crate) fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        // The divisor is non-zero here; the fallback holds the top of the
        // range rather than trapping.
        if let Some(drawn) = self.next_u64().checked_rem(bound) {
            drawn
        } else {
            bound.saturating_sub(1)
        }
    }

    /// Draw in `0..bound` as a `u16`; a zero bound yields zero and leaves the
    /// stream where it was.
    pub(crate) fn below_u16(&mut self, bound: u16) -> u16 {
        // The draw is below a bound that fits in `u16`, so the low two
        // bytes are the whole value, with no conversion to fail.
        let bytes = self.below(u64::from(bound)).to_le_bytes();
        u16::from_le_bytes([bytes[0], bytes[1]])
    }

    /// Draw in `0..bound` as a `u32`; a zero bound yields zero and leaves the
    /// stream where it was.
    pub(crate) fn below_u32(&mut self, bound: u32) -> u32 {
        // The draw is below a bound that fits in `u32`, so the low four
        // bytes are the whole value, with no conversion to fail.
        let bytes = self.below(u64::from(bound)).to_le_bytes();
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Index below an `i32` bound (the bound must be non-negative).
    pub(crate) fn below_i32(&mut self, bound: i32) -> i32 {
        let drawn = if bound <= 0 {
            0
        } else if let Ok(narrow) = u64::try_from(bound) {
            self.below(narrow)
        } else {
            // Unreachable: a positive `i32` always fits in `u64`.
            0
        };
        // The draw is below a non-negative `i32` bound, so the low four
        // bytes are the whole value with a zero sign bit, and no
        // conversion can fail.
        let bytes = drawn.to_le_bytes();
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Index below a length (zero for empty slices).
    pub(crate) fn below_len(&mut self, length: usize) -> usize {
        // Unreachable on 64-bit targets; refuse the draw on others.
        let Ok(bound) = u64::try_from(length) else {
            return 0;
        };
        // Unreachable: the draw is below a bound that fits in `usize`.
        let Ok(index) = usize::try_from(self.below(bound)) else {
            return 0;
        };
        index
    }

    /// Pick a string slice element deterministically, or `None` when empty.
    pub(crate) fn pick_str<'a>(&mut self, options: &'a [&'a str]) -> Option<&'a str> {
        options.get(self.below_len(options.len())).copied()
    }

    /// Lowercase alphanumeric string of `length` characters.
    pub(crate) fn text(&mut self, length: usize) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789.-";
        let mut out = String::new();
        for _ in 0..length {
            // Constant non-zero divisor: the remainder always exists.
            let drawn = self.next_u64().wrapping_rem(38);
            // The draw is below 38, so the low byte is the whole value;
            // `usize` holds every `u8` on all targets.
            let drawn_bytes = drawn.to_le_bytes();
            let slot = usize::from(u8::from_le_bytes([drawn_bytes[0]]));
            // The slot is below the alphabet length, so the byte exists.
            if let Some(byte) = ALPHABET.get(slot) {
                out.push(char::from(*byte));
            }
        }
        out
    }
}

/// A `splitmix64` stream: the generator the ink lab and residency sims draw
/// from. It stays a second named generator, apart from [`SimRng`], so every
/// ink lab seed replays the trace it had before the two suites shared a file;
/// both generators live here so the sims share one randomness module.
pub(crate) struct SplitMix64 {
    /// Generator state.
    state: u64,
}

impl SplitMix64 {
    /// Seed the source.
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64-bit value.
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.state;
        mixed = (mixed ^ mixed.wrapping_shr(30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ mixed.wrapping_shr(27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ mixed.wrapping_shr(31)
    }

    /// A value in `[0, bound)`; `bound` zero yields zero.
    pub(crate) fn below(&mut self, bound: u64) -> u64 {
        let draw = self.next_u64();
        match std::num::NonZeroU64::new(bound) {
            Some(bound) => draw % bound,
            None => 0,
        }
    }

    /// A `u32` in `[0, bound)`; `bound` zero yields zero.
    pub(crate) fn below_u32(&mut self, bound: u32) -> u32 {
        let [b0, b1, b2, b3, _, _, _, _] = self.next_u64().to_be_bytes();
        let draw = u32::from_be_bytes([b0, b1, b2, b3]);
        match std::num::NonZeroU32::new(bound) {
            Some(bound) => draw % bound,
            None => 0,
        }
    }

    /// A `u16` in `[0, bound)`; `bound` zero yields zero.
    pub(crate) fn below_u16(&mut self, bound: u16) -> u16 {
        let [b0, b1, _, _, _, _, _, _] = self.next_u64().to_be_bytes();
        let draw = u16::from_be_bytes([b0, b1]);
        match std::num::NonZeroU16::new(bound) {
            Some(bound) => draw % bound,
            None => 0,
        }
    }

    /// A float in `[0, 1)`.
    pub(crate) fn unit(&mut self) -> f64 {
        let [b0, b1, b2, b3, _, _, _, _] = self.next_u64().to_be_bytes();
        f64::from(u32::from_be_bytes([b0, b1, b2, b3])) / 4_294_967_296.0
    }

    /// A float in `[low, high)`.
    pub(crate) fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    /// Pick one of `items`.
    pub(crate) fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        let length = u64::try_from(items.len()).ok()?;
        let index = usize::try_from(self.below(length)).ok()?;
        items.get(index).copied()
    }
}

/// The initial trace hash: the FNV-1a offset basis.
pub(crate) const TRACE_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold one 64-bit word into a trace hash (FNV-1a over words).
pub(crate) fn mix(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(0x0100_0000_01b3)
}

/// FNV-1a trace accumulator.
pub(crate) struct TraceHash {
    /// Running hash.
    hash: u64,
}

impl TraceHash {
    /// Fresh trace starting from the FNV offset basis.
    pub(crate) fn new() -> Self {
        Self { hash: TRACE_SEED }
    }

    /// Fold `value` into the running hash as one FNV-1a word. Order matters:
    /// swapping two observations changes the result.
    pub(crate) fn mix_u64(&mut self, value: u64) {
        self.hash = mix(self.hash, value);
    }

    /// Fold every byte of `text`, then a `0xff` terminator that UTF-8 never
    /// contains, so the pairs `("ab", "c")` and `("a", "bc")` hash apart.
    pub(crate) fn mix_str(&mut self, text: &str) {
        for byte in text.bytes() {
            self.mix_u64(u64::from(byte));
        }
        self.mix_u64(0xff);
    }

    /// Mix one bit observation.
    pub(crate) fn mix_bool(&mut self, value: bool) {
        self.mix_u64(if value { 1 } else { 0 });
    }

    /// Mix one length observation, byte by byte, so no width is lost and no
    /// narrowing can substitute a sentinel.
    pub(crate) fn mix_len(&mut self, length: usize) {
        for byte in length.to_le_bytes() {
            self.mix_u64(u64::from(byte));
        }
    }

    /// Mix raw float bits (never the float itself).
    pub(crate) fn mix_bits(&mut self, value: f64) {
        self.mix_u64(value.to_bits());
    }

    /// The hash so far. Reading it never advances the trace, so a sim may
    /// sample it mid-run and keep mixing.
    pub(crate) fn finish(&self) -> u64 {
        self.hash
    }
}
