//! Intent envelopes plus envelope validation.
//!
//! Split from the protocol root for the size law. Values, patterns,
//! descriptors, and attribution stay in the parent module, and prediction
//! envelopes live in `prediction`; this module owns the stamped intent
//! envelope with its source-kind decode gate and the format validation the
//! conformance fixtures pin.

use lgwks_std::json::serde::{Deserialize, Serialize};

use super::super::ensure;
use super::super::time::parse_millis;
use super::{JsonValue, SignalEnvelope, SourceKind};
use crate::canvas_protocol::SessionInvariant;

/// Envelope validation failures.
/// # Examples
/// ```
/// use drishti::native_host::protocol::{EnvelopeError, validate_session_invariant};
/// # use drishti::canvas_protocol::SessionInvariant;
/// let bad: SessionInvariant = lgwks_std::json::from_str(
///     r#"{"work_object_id":"nope","session_id":"22222222-2222-2222-2222-222222222222","policy_bundle_id":"33333333-3333-3333-3333-333333333333","tenant_id":"t","user_sub":"44444444-4444-4444-4444-444444444444"}"#,
/// )?;
/// assert_eq!(validate_session_invariant(&bad), Err(EnvelopeError::InvalidInvariant("work_object_id")));
/// # Ok::<(), lgwks_std::json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, lgwks_macros::Error)]
#[non_exhaustive]
pub enum EnvelopeError {
    /// `signal_id` is not a UUID.
    #[error("signal_id is not a UUID")]
    InvalidSignalId,
    /// `kind` is empty, overlong, dotted badly, or has controls.
    #[error("kind is invalid")]
    InvalidKind,
    /// `emitted_at` is not a strict ISO-8601 timestamp.
    #[error("emitted_at is not a timestamp")]
    InvalidTimestamp,
    /// A session invariant field is invalid; names the field.
    #[error("session invariant field is invalid: {0}")]
    InvalidInvariant(&'static str),
}

/// Validate a signal kind: non-empty, at most 128 characters, no
/// leading/trailing dot, no empty segment, no control characters.
/// Character count uses Unicode scalar values; the kernel counted grapheme
/// clusters, which agrees on every fixture and every ASCII kind.
pub(crate) fn valid_signal_kind(kind: &str) -> bool {
    if kind.is_empty() || kind.chars().count() > 128 {
        return false;
    }
    if kind.starts_with('.') || kind.ends_with('.') || kind.contains("..") {
        return false;
    }
    !kind.chars().any(char::is_control)
}

/// Hyphenated UUID validity through the estate parser, which refuses the
/// simple, braced, and URN forms the conformance fixtures
/// (`uuid_no_dashes`, `uuid_with_braces`) pin as invalid signal ids. One
/// shared predicate serves the signal id and the four session invariant
/// UUIDs below.
fn is_valid_uuid(text: &str) -> bool {
    lgwks_std::id::Uuid::parse(text).is_ok()
}

/// Validate a decoded envelope's field formats: UUID signal id, well-formed
/// kind, strict timestamp, and UUID session bindings. Structural decoding
/// (missing fields, wrong JSON types) already fails in serde; this covers
/// the format layer the conformance `invalid_envelopes` fixtures pin.
///
/// # Errors
///
/// Returns the first [`EnvelopeError`] found.
///
/// # Examples
/// ```
/// use drishti::cli::drive::WORK_OBJECT;
/// use drishti::native_host::protocol::{
///     EnvelopeError, JsonValue, SignalEnvelope, SourceKind, validate_envelope,
/// };
/// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
/// let at = "2026-11-03T00:00:00Z";
/// let ok = SignalEnvelope::new(WORK_OBJECT.into(), "forms.submitted".into(), SourceKind::Widget, "w".into(), session.clone(), JsonValue::Null, at.into());
/// assert_eq!(validate_envelope(&ok), Ok(()));
/// let bad = SignalEnvelope::new("not-a-uuid".into(), "k".into(), SourceKind::Widget, "w".into(), session, JsonValue::Null, at.into());
/// assert_eq!(validate_envelope(&bad), Err(EnvelopeError::InvalidSignalId));
/// # Ok::<(), drishti::cli::Failure>(())
/// ```
pub fn validate_envelope(envelope: &SignalEnvelope) -> Result<(), EnvelopeError> {
    ensure(is_valid_uuid(&envelope.signal_id), || {
        EnvelopeError::InvalidSignalId
    })?;
    ensure(valid_signal_kind(&envelope.kind), || {
        EnvelopeError::InvalidKind
    })?;
    // Wire envelopes carry UTC `Z` stamps only: the broker stamps with
    // `format_millis` (always `Z`) and the conformance fixture
    // `invalid_emitted_at_no_z.json` pins `+05:00` as
    // "emitted_at must be ISO 8601 UTC". `parse_millis` still accepts
    // numeric offsets for internal governance reads; admission does not.
    ensure(envelope.emitted_at.ends_with('Z'), || {
        EnvelopeError::InvalidTimestamp
    })?;
    ensure(parse_millis(&envelope.emitted_at).is_ok(), || {
        EnvelopeError::InvalidTimestamp
    })?;
    validate_session_invariant(&envelope.session_invariant)
}

/// Validate session invariant bindings: work object, session, policy, and
/// user are UUIDs; the tenant is a non-empty scope (never a UUID itself in
/// fixtures, but always present).
///
/// # Errors
///
/// Returns [`EnvelopeError::InvalidInvariant`] naming the bad field.
///
/// # Examples
/// ```
/// use drishti::canvas_protocol::SessionInvariant;
/// use drishti::native_host::protocol::{EnvelopeError, validate_session_invariant};
/// // Wire decoding skips the constructor, so an empty tenant reaches here.
/// let wire: SessionInvariant = lgwks_std::json::from_str(
///     r#"{"work_object_id":"11111111-1111-1111-1111-111111111111","session_id":"22222222-2222-2222-2222-222222222222","policy_bundle_id":"33333333-3333-3333-3333-333333333333","tenant_id":"","user_sub":"44444444-4444-4444-4444-444444444444"}"#,
/// )?;
/// assert_eq!(validate_session_invariant(&wire), Err(EnvelopeError::InvalidInvariant("tenant_id")));
/// # Ok::<(), lgwks_std::json::Error>(())
/// ```
pub fn validate_session_invariant(session: &SessionInvariant) -> Result<(), EnvelopeError> {
    let fields = [
        ("work_object_id", is_valid_uuid(session.work_object_id())),
        ("session_id", is_valid_uuid(session.session_id())),
        (
            "policy_bundle_id",
            is_valid_uuid(session.policy_bundle_id()),
        ),
        ("user_sub", is_valid_uuid(session.user_sub())),
        ("tenant_id", !session.tenant_id().is_empty()),
    ];
    fields
        .into_iter()
        .try_for_each(|(name, valid)| ensure(valid, || EnvelopeError::InvalidInvariant(name)))
}

/// Partial intent supplied by the command bar before broker stamping.
///
/// Fields are private: construction runs through [`PartialIntent::new`] and
/// reads go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::protocol::{JsonValue, PartialIntent};
/// let intent = PartialIntent::new("intent.find".into(), JsonValue::Null, "acme".into());
/// assert_eq!(intent.kind(), "intent.find");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct PartialIntent {
    /// Intent kind.
    kind: String,
    /// Intent payload.
    payload: JsonValue,
    /// Raw command text.
    text: String,
}

