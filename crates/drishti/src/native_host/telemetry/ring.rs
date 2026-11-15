//! Bounded ring, latency tracker, and the metrics snapshot of the tap.

use std::num::NonZeroUsize;

use super::TelemetryError;

/// Fixed-capacity ring buffer that overwrites the oldest item when full.
/// # Examples
/// ```
/// use drishti::native_host::telemetry::RingBuffer;
/// let mut ring = RingBuffer::new(2)?;
/// for item in [1, 2, 3] {
///     ring.push(item);
/// }
/// assert_eq!(ring.snapshot(), vec![2, 3]);
/// # Ok::<(), drishti::native_host::telemetry::TelemetryError>(())
/// ```
pub struct RingBuffer<T> {
    /// Slots in write order.
    storage: Vec<T>,
    /// Capacity (always at least one).
    capacity: NonZeroUsize,
    /// Index the next item overwrites once full.
    write_head: usize,
    /// Total items ever pushed (never reset).
    total_pushed: u64,
}

impl<T> RingBuffer<T> {
    /// Create a ring buffer. Capacities below one are refused rather than
    /// panicking.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::InvalidCapacity`] for a zero capacity.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::{RingBuffer, TelemetryError};
    /// assert!(RingBuffer::<u8>::new(1).is_ok());
    /// assert_eq!(RingBuffer::<u8>::new(0).err(), Some(TelemetryError::InvalidCapacity(0)));
    /// ```
    pub fn new(capacity: usize) -> Result<Self, TelemetryError> {
        let capacity = NonZeroUsize::new(capacity).ok_or(TelemetryError::InvalidCapacity(0))?;
        Ok(Self::sized(capacity))
    }

    /// An empty ring of an already-checked capacity.
    const fn sized(capacity: NonZeroUsize) -> Self {
        Self {
            storage: Vec::new(),
            capacity,
            write_head: 0,
            total_pushed: 0,
        }
    }

    /// Push an item, overwriting the oldest once full.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::RingBuffer;
    /// let mut ring = RingBuffer::new(1)?;
    /// ring.push('a');
    /// ring.push('b');
    /// assert_eq!(ring.snapshot(), vec!['b']);
    /// # Ok::<(), drishti::native_host::telemetry::TelemetryError>(())
    /// ```
    pub fn push(&mut self, item: T) {
        if self.storage.len() < self.capacity.get() {
            self.storage.push(item);
        } else {
            self.storage[self.write_head] = item;
            self.write_head = cycle_next(self.write_head, self.capacity);
        }
        self.total_pushed = self.total_pushed.wrapping_add(1);
    }

    /// Number of items currently held (capped at capacity).
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::RingBuffer;
    /// let mut ring = RingBuffer::new(2)?;
    /// (0..5).for_each(|item| ring.push(item));
    /// assert_eq!((ring.count(), ring.total_pushed()), (2, 5));
    /// # Ok::<(), drishti::native_host::telemetry::TelemetryError>(())
    /// ```
    #[must_use]
    pub fn count(&self) -> usize {
        self.storage.len()
    }

    /// Total items ever pushed (never capped).
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::RingBuffer;
    /// let mut ring = RingBuffer::new(1)?;
    /// ring.push(1);
    /// ring.push(2);
    /// assert_eq!(ring.total_pushed(), 2);
    /// # Ok::<(), drishti::native_host::telemetry::TelemetryError>(())
    /// ```
    #[must_use]
    pub const fn total_pushed(&self) -> u64 {
        self.total_pushed
    }

    /// Copy of current items, oldest first, newest last.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::RingBuffer;
    /// let mut ring = RingBuffer::new(3)?;
    /// (1..=4).for_each(|item| ring.push(item));
    /// assert_eq!(ring.snapshot(), vec![2, 3, 4]);
    /// # Ok::<(), drishti::native_host::telemetry::TelemetryError>(())
    /// ```
    #[must_use]
    pub fn snapshot(&self) -> Vec<T>
    where
        T: Clone,
    {
        // The write head stays at slot zero until the ring first fills, so
        // the oldest item is always at the write head.
        (0..self.storage.len())
            .map(|offset| self.storage[cycle_add(self.write_head, offset, self.capacity)].clone())
            .collect()
    }
}

/// Advance a ring position by `offset` modulo the capacity. The capacity is
/// a `NonZeroUsize`, so the remainder cannot trap and needs no fallback.
fn cycle_add(position: usize, offset: usize, capacity: NonZeroUsize) -> usize {
    position.saturating_add(offset) % capacity
}

/// Advance a ring write head by one modulo the capacity.
fn cycle_next(position: usize, capacity: NonZeroUsize) -> usize {
    cycle_add(position, 1, capacity)
}

