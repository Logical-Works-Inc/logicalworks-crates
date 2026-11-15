//! ML prediction envelope with its source-kind decode gate.

use lgwks_std::json::serde::{Deserialize, Serialize};

use super::super::ensure;
use super::{JsonValue, SourceKind};
use crate::canvas_protocol::SessionInvariant;

/// ML prediction envelope. Decoding rejects any `source_kind` but `ml`.
///
/// Fields are private: decoding plus [`PredictionEnvelope::new`] construct,
/// and reads go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
/// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
/// let e = PredictionEnvelope::new("s".into(), "ml.suggest".into(), SourceKind::Ml, "m".into(), "2026-11-03T00:00:00Z".into(), session, JsonValue::Null);
/// assert_eq!(e.source_kind(), SourceKind::Ml);
/// # Ok::<(), drishti::cli::Failure>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct PredictionEnvelope {
    /// Envelope identity.
    signal_id: String,
    /// Prediction kind.
    kind: String,
    /// Stamped source id.
    source_id: String,
    /// Always [`SourceKind::Ml`]; enforced at decode time.
    #[serde(deserialize_with = "check_ml_source")]
    source_kind: SourceKind,
    /// Session the prediction was stamped in.
    session_invariant: SessionInvariant,
    /// Prediction payload.
    payload: JsonValue,
    /// Stamp time as ISO-8601.
    emitted_at: String,
    /// Model version.
    model_version: String,
    /// Confidence.
    confidence: f64,
    /// Human-readable reasoning.
    reasoning: String,
    /// Optional view directive.
    view_directive: Option<JsonValue>,
}

/// Gate a decoded `source_kind`: only the ML source is admitted here.
fn check_ml_source<'de, D>(deserializer: D) -> Result<SourceKind, D::Error>
where
    D: lgwks_std::json::serde::Deserializer<'de>,
{
    let kind = SourceKind::deserialize(deserializer)?;
    ensure(kind == SourceKind::Ml, || {
        <D::Error as lgwks_std::json::serde::de::Error>::custom(
            "PredictionEnvelope sourceKind must be ml",
        )
    })?;
    Ok(kind)
}

impl PredictionEnvelope {
    /// Assemble a prediction envelope. The constructor does not gate the
    /// source kind; decoding does. Model detail defaults empty;
    /// [`PredictionEnvelope::set_model_version`],
    /// [`PredictionEnvelope::set_confidence`],
    /// [`PredictionEnvelope::set_reasoning`], and
    /// [`PredictionEnvelope::set_view_directive`] fill it, keeping this
    /// constructor at the seven-argument clipping bound.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
    /// let e = PredictionEnvelope::new("s".into(), "ml.suggest".into(), SourceKind::Ml, "m".into(), "t".into(), session, JsonValue::Null);
    /// assert_eq!((e.model_version(), e.reasoning(), e.view_directive()), ("", "", None));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn new(
        signal_id: String,
        kind: String,
        source_kind: SourceKind,
        source_id: String,
        emitted_at: String,
        session_invariant: SessionInvariant,
        payload: JsonValue,
    ) -> Self {
        Self {
            signal_id,
            kind,
            source_kind,
            source_id,
            emitted_at,
            session_invariant,
            payload,
            model_version: String::new(),
            confidence: 0.0,
            reasoning: String::new(),
            view_directive: None,
        }
    }

    /// Attach the model version.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let mut e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// e.set_model_version("m1".into());
    /// assert_eq!(e.model_version(), "m1");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn set_model_version(&mut self, model_version: String) {
        self.model_version = model_version;
    }

    /// Attach the confidence.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let mut e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// e.set_confidence(0.75);
    /// assert_eq!(e.confidence().to_bits(), 0.75_f64.to_bits());
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn set_confidence(&mut self, confidence: f64) {
        self.confidence = confidence;
    }

    /// Attach the human-readable reasoning.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let mut e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// e.set_reasoning("recent edits".into());
    /// assert_eq!(e.reasoning(), "recent edits");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn set_reasoning(&mut self, reasoning: String) {
        self.reasoning = reasoning;
    }

    /// Attach the optional view directive.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let mut e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// e.set_view_directive(Some(JsonValue::Bool(true)));
    /// assert_eq!(e.view_directive(), Some(&JsonValue::Bool(true)));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn set_view_directive(&mut self, view_directive: Option<JsonValue>) {
        self.view_directive = view_directive;
    }

    /// Unique id the broker stamped on this prediction.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.signal_id(), "s");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    /// Session the prediction was stamped in.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "tenant-a")?, JsonValue::Null);
    /// assert_eq!(e.session_invariant().tenant_id(), "tenant-a");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn session_invariant(&self) -> &SessionInvariant {
        &self.session_invariant
    }

    /// Prediction payload.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Bool(true));
    /// assert_eq!(e.payload(), &JsonValue::Bool(true));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn payload(&self) -> &JsonValue {
        &self.payload
    }

    /// Prediction kind.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "ml.suggest".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.kind(), "ml.suggest");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Stamp time as ISO-8601.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "2026-11-03T00:00:00Z".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.emitted_at(), "2026-11-03T00:00:00Z");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn emitted_at(&self) -> &str {
        &self.emitted_at
    }

    /// Stamped source id.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "peer-1".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.source_id(), "peer-1");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Stamped origin (always [`SourceKind::Ml`] after decode).
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.source_kind(), SourceKind::Ml);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn source_kind(&self) -> SourceKind {
        self.source_kind
    }

    /// Model version.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.model_version(), "");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn model_version(&self) -> &str {
        &self.model_version
    }

    /// Confidence.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.confidence().to_bits(), 0.0_f64.to_bits());
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Human-readable reasoning.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert_eq!(e.reasoning(), "");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn reasoning(&self) -> &str {
        &self.reasoning
    }

    /// Optional view directive.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{JsonValue, PredictionEnvelope, SourceKind};
    /// # let e = PredictionEnvelope::new("s".into(), "k".into(), SourceKind::Ml, "m".into(), "t".into(), drishti::cli::drive::session(&mut String::new(), "t")?, JsonValue::Null);
    /// assert!(e.view_directive().is_none());
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn view_directive(&self) -> Option<&JsonValue> {
        self.view_directive.as_ref()
    }
}