impl PartialIntent {
    /// Assemble a partial intent.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PartialIntent};
    /// let intent = PartialIntent::new("intent.pin".into(), JsonValue::Null, "w1".into());
    /// assert_eq!((intent.kind(), intent.text()), ("intent.pin", "w1"));
    /// ```
    #[must_use]
    pub fn new(kind: String, payload: JsonValue, text: String) -> Self {
        Self {
            kind,
            payload,
            text,
        }
    }

    /// Intent kind.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PartialIntent};
    /// assert_eq!(PartialIntent::new("k".into(), JsonValue::Null, "t".into()).kind(), "k");
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Intent payload.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PartialIntent};
    /// let intent = PartialIntent::new("k".into(), JsonValue::Bool(true), "t".into());
    /// assert_eq!(intent.payload(), &JsonValue::Bool(true));
    /// ```
    #[must_use]
    pub const fn payload(&self) -> &JsonValue {
        &self.payload
    }

    /// Raw command text.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PartialIntent};
    /// assert_eq!(PartialIntent::new("k".into(), JsonValue::Null, "t".into()).text(), "t");
    /// ```
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// User intent envelope. Decoding rejects any `source_kind` but `user`.
///
/// Fields are private: decoding plus [`IntentEnvelope::new`] construct, and
/// reads go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
/// use drishti::canvas_protocol::SessionInvariant;
/// use drishti::cli::drive::{POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT};
/// let session = SessionInvariant::new(WORK_OBJECT.into(), SESSION_ID.into(), POLICY_BUNDLE.into(), "t".into(), USER_SUB.into())?;
/// let e = IntentEnvelope::new("intent.find".into(), "s".into(), SourceKind::User, "command-bar:user".into(), session, "2026-11-03T00:00:00Z".into(), JsonValue::Null);
/// let wire = lgwks_std::json::to_string(&e)?.replace("\"user\"", "\"widget\"");
/// // Decoding refuses any origin but the user.
/// assert!(lgwks_std::json::from_str::<IntentEnvelope>(&wire).is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct IntentEnvelope {
    /// Envelope identity.
    signal_id: String,
    /// Intent kind.
    kind: String,
    /// Always [`SourceKind::User`]; enforced at decode time.
    #[serde(deserialize_with = "check_user_source")]
    source_kind: SourceKind,
    /// Stamped source id.
    source_id: String,
    /// Intent payload.
    payload: JsonValue,
    /// Session the intent was stamped in.
    session_invariant: SessionInvariant,
    /// Stamp time as ISO-8601.
    emitted_at: String,
    /// Raw command text, if any.
    raw_text: Option<String>,
    /// Resolved kind, if any.
    resolved_kind: Option<String>,
}

