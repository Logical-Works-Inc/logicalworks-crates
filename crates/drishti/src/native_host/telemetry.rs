//! Telemetry: ring buffer, latency tracking, tap, and tape reader.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasTelemetry` (kernel#850
//! row 15). The Swift tap was an actor with asynchronous governance appends
//! and explicit in-flight draining; this port appends synchronously inside the
//! tap callback, and the tap state lock is released before that append runs:
//! an append already in flight when `stop` returns may still land, and no
//! append starts after it. Ring semantics (overwrite-oldest, oldest-first
//! snapshots, total pushed) and the p99 definition (floor((n-1)*0.99), whole
//! microseconds, 0 when empty) match exactly. The ring, the latency tracker
//! and the metrics snapshot live in `ring`, split for the size law.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use super::broker::{
    AuditAction, Broker, BrokerError, Reach, RegisteredHandle, Scope, Subject, TapEvent, TapId,
};
use super::capability::assert_capability;
use super::ensure;
use super::governance::{GovernanceError, GovernanceQuery, GovernanceStore};
use super::protocol::{Attribution, AttributionKind, GovernanceEntry, SignalEnvelope, SourceKind};
use super::time::format_millis;
use crate::canvas_protocol::Capability;

/// Bounded ring, latency tracker and metrics snapshot (split for the size law).
mod ring;

pub use ring::{LatencyTracker, RingBuffer, TelemetryMetrics};

/// Telemetry failures.
/// # Examples
/// ```
/// use drishti::native_host::telemetry::{RingBuffer, TelemetryError};
/// assert_eq!(RingBuffer::<u8>::new(0).err(), Some(TelemetryError::InvalidCapacity(0)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, lgwks_macros::Error)]
#[non_exhaustive]
pub enum TelemetryError {
    /// A governance append failed (or no entry id could be minted), with the
    /// cause attached. The tap counts it and keeps it as
    /// [`TelemetryTap::last_error`]; it never fails the emit it observes.
    #[error("telemetry append failed: {0}")]
    AppendFailed(String),
    /// Ring capacity must be at least one.
    #[error("invalid ring capacity: {0}")]
    InvalidCapacity(usize),
    /// Ring capacity is past [`MAX_RING_CAPACITY`].
    #[error("ring capacity {0} is past the bound")]
    CapacityTooLarge(usize),
    /// `stop` could not release the broker tap, with the broker's refusal.
    #[error("tap release refused: {0}")]
    UntapRefused(BrokerError),
}

/// Most envelopes one tap ring may hold (also the default).
/// # Examples
/// ```
/// use drishti::native_host::telemetry::MAX_RING_CAPACITY;
/// assert_eq!(MAX_RING_CAPACITY, 5_000);
/// ```
pub const MAX_RING_CAPACITY: usize = 5_000;

/// Subscribes to the broker tap stream, buffers every routed envelope in a
/// fixed-capacity ring, and appends each envelope to the governance store.
///
/// Starting requires the `signal.subscribe` capability. Start and stop are
/// idempotent. Appends run synchronously in the tap callback, outside the
/// state lock: `stop` ends new appends, and an append already in flight may
/// still land.
/// # Examples
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::governance::GovernanceStore;
/// use drishti::native_host::telemetry::{TelemetryError, TelemetryTap};
/// let store = GovernanceStore::open_in_memory().map_err(|e| TelemetryError::AppendFailed(e.to_string()))?;
/// let tap = TelemetryTap::new(store, vec![Capability::SignalSubscribe], 16)?;
/// assert_eq!(tap.metrics().emits_observed(), 0);
/// # Ok::<(), TelemetryError>(())
/// ```
#[derive(Clone)]
pub struct TelemetryTap {
    /// Shared tap state; the broker listener holds one clone.
    inner: Arc<Mutex<TapInner>>,
}

