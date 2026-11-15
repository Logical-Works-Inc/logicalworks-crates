//! The seeded draw stream the crate's in-module simulations share.
//!
//! In-module simulations (`sim_*.rs` beside the code they test) cannot reach
//! the integration suite's `tests/sim` substrate, so they draw from this one
//! stream instead of each carrying a copy: one word of SplitMix64 state, so a
//! seed is a replayable trace, and every draw refuses rather than guesses when
//! asked for an index into nothing.

use std::fmt;

use crate::error::KernelError;

/// A draw the stream refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawError {
    /// An index was asked of an empty range.
    EmptyRange,
    /// A drawn index fell outside the `len` items it was drawn for.
    OutsideItems {
        /// The index drawn.
        at: usize,
        /// How many items there were.
        len: usize,
    },
}

impl fmt::Display for DrawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::EmptyRange => f.write_str("an index drawn from an empty range"),
            Self::OutsideItems { at, len } => write!(f, "{at} is outside {len} items"),
        }
    }
}

impl std::error::Error for DrawError {}

/// Families whose own verdict type is still text read a refusal as its line.
impl From<DrawError> for String {
    fn from(error: DrawError) -> Self {
        error.to_string()
    }
}

/// Why a seeded check could not reach its verdict. A failed assertion is not
/// this: assertions fail the test on their own, and this is the check being
/// unable to ask its question.
#[derive(Debug)]
pub(crate) enum SimError {
    /// The stream refused a draw.
    Draw(DrawError),
    /// A call came back as something other than the outcome the check drives.
    Unexpected {
        /// The seed whose scenario drew the call.
        seed: u64,
        /// What the check expected the call to be.
        wanted: &'static str,
        /// What came back instead, boxed so the `Result` stays small.
        got: Box<KernelError>,
    },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Draw(ref error) => write!(f, "{error}"),
            Self::Unexpected {
                seed,
                wanted,
                ref got,
            } => write!(f, "seed {seed:#x}: expected {wanted}, got {got}"),
        }
    }
}

impl std::error::Error for SimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Draw(ref error) => Some(error),
            Self::Unexpected { ref got, .. } => Some(&**got),
        }
    }
}

impl From<DrawError> for SimError {
    fn from(error: DrawError) -> Self {
        Self::Draw(error)
    }
}

/// Families whose own verdict type is still text read a refusal as its line.
impl From<SimError> for String {
    fn from(error: SimError) -> Self {
        error.to_string()
    }
}

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
    pub(crate) fn below(&mut self, bound: usize) -> Result<usize, DrawError> {
        let [low, high, ..] = self.next().to_le_bytes();
        usize::from(u16::from_le_bytes([low, high]))
            .checked_rem(bound)
            .ok_or(DrawError::EmptyRange)
    }

    /// A drawn element of `items`.
    pub(crate) fn choose<T: Copy>(&mut self, items: &[T]) -> Result<T, DrawError> {
        let at = self.below(items.len())?;
        items.get(at).copied().ok_or(DrawError::OutsideItems {
            at,
            len: items.len(),
        })
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
/// Named by a counter and made with exclusive creation, never by a clock or a
/// process id: the OS reuses ids, and a reused one must not make two
/// simulations share a directory. A name already taken (by a parallel test
/// process, or by a directory a crashed run left) is stepped over, so the
/// directory handed back is always new and empty.
pub(crate) struct TempDir(std::path::PathBuf);

impl TempDir {
    /// Names tried before giving up: more than any one run leaves behind.
    const ATTEMPTS: u32 = 4096;

    /// A fresh directory whose name starts `lgwks-sim-<name>`.
    pub(crate) fn new(name: &str) -> std::io::Result<Self> {
        use std::io::ErrorKind;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir();
        for _ in 0..Self::ATTEMPTS {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = root.join(format!("lgwks-sim-{name}-{unique}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    lgwks_std::trace::warn!(error = %error, "sim_stream: a simulation's directory was not made");
                    return Err(error);
                }
            }
        }
        Err(ErrorKind::AlreadyExists.into())
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
