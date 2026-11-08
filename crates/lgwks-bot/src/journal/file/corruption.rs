//! What the journal says about committed bytes it refuses.

use super::ChainBreak;

/// Why the journal refused its own committed bytes.
///
/// Distinct from a torn tail, and the distinction is the point: a torn tail
/// was never acknowledged and is repaired, while either arm here may have
/// been, and so is refused.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::CorruptionKind;
///
/// assert_ne!(CorruptionKind::Framed, CorruptionKind::Sealed);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CorruptionKind {
    /// The frame's bytes do not decode to an event.
    Undecodable,
    /// The frame's length prefix cannot be true of any frame this journal
    /// writes. A killed writer leaves only a prefix of its bytes, so a
    /// complete prefix that names an impossible frame is rot or a hand,
    /// never an interrupted append. The same holds for a length that is
    /// *possible* but lies: the frame's own stored head authenticates the
    /// bytes actually on the disk, and a tail that verifies as a complete
    /// frame under its true length was never interrupted.
    Framed,
    /// The stored head does not follow from the events before it.
    Chain(ChainBreak),
    /// The file holds a second sealed checkpoint.
    ///
    /// One journal continues once. A second seal frame in one file is bytes no
    /// writer of this journal produces, and it is refused rather than read as a
    /// later continuation: the successor is a distinct file, so a seal that
    /// appears here twice means the file's own chain has been given two
    /// incompatible answers about where its authoritative journal is.
    Sealed,
}

impl CorruptionKind {
    /// The human spelling, for the error chain.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::{Corruption, CorruptionKind};
    ///
    /// let refused = Corruption::new(0, CorruptionKind::Sealed);
    /// assert!(refused.kind().as_str().contains("sealed"));
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undecodable => "the frame's bytes do not decode to an event",
            Self::Framed => {
                "the frame's length prefix cannot be true of any frame this journal writes"
            }
            Self::Chain(_) => "the stored head does not follow from the events before it",
            Self::Sealed => "the file holds a second sealed checkpoint",
        }
    }
}

impl core::fmt::Display for CorruptionKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One refused frame, named.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::{Corruption, CorruptionKind};
///
/// let refused = Corruption::new(3, CorruptionKind::Framed);
/// assert!(refused.to_string().contains("frame 3"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Corruption {
    /// The frame's index, counted from zero in file order.
    at: u64,
    /// Why it was refused.
    kind: CorruptionKind,
}

impl Corruption {
    /// Name a refused frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::{Corruption, CorruptionKind};
    ///
    /// let refused = Corruption::new(3, CorruptionKind::Framed);
    /// assert_eq!(refused.at(), 3);
    /// ```
    #[must_use]
    pub const fn new(at: u64, kind: CorruptionKind) -> Self {
        Self { at, kind }
    }

    /// The frame's index, counted from zero in file order.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::{Corruption, CorruptionKind};
    ///
    /// assert_eq!(Corruption::new(7, CorruptionKind::Undecodable).at(), 7);
    /// ```
    #[must_use]
    pub const fn at(&self) -> u64 {
        self.at
    }

    /// Why the frame was refused.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::{Corruption, CorruptionKind};
    ///
    /// let refused = Corruption::new(0, CorruptionKind::Framed);
    /// assert_eq!(*refused.kind(), CorruptionKind::Framed);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> &CorruptionKind {
        &self.kind
    }
}

impl core::fmt::Display for Corruption {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "frame {} corrupt: {}", self.at, self.kind)
    }
}

impl std::error::Error for Corruption {}