/// Tap state behind the handle lock.
struct TapInner {
    /// Buffered envelopes.
    ring: RingBuffer<SignalEnvelope>,
    /// Callback latency samples.
    latency: LatencyTracker,
    /// Tap events observed.
    emits_observed: u64,
    /// Governance appends that failed.
    append_errors: u64,
    /// Whether the tap is subscribed.
    running: bool,
    /// Broker tap identity, while subscribed.
    tap_id: Option<TapId>,
    /// Widget that owns the broker tap, while subscribed.
    owner: Option<RegisteredHandle>,
    /// Governance sink.
    governance: GovernanceStore,
    /// Granted capabilities for the start gate.
    granted: Vec<Capability>,
    /// The most recent failure the tap absorbed (one slot, overwritten).
    last_error: Option<TelemetryError>,
}

impl TapInner {
    /// Count one failed append and keep its cause.
    fn fail(&mut self, error: TelemetryError) {
        self.append_errors = self.append_errors.wrapping_add(1);
        self.last_error = Some(error);
    }
}

impl TelemetryTap {
    /// Build a tap over a governance store. The ring holds at most
    /// [`MAX_RING_CAPACITY`] (5,000) envelopes.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::InvalidCapacity`] for a zero ring capacity
    /// and [`TelemetryError::CapacityTooLarge`] past the bound.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::{MAX_RING_CAPACITY, TelemetryError, TelemetryTap};
    /// let store = GovernanceStore::open_in_memory().map_err(|e| TelemetryError::AppendFailed(e.to_string()))?;
    /// let over = TelemetryTap::new(store, vec![], MAX_RING_CAPACITY + 1).err();
    /// assert_eq!(over, Some(TelemetryError::CapacityTooLarge(MAX_RING_CAPACITY + 1)));
    /// # Ok::<(), TelemetryError>(())
    /// ```
    pub fn new(
        governance: GovernanceStore,
        granted: Vec<Capability>,
        ring_capacity: usize,
    ) -> Result<Self, TelemetryError> {
        ensure(ring_capacity <= MAX_RING_CAPACITY, || {
            TelemetryError::CapacityTooLarge(ring_capacity)
        })?;
        Ok(Self {
            inner: Arc::new(Mutex::new(TapInner {
                ring: RingBuffer::new(ring_capacity)?,
                latency: LatencyTracker::new(),
                emits_observed: 0,
                append_errors: 0,
                running: false,
                tap_id: None,
                owner: None,
                governance,
                granted,
                last_error: None,
            })),
        })
    }

