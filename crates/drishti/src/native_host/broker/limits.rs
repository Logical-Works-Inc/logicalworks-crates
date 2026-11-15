//! Numeric bounds for the broker registries and the typed refusal past them.
//!
//! Every registry a caller can grow has a global bound and, where one tenant
//! could starve the rest, a per-tenant bound. Past a bound the broker refuses
//! with [`BrokerError::LimitExceeded`] and leaves state untouched; revoking an
//! entry frees its slot again. The audit sink is the broker's only other buffer and
//! it owns none: events are handed to the injected sink inline. A signal
//! payload is bounded by its serialized size ([`Limit::PayloadBytes`]).

use std::fmt;

use super::super::ensure;
use super::super::protocol::JsonValue;
use super::BrokerError;

/// Widgets across every tenant (also bounds source ids and rate buckets).
const MAX_WIDGETS: usize = 1_024;
/// Widgets one tenant may hold.
const MAX_WIDGETS_PER_TENANT: usize = 128;
/// ML peers across every tenant (also bounds their sessions).
const MAX_ML_PEERS: usize = 64;
/// ML peers one tenant may hold.
const MAX_ML_PEERS_PER_TENANT: usize = 8;
/// Subscriptions across every tenant.
const MAX_SUBSCRIPTIONS: usize = 4_096;
/// Subscriptions one tenant may hold.
const MAX_SUBSCRIPTIONS_PER_TENANT: usize = 512;
/// Tap listeners across every tenant.
const MAX_TAPS: usize = 64;
/// Tap listeners one tenant may hold.
const MAX_TAPS_PER_TENANT: usize = 8;
/// Serialized bytes of one signal payload (64 KiB).
const MAX_PAYLOAD_BYTES: usize = 65_536;
/// Serialized bytes of the longest number (`-1.7976931348623157e308`).
const NUMBER_BYTES: usize = 24;

/// The bound a refusal names.
/// # Examples
/// ```
/// use drishti::native_host::broker::Limit;
/// assert_eq!(Limit::PayloadBytes.to_string(), "payload bytes (bound 65536)");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Limit {
    /// Widgets across every tenant.
    Widgets,
    /// Widgets in one tenant.
    WidgetsPerTenant,
    /// ML peers across every tenant.
    MlPeers,
    /// ML peers in one tenant.
    MlPeersPerTenant,
    /// Subscriptions across every tenant.
    Subscriptions,
    /// Subscriptions in one tenant.
    SubscriptionsPerTenant,
    /// Tap listeners across every tenant.
    Taps,
    /// Tap listeners in one tenant.
    TapsPerTenant,
    /// Serialized bytes of one signal payload.
    PayloadBytes,
}

impl Limit {
    /// The numeric bound: the registry refuses once it holds this many, and a
    /// payload is refused once it serializes past this many bytes.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::Limit;
    /// assert_eq!(Limit::TapsPerTenant.cap(), 8);
    /// assert!(Limit::TapsPerTenant.cap() < Limit::Taps.cap());
    /// ```
    #[must_use]
    pub const fn cap(self) -> usize {
        match self {
            Self::Widgets => MAX_WIDGETS,
            Self::WidgetsPerTenant => MAX_WIDGETS_PER_TENANT,
            Self::MlPeers => MAX_ML_PEERS,
            Self::MlPeersPerTenant => MAX_ML_PEERS_PER_TENANT,
            Self::Subscriptions => MAX_SUBSCRIPTIONS,
            Self::SubscriptionsPerTenant => MAX_SUBSCRIPTIONS_PER_TENANT,
            Self::Taps => MAX_TAPS,
            Self::TapsPerTenant => MAX_TAPS_PER_TENANT,
            Self::PayloadBytes => MAX_PAYLOAD_BYTES,
        }
    }
}

impl fmt::Display for Limit {
    /// The registry name and its bound.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::Widgets => "widgets",
            Self::WidgetsPerTenant => "widgets per tenant",
            Self::MlPeers => "ML peers",
            Self::MlPeersPerTenant => "ML peers per tenant",
            Self::Subscriptions => "subscriptions",
            Self::SubscriptionsPerTenant => "subscriptions per tenant",
            Self::Taps => "taps",
            Self::TapsPerTenant => "taps per tenant",
            Self::PayloadBytes => "payload bytes",
        };
        write!(formatter, "{name} (bound {})", self.cap())
    }
}

/// Refuse once `used` entries already fill `limit`.
pub(super) fn within(used: usize, limit: Limit) -> Result<(), BrokerError> {
    ensure(used < limit.cap(), || BrokerError::LimitExceeded(limit))
}

/// Refuse once the walked payload `used` bytes pass `cap`.
fn within_cap(used: usize, cap: usize) -> Result<(), BrokerError> {
    ensure(used <= cap, || {
        BrokerError::LimitExceeded(Limit::PayloadBytes)
    })
}

/// Count the entries owned by `tenant`.
pub(super) fn tenant_count<'owners>(
    owners: impl Iterator<Item = &'owners str>,
    tenant: &str,
) -> usize {
    owners.filter(|owner| *owner == tenant).count()
}

/// Serialized bytes of `text` as a JSON string, quotes and escapes included.
fn string_bytes(text: &str) -> usize {
    text.chars().fold(2_usize, |total, character| {
        let width = match character {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            control if control < ' ' => 6,
            other => other.len_utf8(),
        };
        total.saturating_add(width)
    })
}

/// Refuse a payload whose JSON text would pass [`Limit::PayloadBytes`]. The
/// walk keeps its own stack and stops at the first byte over the bound, so a
/// hostile shape costs at most the bound in steps and recursion never grows
/// with the nesting.
pub(super) fn payload_within(payload: &JsonValue) -> Result<(), BrokerError> {
    let cap = Limit::PayloadBytes.cap();
    let mut used = 0_usize;
    let mut pending = vec![payload];
    while let Some(next) = pending.pop() {
        let own = match *next {
            JsonValue::Null | JsonValue::Bool(true) => 4,
            JsonValue::Bool(false) => 5,
            JsonValue::Number(_) => NUMBER_BYTES,
            JsonValue::String(ref text) => string_bytes(text),
            JsonValue::Array(ref items) => {
                pending.extend(items.iter());
                items.len().saturating_sub(1).saturating_add(2)
            }
            JsonValue::Object(ref members) => {
                let mut keys = members.len().saturating_sub(1).saturating_add(2);
                for (key, member) in members {
                    keys = keys.saturating_add(string_bytes(key).saturating_add(1));
                    pending.push(member);
                }
                keys
            }
        };
        used = used.saturating_add(own);
        within_cap(used, cap)?;
    }
    Ok(())
}
