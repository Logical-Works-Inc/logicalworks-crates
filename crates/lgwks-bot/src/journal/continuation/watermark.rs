//! When a journal continues: the declared policy and the watermark measured against it.

use crate::journal::{JournalError, JournalLimitKind};

/// Eight tenths of either ceiling: the declared continuation watermark.
///
/// A constant rather than a parameter because the point of it is that the
/// decision is made *before* anything depends on it. One fifth of each ceiling is
/// left as settlement headroom, so an attempt already handed to the outside world
/// can always record its outcome: the ladder's third rung is admitted at the
/// watermark and refused only at the hard cap, and a continuation happens before
/// either.
const CONTINUATION_WATERMARK_NUMERATOR: u64 = 8;

/// The denominator of [`CONTINUATION_WATERMARK_NUMERATOR`].
const CONTINUATION_WATERMARK_DENOMINATOR: u64 = 10;

/// The event ceiling a policy is measured against.
pub(super) const SHIPPED_EVENT_CEILING: u64 = 100_000;

/// The byte ceiling a policy is measured against.
pub(super) const SHIPPED_BYTE_CEILING: u64 = 64 * 1024 * 1024;

/// The watermark count for a ceiling of `limit` events or bytes.
///
/// Integer arithmetic in the declared order, so the watermark is the same number
/// on every host: `limit * 8 / 10`, with the multiplication first because it is
/// exact for every ceiling this crate declares. The `checked_div` arm is
/// unreachable for the constant denominator above; it is here rather than an
/// `unwrap` because the crate forbids both.
pub(super) const fn watermark_of(limit: u64) -> u64 {
    match limit
        .saturating_mul(CONTINUATION_WATERMARK_NUMERATOR)
        .checked_div(CONTINUATION_WATERMARK_DENOMINATOR)
    {
        Some(at) => at,
        None => limit,
    }
}

/// Where one journal continues.
///
/// Declared rather than derived from the file, so the trigger is a policy a host
/// can state, read back and measure against rather than a constant buried in a
/// comparison. [`ContinuationPolicy::declared`] is the shipped default — eight
/// tenths of each ceiling — and it is the one an unattended run should use; the
/// constructor exists because a host with a shorter retention budget than this
/// crate's defaults, and a simulation that has to reach a hundred continuations
/// inside a test's wall clock, both need to move the trigger rather than fake the
/// ceilings.
///
/// Moving it is always *earlier*. A policy past a ceiling would ask a journal to
/// continue when it already cannot admit another event, so the constructor
/// refuses rather than clamping: a silently clamped trigger is indistinguishable
/// from the one the caller asked for.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::continuation::ContinuationPolicy;
///
/// let policy = ContinuationPolicy::default();
/// assert_eq!(policy, ContinuationPolicy::declared());
/// assert_eq!(policy.events_at(), 80_000);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationPolicy {
    /// The committed event count at which a continuation is due.
    events_at: u64,
    /// The acknowledged byte length at which a continuation is due.
    bytes_at: u64,
}

impl ContinuationPolicy {
    /// The shipped policy: eight tenths of each ceiling, which leaves the
    /// remaining fifth as the settlement headroom #143 asked for.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationPolicy;
    ///
    /// let policy = ContinuationPolicy::declared();
    /// assert_eq!(policy.events_at(), 80_000);
    /// assert_eq!(policy.bytes_at(), 53_687_091);
    /// ```
    #[must_use]
    pub const fn declared() -> Self {
        Self {
            events_at: watermark_of(SHIPPED_EVENT_CEILING),
            bytes_at: watermark_of(SHIPPED_BYTE_CEILING),
        }
    }