    /// Lock tap state, recovering from poisoning by keeping the state.
    fn lock(&self) -> MutexGuard<'_, TapInner> {
        lock_inner(&self.inner)
    }

    /// Subscribe to the broker tap as `caller`, a registered widget: the tap
    /// hears the caller's own session and tenant only. Requires
    /// `signal.subscribe`. Double start is idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::CapabilityDenied`] without the capability
    /// (audited as a denied tap registration), and the broker's refusal when
    /// the tap is not admitted (the tap is then not running and may be
    /// started again).
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::TelemetryTap;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let session = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &session)?;
    /// let tap = TelemetryTap::new(GovernanceStore::open_in_memory()?, vec![Capability::SignalSubscribe], 8)?;
    /// tap.start(&broker, &owner)?;
    /// tap.start(&broker, &owner)?; // a second start is a no-op
    /// // Without signal.subscribe the start is denied.
    /// let denied = TelemetryTap::new(GovernanceStore::open_in_memory()?, vec![], 8)?;
    /// assert!(denied.start(&broker, &owner).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn start(&self, broker: &Broker, caller: &RegisteredHandle) -> Result<(), BrokerError> {
        let admitted = {
            // One guard decides and flips `running`, so two starts racing
            // cannot both subscribe.
            let mut tap = self.lock();
            if tap.running {
                return Ok(());
            }
            assert_capability(&tap.granted, Capability::SignalSubscribe).map(|()| {
                tap.running = true;
            })
        };
        admitted.map_err(|refusal| {
            let denial = BrokerError::from(refusal);
            let action = AuditAction::Register(Subject::Tap);
            broker.deny_handle(caller, action, &denial);
            denial
        })?;
        let inner = Arc::clone(&self.inner);
        let tapped = broker.tap(
            caller,
            Reach::Own,
            Arc::new(move |event: TapEvent| {
                handle_tap_event(&inner, &event);
            }),
        );
        let mut tap = self.lock();
        match tapped {
            Ok(id) => {
                tap.tap_id = Some(id);
                tap.owner = Some(caller.clone());
                Ok(())
            }
            Err(error) => {
                tap.running = false;
                Err(error)
            }
        }
    }

    /// Unsubscribe from the broker tap. Idempotent; after return, no new
    /// governance write starts for this tap.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::TelemetryTap;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let session = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &session)?;
    /// let tap = TelemetryTap::new(GovernanceStore::open_in_memory()?, vec![Capability::SignalSubscribe], 8)?;
    /// tap.start(&broker, &owner)?;
    /// tap.stop(&broker);
    /// tap.stop(&broker); // idempotent
    /// assert!(tap.last_error().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(&self, broker: &Broker) {
        let mut tap = self.lock();
        if !tap.running {
            return;
        }
        tap.running = false;
        let (id, owner) = (tap.tap_id.take(), tap.owner.take());
        drop(tap);
        if let (Some(id), Some(owner)) = (id, owner)
            && let Err(refusal) = broker.untap(&owner, &id)
        {
            // The only refusal is an unknown tap: an owner that already
            // unregistered took the tap with it, so nothing is left to
            // release. The broker audited the denial; the tap keeps it too.
            self.lock().last_error = Some(TelemetryError::UntapRefused(refusal));
        }
    }

    /// The most recent failure the tap absorbed: a failed governance append
    /// or a refused release. `None` when the tap has had none.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::TelemetryTap;
    /// let tap = TelemetryTap::new(GovernanceStore::open_in_memory()?, vec![], 4)?;
    /// assert!(tap.last_error().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn last_error(&self) -> Option<TelemetryError> {
        self.lock().last_error.clone()
    }

    /// Copy of the ring contents, oldest first. With a session id, only
    /// envelopes from that session are returned.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::TelemetryTap;
    /// let tap = TelemetryTap::new(GovernanceStore::open_in_memory()?, vec![], 4)?;
    /// assert!(tap.snapshot(None).is_empty());
    /// assert!(tap.snapshot(Some("session-a")).is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn snapshot(&self, session_id: Option<&str>) -> Vec<SignalEnvelope> {
        let all = self.lock().ring.snapshot();
        match session_id {
            None => all,
            Some(session) => all
                .into_iter()
                .filter(|envelope| envelope.session_invariant().session_id() == session)
                .collect(),
        }
    }

    /// Point-in-time health metrics.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::TelemetryTap;
    /// let tap = TelemetryTap::new(GovernanceStore::open_in_memory()?, vec![], 4)?;
    /// let metrics = tap.metrics();
    /// assert_eq!((metrics.emits_observed(), metrics.append_errors()), (0, 0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn metrics(&self) -> TelemetryMetrics {
        let tap = self.lock();
        TelemetryMetrics::new(
            tap.emits_observed,
            tap.append_errors,
            tap.latency.p99_microseconds(),
        )
    }
}

/// Lock tap state, recovering from poisoning by keeping the state.
fn lock_inner(inner: &Mutex<TapInner>) -> MutexGuard<'_, TapInner> {
    match inner.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Handle one tap event: ring push plus a synchronous governance append. A
