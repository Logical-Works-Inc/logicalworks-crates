//! Native-host protocol types: envelopes, patterns, descriptors, entries.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasProtocol/generated`
//! (kernel#850 row 15): JSON values, source kinds, signal envelopes and
//! patterns, widget descriptors, governance entries, and the intent and
//! prediction envelopes with their source-kind decode gates.
//!
//! Split for the size law: widget descriptors live in `descriptor`,
//! governance entries and attribution in `entries`, subscriber patterns and
//! predicates in `patterns`, intent/prediction envelopes and envelope
//! validation in `envelopes`; this root owns values and the signal envelope.

use std::collections::BTreeMap;

use lgwks_std::json::serde::{Deserialize, Serialize};

use crate::canvas_protocol::SessionInvariant;

/// Widget registration descriptor (split for the size law).
mod descriptor;
/// Governance entries and attribution (split for the size law).
mod entries;
/// Intent/prediction envelopes and validation (split for the size law).
mod envelopes;
/// Subscriber patterns and payload predicates (split for the size law).
mod patterns;
/// ML prediction envelope (split for the size law).
mod prediction;

pub use descriptor::{WidgetDescriptor, WidgetSurface};
pub use entries::{Attribution, AttributionKind, GovernanceEntry};
pub(crate) use envelopes::valid_signal_kind;
pub use envelopes::{
    EnvelopeError, IntentEnvelope, PartialIntent, validate_envelope, validate_session_invariant,
};
pub use patterns::{PayloadPredicate, SignalPattern};
pub use prediction::PredictionEnvelope;

/// Envelope origin.
/// # Examples
/// ```
/// use drishti::native_host::protocol::SourceKind;
/// assert_eq!(SourceKind::all()[3], SourceKind::User);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A registered widget.
    Widget,
    /// A verified ML peer.
    Ml,
    /// The system substrate.
    System,
    /// The command-bar user.
    User,
}

impl SourceKind {
    /// All four source kinds in declaration order.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::SourceKind;
    /// assert_eq!(SourceKind::all().len(), 4);
    /// ```
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Widget, Self::Ml, Self::System, Self::User]
    }
}

/// Type-safe JSON value. Decode order mirrors the kernel: null, bool,
/// number, string, array, object — so booleans never decode as numbers.
/// # Examples
/// ```
/// use drishti::native_host::protocol::JsonValue;
/// let decoded: JsonValue = lgwks_std::json::from_str("true")?;
/// assert_eq!(decoded, JsonValue::Bool(true));
/// # Ok::<(), lgwks_std::json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
#[serde(untagged)]
pub enum JsonValue {
    /// Text.
    String(String),
    /// Number as `f64` (conformance payloads are small; integers beyond 2^53
    /// lose precision here, which the matcher documents).
    Number(f64),
    /// Boolean, never confused with a number.
    Bool(bool),
    /// Null.
    Null,
    /// Array.
    Array(Vec<JsonValue>),
    /// Object in key order.
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    /// Borrow the string, if this is one.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::JsonValue;
    /// assert_eq!(JsonValue::String("a".into()).string_value().map(String::as_str), Some("a"));
    /// assert!(JsonValue::Null.string_value().is_none());
    /// ```
    #[must_use]
    pub const fn string_value(&self) -> Option<&String> {
        match *self {
            Self::String(ref text) => Some(text),
            _ => None,
        }
    }

    /// Borrow the array, if this is one.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::JsonValue;
    /// assert_eq!(JsonValue::Array(vec![JsonValue::Null]).array_value().map(Vec::len), Some(1));
    /// assert!(JsonValue::Null.array_value().is_none());
    /// ```
    #[must_use]
    pub const fn array_value(&self) -> Option<&Vec<JsonValue>> {
        match *self {
            Self::Array(ref items) => Some(items),
            _ => None,
        }
    }

    /// Borrow the object map, if this is one.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::JsonValue;
    /// assert!(JsonValue::Object(Default::default()).object_value().is_some());
    /// assert!(JsonValue::Null.object_value().is_none());
    /// ```
    #[must_use]
    pub const fn object_value(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match *self {
            Self::Object(ref map) => Some(map),
            _ => None,
        }
    }