/// Gate a decoded `source_kind`: only the user source is admitted here.
fn check_user_source<'de, D>(deserializer: D) -> Result<SourceKind, D::Error>
where
    D: lgwks_std::json::serde::Deserializer<'de>,
{
    let kind = SourceKind::deserialize(deserializer)?;
    ensure(kind == SourceKind::User, || {
        <D::Error as lgwks_std::json::serde::de::Error>::custom(
            "IntentEnvelope sourceKind must be user",
        )
    })?;
    Ok(kind)
}

impl IntentEnvelope {
    /// Assemble an intent envelope. The constructor does not gate the source
    /// kind; decoding does. Optional text defaults to absent;
    /// [`IntentEnvelope::set_raw_text`] and
    /// [`IntentEnvelope::set_resolved_kind`] fill it, keeping this
    /// constructor at the seven-argument clipping bound.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
    /// let e = IntentEnvelope::new("intent.find".into(), "s".into(), SourceKind::User, "bar".into(), session, "2026-11-03T00:00:00Z".into(), JsonValue::Null);
    /// assert_eq!((e.kind(), e.signal_id(), e.raw_text(), e.resolved_kind()), ("intent.find", "s", None, None));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn new(
        kind: String,
        signal_id: String,
        source_kind: SourceKind,
        source_id: String,
        session_invariant: SessionInvariant,
        emitted_at: String,
        payload: JsonValue,
    ) -> Self {
        Self {
            kind,
            signal_id,
            source_kind,
            source_id,
            session_invariant,
            emitted_at,
            payload,
            raw_text: None,
            resolved_kind: None,
        }
    }

    /// Attach the raw command text, if any.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let mut e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// e.set_raw_text(Some("/find acme".into()));
    /// assert_eq!(e.raw_text().map(String::as_str), Some("/find acme"));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn set_raw_text(&mut self, raw_text: Option<String>) {
        self.raw_text = raw_text;
    }

    /// Attach the resolved kind, if any.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let mut e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// e.set_resolved_kind(Some("intent.find".into()));
    /// assert_eq!(e.resolved_kind().map(String::as_str), Some("intent.find"));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn set_resolved_kind(&mut self, resolved_kind: Option<String>) {
        self.resolved_kind = resolved_kind;
    }

    /// Unique id the broker stamped on this intent.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// assert_eq!(e.signal_id(), "s");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    /// Dotted intent kind, such as `intent.find`.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("intent.pin".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// assert_eq!(e.kind(), "intent.pin");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Intent payload.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Bool(true));
    /// assert_eq!(e.payload(), &JsonValue::Bool(true));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn payload(&self) -> &JsonValue {
        &self.payload
    }

    /// Stamp time as ISO-8601.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "2026-11-03T00:00:00Z".into(), JsonValue::Null);
    /// assert_eq!(e.emitted_at(), "2026-11-03T00:00:00Z");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn emitted_at(&self) -> &str {
        &self.emitted_at
    }

    /// Session the intent was stamped in.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "tenant-a")?, "t".into(), JsonValue::Null);
    /// assert_eq!(e.session_invariant().tenant_id(), "tenant-a");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn session_invariant(&self) -> &SessionInvariant {
        &self.session_invariant
    }

    /// Stamped source id.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "command-bar:user".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// assert_eq!(e.source_id(), "command-bar:user");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Stamped origin (always [`SourceKind::User`] after decode).
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// assert_eq!(e.source_kind(), SourceKind::User);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn source_kind(&self) -> SourceKind {
        self.source_kind
    }

    /// Raw command text, if any.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// assert!(e.raw_text().is_none());
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn raw_text(&self) -> Option<&String> {
        self.raw_text.as_ref()
    }

    /// Resolved kind, if any.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{IntentEnvelope, JsonValue, SourceKind};
    /// # let e = IntentEnvelope::new("k".into(), "s".into(), SourceKind::User, "bar".into(), drishti::cli::drive::session(&mut String::new(), "t")?, "t".into(), JsonValue::Null);
    /// assert!(e.resolved_kind().is_none());
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn resolved_kind(&self) -> Option<&String> {
        self.resolved_kind.as_ref()
    }
}