/// failing append is counted and kept as the tap's last error, never
/// propagated, so the tap cannot fail the broker emit it observes. The state
/// lock is released before the append runs, so a slow store never blocks
/// `metrics`, `snapshot`, or `stop`.
fn handle_tap_event(inner: &Arc<Mutex<TapInner>>, event: &TapEvent) {
    let started = Instant::now();
    let mut tap = lock_inner(inner);
    if !tap.running {
        return;
    }
    tap.ring.push(event.envelope().clone());
    tap.emits_observed = tap.emits_observed.wrapping_add(1);
    // O(1) tap-callback latency ends here (ring push plus count), matching
    // the kernel's `tapLatencyP99Microseconds`: the governance append below
    // is synchronous dispatch, not callback work, and stays untimed.
    let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX));
    // The value was just clamped to `u64::MAX`, so the low eight bytes are
    // the whole value, with no conversion to fail.
    let elapsed_bytes = elapsed.to_le_bytes();
    tap.latency.record(u64::from_le_bytes([
        elapsed_bytes[0],
        elapsed_bytes[1],
        elapsed_bytes[2],
        elapsed_bytes[3],
        elapsed_bytes[4],
        elapsed_bytes[5],
        elapsed_bytes[6],
        elapsed_bytes[7],
    ]));
    let entry = match governance_entry_from_envelope(event.envelope(), event.timestamp_ms()) {
        Ok(entry) => entry,
        Err(error) => {
            // No entry id, no append: the observation still counts above, and
            // the failed write counts here, so the tap cannot fail the broker
            // emit it observes.
            tap.fail(TelemetryError::AppendFailed(format!(
                "no entry id: {error:?}"
            )));
            return;
        }
    };
    let governance = tap.governance.clone();
    drop(tap);
    let policy = event
        .envelope()
        .session_invariant()
        .policy_bundle_id()
        .to_owned();
    if let Err(error) = governance.append_with_policy(&entry, policy) {
        lock_inner(inner).fail(TelemetryError::AppendFailed(format!("{error}")));
    }
}

/// Build a governance entry from a tap envelope through typed construction
/// (no string interpolation into JSON). The entry id is fresh per tap event
/// and `recorded_at` is the tap receipt time, so re-tapped envelopes stay
/// unique and replayable under a frozen clock.
///
/// # Errors
///
/// Returns the estate entropy failure when no fresh entry id can be minted.
fn governance_entry_from_envelope(
    envelope: &SignalEnvelope,
    tap_now_ms: u64,
) -> Result<GovernanceEntry, lgwks_std::random::EntropyError> {
    let mut entry = GovernanceEntry::new(
        envelope.session_invariant().session_id().to_owned(),
        lgwks_std::id::Uuid::new_v4()?.to_string(),
        envelope.session_invariant().work_object_id().to_owned(),
        envelope.signal_id().to_owned(),
        envelope.kind().to_owned(),
        String::from("telemetry tap capture"),
        Attribution::new(
            map_source_kind(envelope.source_kind()),
            envelope.source_id().to_owned(),
            None,
            None,
        ),
    );
    entry.set_recorded_at(format_millis(tap_now_ms));
    entry.set_tenant_id(envelope.session_invariant().tenant_id().to_owned());
    Ok(entry)
}

/// Map a signal origin to its attribution kind.
fn map_source_kind(kind: SourceKind) -> AttributionKind {
    match kind {
        SourceKind::Widget => AttributionKind::Widget,
        SourceKind::Ml => AttributionKind::Ml,
        SourceKind::System => AttributionKind::System,
        SourceKind::User => AttributionKind::User,
    }
}

/// Scoped tape reader over a governance store.
/// # Examples
/// ```
/// use drishti::native_host::governance::GovernanceStore;
/// use drishti::native_host::telemetry::TapeReader;
/// let reader = TapeReader::new(GovernanceStore::open_in_memory()?);
/// assert!(format!("{reader:?}").starts_with("TapeReader"));
/// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
/// ```
#[derive(Debug, Clone)]
pub struct TapeReader {
    /// Store to read from.
    store: GovernanceStore,
}

