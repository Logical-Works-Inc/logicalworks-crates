//! The seeded draw stream the crate's in-module simulations share.
//!
//! In-module simulations (`sim_*.rs` beside the code they test) cannot reach
//! the integration suite's `tests/sim` substrate, so they draw from this one
//! stream instead of each carrying a copy: one word of SplitMix64 state, so a
//! seed is a replayable trace, and every draw refuses rather than guesses when
//! asked for an index into nothing.

/// SplitMix64 over one word of state.
pub(crate) struct Stream(u64);

impl Stream {
    /// The stream for `seed`.
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next word.
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A drawn index in `0..bound`, from the word's low sixteen bits: every
    /// bound a simulation here asks for is far below 65,536, so the bias is
    /// under one part in a hundred. An empty range has no index, and is
    /// refused.
    pub(crate) fn below(&mut self, bound: usize) -> Result<usize, String> {
        let [low, high, ..] = self.next().to_le_bytes();
        usize::from(u16::from_le_bytes([low, high]))
            .checked_rem(bound)
            .ok_or_else(|| "an index drawn from an empty range".to_owned())
    }

    /// A drawn element of `items`.
    pub(crate) fn choose<T: Copy>(&mut self, items: &[T]) -> Result<T, String> {
        let at = self.below(items.len())?;
        items
            .get(at)
            .copied()
            .ok_or_else(|| format!("{at} is outside {} items", items.len()))
    }
}

/// The FNV-1a 64 fold of `bytes` into `digest`: the replay receipt every
/// in-module simulation hashes its verdicts into.
pub(crate) fn fold(digest: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(digest, |digest, byte| {
        (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The FNV-1a 64 offset basis, where every receipt starts.
pub(crate) const RECEIPT_START: u64 = 0xcbf2_9ce4_8422_2325;

/// A directory of one simulation's own, removed when it drops.
///
/// Named by nanos plus a counter, never by a process id: the OS reuses ids, and
/// a reused one must not make two simulations share a directory.
pub(crate) struct TempDir(std::path::PathBuf);

impl TempDir {
    /// A fresh directory whose name starts `lgwks-sim-<name>`.
    pub(crate) fn new(name: &str) -> std::io::Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static UNIQUE: AtomicU64 = AtomicU64::new(0);
        let unique = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(since) => since.as_nanos(),
            Err(before) => before.duration().as_nanos(),
        };
        let dir = std::env::temp_dir().join(format!("lgwks-sim-{name}-{nanos}-{unique}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// The directory.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(cause) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::warn!(error = %cause, "sim_stream: a simulation's directory stayed");
        }
    }
}

/// Draws `draws` scenarios under each of `seeds` and checks each one.
///
/// The one sweep loop the in-module families share: a scenario is drawn from
/// the seed's stream, then checked with the same stream, so whatever a check
/// draws is part of the seed's trace too.
pub(crate) fn sweep<S>(
    seeds: &[u64],
    draws: usize,
    mut draw: impl FnMut(&mut Stream) -> Result<S, String>,
    mut check: impl FnMut(u64, &mut Stream, &S) -> Result<(), String>,
) -> Result<(), String> {
    for &seed in seeds {
        let mut stream = Stream::new(seed);
        for _ in 0..draws {
            let scenario = draw(&mut stream)?;
            check(seed, &mut stream, &scenario)?;
        }
    }
    Ok(())
}