    /// A policy with explicit trigger points.
    ///
    /// # Errors
    ///
    /// [`JournalError::CapacityExceeded`] when either point is at or past its
    /// ceiling, because such a policy would ask for a continuation the journal
    /// could not afford to perform.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationPolicy;
    ///
    /// let earlier = ContinuationPolicy::new(1_000, 4_096);
    /// assert_eq!(earlier.map(|policy| policy.events_at()).ok(), Some(1_000));
    /// assert!(ContinuationPolicy::new(100_000, 1).is_err());
    /// ```
    pub fn new(events_at: u64, bytes_at: u64) -> Result<Self, JournalError> {
        if events_at >= SHIPPED_EVENT_CEILING || bytes_at >= SHIPPED_BYTE_CEILING {
            let requested = events_at.max(bytes_at);
            let refusal = Err(JournalError::CapacityExceeded {
                resource: JournalLimitKind::ContinuationWatermark,
                limit: SHIPPED_EVENT_CEILING,
                requested,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "ContinuationPolicy::new: returning an error to the caller");
            return refusal;
        }
        Ok(Self {
            events_at,
            bytes_at,
        })
    }

    /// The committed event count at which a continuation is due.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationPolicy;
    ///
    /// assert_eq!(ContinuationPolicy::declared().events_at(), 80_000);
    /// ```
    #[must_use]
    pub const fn events_at(&self) -> u64 {
        self.events_at
    }

    /// The acknowledged byte length at which a continuation is due.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationPolicy;
    ///
    /// assert_eq!(ContinuationPolicy::declared().bytes_at(), 53_687_091);
    /// ```
    #[must_use]
    pub const fn bytes_at(&self) -> u64 {
        self.bytes_at
    }
}

impl Default for ContinuationPolicy {
    fn default() -> Self {
        Self::declared()
    }
}

/// How much of each ceiling a journal has used, and whether it continues.
///
/// The question a caller asks before an append, and the answer is a measurement
/// rather than a guess: it is read from the journal's own committed count and
/// acknowledged byte length, both of which the append fence already keeps exact.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::continuation::ContinuationWatermark;
///
/// let mark = ContinuationWatermark::inert(10, 100_000, 0, 67_108_864);
/// assert!(!mark.is_due());
/// assert_eq!(mark.events(), (10, 100_000));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationWatermark {
    /// Whether this journal continues at all.
    continuing: bool,
    /// Committed events and the ceiling they are measured against.
    events: u64,
    /// The event ceiling this journal was opened against.
    events_limit: u64,
    /// Committed bytes and the ceiling they are measured against.
    bytes: u64,
    /// The byte ceiling this journal was opened against.
    bytes_limit: u64,
    /// The event count at which this journal's watermark falls.
    events_at: u64,
    /// The byte length at which this journal's watermark falls.
    bytes_at: u64,
}

impl ContinuationWatermark {
    /// A watermark for a journal that does not continue.
    ///
    /// The answer every adapter but the continuing file journal gives. It is a
    /// real measurement of a real journal with `continuing` false rather than a
    /// zeroed placeholder: "this journal will not continue" is the answer, and a
    /// sentinel would be a measurement nobody took.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationWatermark;
    ///
    /// let mark = ContinuationWatermark::inert(99_999, 100_000, 0, 67_108_864);
    /// assert!(!mark.is_due());
    /// assert_eq!(mark.events(), (99_999, 100_000));
    /// ```
    #[must_use]
    pub const fn inert(events: u64, events_limit: u64, bytes: u64, bytes_limit: u64) -> Self {
        Self {
            continuing: false,
            events,
            events_limit,
            bytes,
            bytes_limit,
            events_at: 0,
            bytes_at: 0,
        }
    }

    /// A watermark for a journal that continues at its declared policy's two
    /// trigger points.
    #[cfg(feature = "wire")]
    #[must_use]
    pub(crate) const fn measured(
        events: u64,
        events_limit: u64,
        bytes: u64,
        bytes_limit: u64,
        policy: ContinuationPolicy,
    ) -> Self {
        Self {
            continuing: true,
            events,
            events_limit,
            bytes,
            bytes_limit,
            events_at: policy.events_at,
            bytes_at: policy.bytes_at,
        }
    }

    /// Whether a continuation is due now.
    ///
    /// `false` for a journal that does not continue, which is what leaves every
    /// non-continuing adapter's behaviour exactly as it was: the append after
    /// this question is the append that always happened.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationWatermark;
    ///
    /// assert!(!ContinuationWatermark::inert(99_999, 100_000, 0, 67_108_864).is_due());
    /// ```
    #[must_use]
    pub const fn is_due(&self) -> bool {
        self.continuing && (self.events >= self.events_at || self.bytes >= self.bytes_at)
    }

    /// Committed events and the ceiling they are measured against.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationWatermark;
    ///
    /// let mark = ContinuationWatermark::inert(7, 100_000, 9, 67_108_864);
    /// assert_eq!(mark.events(), (7, 100_000));
    /// ```
    #[must_use]
    pub const fn events(&self) -> (u64, u64) {
        (self.events, self.events_limit)
    }

    /// Committed bytes and the ceiling they are measured against.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::continuation::ContinuationWatermark;
    ///
    /// let mark = ContinuationWatermark::inert(7, 100_000, 9, 67_108_864);
    /// assert_eq!(mark.bytes(), (9, 67_108_864));
    /// ```
    #[must_use]
    pub const fn bytes(&self) -> (u64, u64) {
        (self.bytes, self.bytes_limit)
    }
}