impl TapeReader {
    /// Wrap a store for replay reads.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// use drishti::native_host::telemetry::TapeReader;
    /// let reader = TapeReader::new(GovernanceStore::open_in_memory()?);
    /// assert!(format!("{reader:?}").starts_with("TapeReader"));
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub fn new(store: GovernanceStore) -> Self {
        Self { store }
    }

    /// Up to `limit` most-recent entries for the scope's session in the
    /// scope's tenant over the last `window_hours` (clamped to the 24-hour
    /// query bound), newest last. Another tenant's entries are never read,
    /// even under the same session id, and the limit runs inside the store.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Unbounded`] for an empty tenant or session
    /// and [`GovernanceError::Store`] for backend failures.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::Scope;
    /// use drishti::native_host::governance::{GovernanceError, GovernanceStore};
    /// use drishti::native_host::telemetry::TapeReader;
    /// let reader = TapeReader::new(GovernanceStore::open_in_memory()?);
    /// assert!(reader.recent_entries(&Scope::new("tenant-a", "sess"), 10, 1.0, 5_000)?.is_empty());
    /// // An empty tenant is an unbounded read and is refused.
    /// let refused = reader.recent_entries(&Scope::new("", "sess"), 10, 1.0, 5_000);
    /// assert_eq!(refused, Err(GovernanceError::Unbounded));
    /// # Ok::<(), GovernanceError>(())
    /// ```
    pub fn recent_entries(
        &self,
        scope: &Scope,
        limit: usize,
        window_hours: f64,
        now_ms: u64,
    ) -> Result<Vec<super::governance::GovernanceEntrySnapshot>, GovernanceError> {
        let clamped_window = if window_hours.is_finite() && window_hours > 0.0 {
            window_hours.min(24.0)
        } else {
            0.0
        };
        let window_millis = millis_from_hours(clamped_window);
        let from_ms = now_ms.saturating_sub(window_millis);
        let query = GovernanceQuery::new(
            String::from(scope.tenant_id()),
            String::from(scope.session_id()),
            from_ms,
            now_ms,
            None,
            None,
        )?;
        self.store.recent(&query, limit)
    }
}

/// Convert hours (fractions included) to whole milliseconds without an `as`
/// cast. Inputs are finite and at most 24 by construction (the caller clamps),
/// so the product is below 2^53 and exact to the millisecond for any
/// multiple of 1/8 hour; the float is scaled first and then truncated toward
/// zero, so a half-hour window is 1,800,000 ms and not zero.
fn millis_from_hours(hours: f64) -> u64 {
    trunc_to_u64(hours * 3_600_000.0)
}

/// Truncate a non-negative finite `f64` toward zero without an `as` cast.
fn trunc_to_u64(hours: f64) -> u64 {
    if !hours.is_finite() || hours <= 0.0 {
        return 0;
    }
    if hours >= 9_007_199_254_740_992.0 {
        return u64::MAX;
    }
    let bits = hours.to_bits();
    let biased = (bits >> 52) & 0x7ff;
    if biased < 1023 {
        return 0;
    }
    let shift = biased.wrapping_sub(1023);
    let mantissa = (bits & 0x000f_ffff_ffff_ffff) | (1_u64 << 52);
    if shift >= 52 {
        // The shift is below 2^10 here, so the low four bytes are the
        // whole amount, with no conversion to fail.
        let amount_bytes = shift.wrapping_sub(52).to_le_bytes();
        let amount = u32::from_le_bytes([
            amount_bytes[0],
            amount_bytes[1],
            amount_bytes[2],
            amount_bytes[3],
        ]);
        mantissa.wrapping_shl(amount)
    } else {
        // The difference is at most 52, so the low four bytes are the
        // whole amount, with no conversion to fail.
        let amount_bytes = 52_u64.wrapping_sub(shift).to_le_bytes();
        let amount = u32::from_le_bytes([
            amount_bytes[0],
            amount_bytes[1],
            amount_bytes[2],
            amount_bytes[3],
        ]);
        mantissa.wrapping_shr(amount)
    }
}
