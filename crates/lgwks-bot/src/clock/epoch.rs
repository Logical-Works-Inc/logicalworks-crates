//! A clock reading that means the same thing on any host and after any restart.
//!
//! [`Clock`] counts from an origin nobody else knows, which is right for a deadline
//! and useless for a record. An [`EpochClock`] is a [`Clock`] plus one anchor: the
//! Unix-epoch nanosecond its timeline starts at. [`EpochClock::now`] returns a
//! [`RecordedInstant`], a plain count of nanoseconds since the Unix epoch, so a
//! journal can hold it and another host can read it back exactly.
//!
//! The anchor is read once. A live clock then advances on the monotonic clock, so
//! an operator stepping the system clock does not move a run's timeline. A clock
//! rebuilt with [`EpochClock::system_after`] never reads earlier than the last
//! instant it recorded, even when the system clock went backwards across the
//! restart.
//!
//! Under simulation [`EpochClock::seeded`] takes its anchor from the seed and reads
//! no real clock, so the same seed and the same advances give the same bytes on
//! every host. `tests/it/sim_recorded_instant.rs` holds it.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Clock, ClockError, duration_to_nanos};

/// One moment, as nanoseconds since the Unix epoch.
///
/// Ordered and comparable across hosts as far as their anchors agree: two live
/// hosts carry whatever skew their system clocks had when they read the anchor,
/// and [`SkewBound`](crate::skew::SkewBound) is the type that prices it. A deadline
/// still belongs to [`Clock`], not to this.
///
/// Archived by the `wire` feature, so it is a `journal::Durable` record under
/// that feature.
///
/// # Examples
///
/// ```
/// use lgwks_bot::kernel::clock::{EpochClock, RecordedInstant};
/// let instant = EpochClock::seeded(8).now();
/// assert_eq!(instant, RecordedInstant::from_unix_nanos(4));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv))]
pub struct RecordedInstant {
    /// Nanoseconds since the Unix epoch, saturating about the year 2554.
    unix_nanos: u64,
}

impl RecordedInstant {
    /// The instant `unix_nanos` nanoseconds after the Unix epoch.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::RecordedInstant;
    /// assert_eq!(RecordedInstant::from_unix_nanos(9).unix_nanos(), 9);
    /// ```
    #[must_use]
    pub const fn from_unix_nanos(unix_nanos: u64) -> Self {
        Self { unix_nanos }
    }

    /// Nanoseconds since the Unix epoch.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::RecordedInstant;
    /// assert_eq!(RecordedInstant::from_unix_nanos(0).unix_nanos(), 0);
    /// ```
    #[must_use]
    pub const fn unix_nanos(self) -> u64 {
        self.unix_nanos
    }

    /// How long after `earlier` this instant is, or zero if it is not after it.
    ///
    /// Saturating like [`std::time::Instant::saturating_duration_since`], because
    /// two hosts' readings can legitimately cross by their skew.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::RecordedInstant;
    /// let (early, late) = (RecordedInstant::from_unix_nanos(1), RecordedInstant::from_unix_nanos(4));
    /// assert_eq!(late.saturating_duration_since(early).as_nanos(), 3);
    /// assert_eq!(early.saturating_duration_since(late).as_nanos(), 0);
    /// ```
    #[must_use]
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.unix_nanos.saturating_sub(earlier.unix_nanos))
    }
}

impl fmt::Display for RecordedInstant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let since_epoch = Duration::from_nanos(self.unix_nanos);
        write!(
            formatter,
            "{}.{:09}s since the Unix epoch",
            since_epoch.as_secs(),
            since_epoch.subsec_nanos()
        )
    }
}

/// A [`Clock`] anchored to the Unix epoch.
///
/// Cheap to clone, and every clone shares one timeline, as with [`Clock`].
///
/// # Examples
///
/// ```
/// use lgwks_bot::kernel::clock::{EpochClock, RecordedInstant};
/// let clock = EpochClock::restored(RecordedInstant::from_unix_nanos(1));
/// assert_eq!(clock.clone().now(), clock.now());
/// ```
#[derive(Debug, Clone)]
pub struct EpochClock {
    /// The timeline, and the authority for any deadline taken from this clock.
    clock: Clock,
    /// The Unix-epoch nanosecond the timeline's zero stands for.
    anchor: u64,
}