    /// Convert an estate JSON value. Integers become the nearest `f64`;
    /// every other shape maps exactly. Unrepresentable numbers have no
    /// number to carry: without arbitrary precision `as_f64` always
    /// represents them, so the null below documents the unreachable arm
    /// as absence rather than trapping or fabricating a zero.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::JsonValue;
    /// let estate: lgwks_std::json::Value = lgwks_std::json::from_str("[1, null]")?;
    /// assert_eq!(
    ///     JsonValue::from_json(&estate),
    ///     JsonValue::Array(vec![JsonValue::Number(1.0), JsonValue::Null])
    /// );
    /// # Ok::<(), lgwks_std::json::Error>(())
    /// ```
    #[must_use]
    pub fn from_json(value: &lgwks_std::json::Value) -> Self {
        match *value {
            lgwks_std::json::Value::Null => Self::Null,
            lgwks_std::json::Value::Bool(flag) => Self::Bool(flag),
            lgwks_std::json::Value::Number(ref number) => {
                let Some(value) = number.as_f64() else {
                    return Self::Null;
                };
                Self::Number(value)
            }
            lgwks_std::json::Value::String(ref text) => Self::String(String::from(text.as_str())),
            lgwks_std::json::Value::Array(ref items) => {
                Self::Array(items.iter().map(Self::from_json).collect())
            }
            lgwks_std::json::Value::Object(ref map) => Self::Object(
                map.iter()
                    .map(|(key, item)| (String::from(key), Self::from_json(item)))
                    .collect(),
            ),
            // The estate enum is `#[non_exhaustive]`: a shape added after
            // this port has no counterpart here and reads as absence.
            _ => Self::Null,
        }
    }
}

/// One stamped signal on the bus.
///
/// Fields are private: only the broker stamps envelopes (tests build fixtures
/// through [`SignalEnvelope::new`]), and every reader goes through the
/// accessors. Wire input additionally passes [`validate_envelope`].
/// # Examples
/// ```
/// use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
/// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
/// let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), session, JsonValue::Null, "2026-11-03T00:00:00Z".into());
/// assert_eq!(e.source_kind(), SourceKind::Ml);
/// # Ok::<(), drishti::cli::Failure>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct SignalEnvelope {
    /// Envelope identity.
    signal_id: String,
    /// Signal kind.
    kind: String,
    /// Stamped origin (never caller-supplied).
    source_kind: SourceKind,
    /// Stamped source id (never caller-supplied).
    source_id: String,
    /// Session the signal was stamped in.
    session_invariant: SessionInvariant,
    /// Stamp time as ISO-8601.
    emitted_at: String,
    /// Caller-supplied payload.
    payload: JsonValue,
}

impl SignalEnvelope {
    /// Assemble a stamped envelope. Only the broker stamps; tests use this
    /// to build fixtures.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
    /// let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Widget, "w".into(), session, JsonValue::Null, "2026-11-03T00:00:00Z".into());
    /// assert_eq!((e.signal_id(), e.kind(), e.source_id()), ("s", "k", "w"));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn new(
        signal_id: String,
        kind: String,
        source_kind: SourceKind,
        source_id: String,
        session_invariant: SessionInvariant,
        payload: JsonValue,
        emitted_at: String,
    ) -> Self {
        Self {
            signal_id,
            kind,
            source_kind,
            source_id,
            session_invariant,
            payload,
            emitted_at,
        }
    }

    /// Unique id the broker stamped on this signal at emit time.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Widget, "w".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null, "t".into());
    /// assert_eq!(e.signal_id(), "s");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    /// Dotted signal kind that subscriber globs match against.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "forms.submitted".into(), SourceKind::Widget, "w".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null, "t".into());
    /// assert_eq!(e.kind(), "forms.submitted");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Stamped origin.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::System, "w".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null, "t".into());
    /// assert_eq!(e.source_kind(), SourceKind::System);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn source_kind(&self) -> SourceKind {
        self.source_kind
    }

    /// Stamped source id.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Widget, "widget.a".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null, "t".into());
    /// assert_eq!(e.source_id(), "widget.a");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Session the signal was stamped in.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Widget, "w".into(), drishti::cli::drive::session(&mut String::new(), "tenant-a")?, JsonValue::Null, "t".into());
    /// assert_eq!(e.session_invariant().tenant_id(), "tenant-a");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn session_invariant(&self) -> &SessionInvariant {
        &self.session_invariant
    }

    /// Stamp time as ISO-8601.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Widget, "w".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null, "2026-11-03T00:00:00Z".into());
    /// assert_eq!(e.emitted_at(), "2026-11-03T00:00:00Z");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn emitted_at(&self) -> &str {
        &self.emitted_at
    }

    /// Caller-supplied payload.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SourceKind};
    /// # let e = SignalEnvelope::new("s".into(), "k".into(), SourceKind::Widget, "w".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Bool(true), "t".into());
    /// assert_eq!(e.payload(), &JsonValue::Bool(true));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn payload(&self) -> &JsonValue {
        &self.payload
    }
}