/// Snapshot of telemetry health at a point in time.
///
/// Fields are private: [`TelemetryTap::metrics`](super::TelemetryTap::metrics)
/// builds snapshots and readers go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::telemetry::TelemetryMetrics;
/// let metrics = TelemetryMetrics::new(3, 1, 40);
/// assert_eq!(metrics.emits_observed(), 3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TelemetryMetrics {
    /// Tap events observed since start.
    emits_observed: u64,
    /// Governance appends that failed.
    append_errors: u64,
    /// 99th-percentile tap-callback latency, whole microseconds.
    tap_latency_p99_microseconds: u64,
}

impl TelemetryMetrics {
    /// Assemble a metrics snapshot.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::TelemetryMetrics;
    /// let metrics = TelemetryMetrics::new(1, 2, 3);
    /// assert_eq!(metrics.tap_latency_p99_microseconds(), 3);
    /// ```
    #[must_use]
    pub const fn new(
        emits_observed: u64,
        append_errors: u64,
        tap_latency_p99_microseconds: u64,
    ) -> Self {
        Self {
            emits_observed,
            append_errors,
            tap_latency_p99_microseconds,
        }
    }

    /// Tap events observed since start.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::TelemetryMetrics;
    /// assert_eq!(TelemetryMetrics::new(7, 0, 0).emits_observed(), 7);
    /// ```
    #[must_use]
    pub const fn emits_observed(&self) -> u64 {
        self.emits_observed
    }

    /// Governance appends that failed.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::TelemetryMetrics;
    /// assert_eq!(TelemetryMetrics::new(0, 4, 0).append_errors(), 4);
    /// ```
    #[must_use]
    pub const fn append_errors(&self) -> u64 {
        self.append_errors
    }

    /// 99th-percentile tap-callback latency, whole microseconds.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::TelemetryMetrics;
    /// assert_eq!(TelemetryMetrics::new(0, 0, 9).tap_latency_p99_microseconds(), 9);
    /// ```
    #[must_use]
    pub const fn tap_latency_p99_microseconds(&self) -> u64 {
        self.tap_latency_p99_microseconds
    }
}

/// Accumulates nanosecond wall-clock samples and computes p99 over a bounded
/// 1,000-sample ring.
/// # Examples
/// ```
/// use drishti::native_host::telemetry::LatencyTracker;
/// let mut tracker = LatencyTracker::new();
/// tracker.record(5_000);
/// assert_eq!(tracker.p99_microseconds(), 5);
/// ```
pub struct LatencyTracker {
    /// Recent samples, oldest overwritten first.
    samples: RingBuffer<u64>,
}

impl LatencyTracker {
    /// Tracker with the default 1,000-sample ring.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::LatencyTracker;
    /// assert_eq!(LatencyTracker::new().p99_microseconds(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(1_000)
    }

    /// Tracker with an explicit sample ring capacity.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::LatencyTracker;
    /// // The ring holds two samples, so the 9 ms sample has been overwritten.
    /// let mut tracker = LatencyTracker::with_capacity(2);
    /// for sample in [9_000_000, 1_000, 2_000] {
    ///     tracker.record(sample);
    /// }
    /// assert_eq!(tracker.p99_microseconds(), 1);
    /// ```
    #[must_use]
    pub fn with_capacity(sample_capacity: usize) -> Self {
        // One plus (n - 1) is `max(n, 1)` and is non-zero by construction.
        let capacity = NonZeroUsize::MIN.saturating_add(sample_capacity.saturating_sub(1));
        Self {
            samples: RingBuffer::sized(capacity),
        }
    }

    /// Record one nanosecond sample.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::LatencyTracker;
    /// let mut tracker = LatencyTracker::new();
    /// tracker.record(2_000);
    /// assert_eq!(tracker.p99_microseconds(), 2);
    /// ```
    pub fn record(&mut self, nanoseconds: u64) {
        self.samples.push(nanoseconds);
    }

    /// p99 latency in whole microseconds; 0 when no sample exists. The index
    /// is floor((n-1)*0.99) computed in integers.
    /// # Examples
    /// ```
    /// use drishti::native_host::telemetry::LatencyTracker;
    /// let mut tracker = LatencyTracker::new();
    /// (1..=100).for_each(|micros| tracker.record(micros * 1_000));
    /// // floor((100 - 1) * 0.99) = 98, the 99th smallest sample.
    /// assert_eq!(tracker.p99_microseconds(), 99);
    /// ```
    #[must_use]
    pub fn p99_microseconds(&self) -> u64 {
        let mut sorted = self.samples.snapshot();
        if sorted.is_empty() {
            return 0;
        }
        sorted.sort_unstable();
        let last = sorted.len().saturating_sub(1);
        let index = last.wrapping_mul(99).wrapping_div(100);
        sorted[index].wrapping_div(1_000)
    }
}

impl Default for LatencyTracker {
    /// Tracker with the default 1,000-sample ring.
    fn default() -> Self {
        Self::new()
    }
}