impl EpochClock {
    /// A live clock anchored at the system clock's reading now.
    ///
    /// # Errors
    ///
    /// [`ClockError::BeforeEpoch`] when the system clock reads before 1970, and
    /// [`ClockError::OutOfRange`] when it reads past the representable ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::{ClockError, EpochClock};
    /// let clock = EpochClock::system()?;
    /// assert!(clock.now().unix_nanos() > 0);
    /// # Ok::<(), ClockError>(())
    /// ```
    pub fn system() -> Result<Self, ClockError> {
        Self::system_after(RecordedInstant::from_unix_nanos(0))
    }

    /// A live clock that never reads earlier than `floor`, for a restart.
    ///
    /// `floor` is the last instant the previous process recorded. A system clock
    /// that went backwards while the process was down would otherwise order new
    /// records before old ones.
    ///
    /// # Errors
    ///
    /// As [`EpochClock::system`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::{EpochClock, RecordedInstant};
    /// let floor = RecordedInstant::from_unix_nanos(u64::MAX - 1);
    /// assert!(EpochClock::system_after(floor)?.now() >= floor);
    /// # Ok::<(), lgwks_bot::kernel::clock::ClockError>(())
    /// ```
    pub fn system_after(floor: RecordedInstant) -> Result<Self, ClockError> {
        let Ok(since_epoch) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            lgwks_std::trace::debug!("system_after: the system clock reads before the epoch");
            return Err(ClockError::BeforeEpoch);
        };
        let ceiling = Clock::elapsed_ceiling();
        if since_epoch > ceiling {
            lgwks_std::trace::debug!(
                ?since_epoch,
                ?ceiling,
                "system_after: the system clock reads past the ceiling"
            );
            return Err(ClockError::OutOfRange {
                requested: since_epoch,
                ceiling,
            });
        }
        Ok(Self {
            clock: Clock::wall(),
            anchor: duration_to_nanos(since_epoch).max(floor.unix_nanos),
        })
    }

    /// A caller-advanced clock whose anchor comes from `seed` and no real clock.
    ///
    /// The seed's top bit is dropped so a run has half the representable range to
    /// advance through before it saturates.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::EpochClock;
    /// assert_eq!(EpochClock::seeded(7).now(), EpochClock::seeded(7).now());
    /// ```
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        Self::restored(RecordedInstant::from_unix_nanos(seed >> 1))
    }

    /// A caller-advanced clock standing at `instant`, for a replay.
    ///
    /// It reads exactly `instant` until it is advanced, on any host.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::{EpochClock, RecordedInstant};
    /// let instant = RecordedInstant::from_unix_nanos(5);
    /// assert_eq!(EpochClock::restored(instant).now(), instant);
    /// ```
    #[must_use]
    pub fn restored(instant: RecordedInstant) -> Self {
        Self {
            clock: Clock::virtual_at(Duration::ZERO),
            anchor: instant.unix_nanos,
        }
    }

    /// This moment on the timeline, saturating at the representable ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::EpochClock;
    /// assert_eq!(EpochClock::seeded(2).now().unix_nanos(), 1);
    /// ```
    #[must_use]
    pub fn now(&self) -> RecordedInstant {
        RecordedInstant::from_unix_nanos(
            self.anchor
                .saturating_add(duration_to_nanos(self.clock.now())),
        )
    }

    /// Move a caller-advanced clock forward and return where it landed.
    ///
    /// # Errors
    ///
    /// As [`Clock::advance`]: [`ClockError::NotVirtual`] on a live clock, and
    /// [`ClockError::OutOfRange`] when the timeline is already at its ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::{ClockError, EpochClock};
    /// use std::time::Duration;
    /// let clock = EpochClock::seeded(0);
    /// assert_eq!(clock.advance(Duration::from_nanos(3))?.unix_nanos(), 3);
    /// # Ok::<(), ClockError>(())
    /// ```
    pub fn advance(&self, by: Duration) -> Result<RecordedInstant, ClockError> {
        self.clock.advance(by)?;
        Ok(self.now())
    }

    /// The timeline, for a deadline or a sleep on the same time this clock reads.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::kernel::clock::EpochClock;
    /// use std::time::Duration;
    /// assert_eq!(EpochClock::seeded(0).clock().now(), Duration::ZERO);
    /// ```
    #[must_use]
    pub const fn clock(&self) -> &Clock {
        &self.clock
    }
}
