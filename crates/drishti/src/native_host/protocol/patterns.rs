//! Subscriber patterns and payload predicates.
//!
//! Split from the protocol root for the size law. Envelope, descriptor, and
//! value types stay in the parent module; this module owns the glob plus
//! JSON-logic predicate vocabulary the matcher evaluates.

use std::collections::BTreeMap;

use lgwks_std::json::serde::{Deserialize, Serialize};

use super::JsonValue;

/// Subscriber pattern: dot-segment kind glob plus an optional payload
/// predicate. `*` matches one segment, trailing `**` matches a subtree.
///
/// Fields are private: construction runs through [`SignalPattern::new`] and
/// reads through the accessors, so a decoded pattern cannot be rebound to an
/// unvalidated glob after matching.
/// # Examples
/// ```
/// use drishti::native_host::protocol::SignalPattern;
/// let pattern = SignalPattern::new("forms.**".into(), None);
/// assert_eq!(pattern.kind_glob(), "forms.**");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct SignalPattern {
    /// Kind glob.
    kind_glob: String,
    /// Optional payload predicate.
    payload_predicate: Option<PayloadPredicate>,
}

impl SignalPattern {
    /// Assemble a pattern with an optional predicate.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{PayloadPredicate, SignalPattern};
    /// let predicate = PayloadPredicate::exists("id".into());
    /// let pattern = SignalPattern::new("a.*".into(), Some(predicate.clone()));
    /// assert_eq!(pattern.payload_predicate(), Some(&predicate));
    /// ```
    #[must_use]
    pub fn new(kind_glob: String, payload_predicate: Option<PayloadPredicate>) -> Self {
        Self {
            kind_glob,
            payload_predicate,
        }
    }

    /// Kind glob the pattern matches against.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::SignalPattern;
    /// assert_eq!(SignalPattern::new("a.*".into(), None).kind_glob(), "a.*");
    /// ```
    #[must_use]
    pub fn kind_glob(&self) -> &str {
        &self.kind_glob
    }

    /// Predicate the payload must also satisfy; none means any payload passes.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::SignalPattern;
    /// assert!(SignalPattern::new("a".into(), None).payload_predicate().is_none());
    /// ```
    #[must_use]
    pub const fn payload_predicate(&self) -> Option<&PayloadPredicate> {
        self.payload_predicate.as_ref()
    }
}

/// JSON-logic subset predicate for payload filtering.
/// # Examples
/// ```
/// use drishti::native_host::protocol::PayloadPredicate;
/// let wire: PayloadPredicate = lgwks_std::json::from_str(r#"{"exists": "id"}"#)?;
/// assert_eq!(wire, PayloadPredicate::exists("id".into()));
/// # Ok::<(), lgwks_std::json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PayloadPredicate {
    /// Keypath equals a value.
    Eq {
        /// Dotted keypath.
        keypath: String,
        /// Expected value.
        value: JsonValue,
    },
    /// Keypath differs from a value (missing key is false, not true).
    Neq {
        /// Dotted keypath.
        keypath: String,
        /// Unexpected value.
        value: JsonValue,
    },
    /// Keypath holds one of the listed values.
    In {
        /// Dotted keypath.
        keypath: String,
        /// Candidate values.
        values: Vec<JsonValue>,
    },
    /// Keypath exists, whatever its value.
    Exists {
        /// Dotted keypath.
        keypath: String,
    },
    /// All clauses hold (empty holds).
    And(Vec<PayloadPredicate>),
    /// Any clause holds (empty fails).
    Or(Vec<PayloadPredicate>),
}

impl PayloadPredicate {
    /// Clause that holds when the value at `keypath` equals `value` exactly.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PayloadPredicate};
    /// let clause = PayloadPredicate::eq("a.b".into(), JsonValue::Bool(true));
    /// assert!(matches!(clause, PayloadPredicate::Eq { .. }));
    /// ```
    #[must_use]
    pub fn eq(keypath: String, value: JsonValue) -> Self {
        Self::Eq { keypath, value }
    }

    /// Inequality clause.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PayloadPredicate};
    /// let clause = PayloadPredicate::neq("a".into(), JsonValue::Null);
    /// assert!(matches!(clause, PayloadPredicate::Neq { .. }));
    /// ```
    #[must_use]
    pub fn neq(keypath: String, value: JsonValue) -> Self {
        Self::Neq { keypath, value }
    }

    /// Membership clause.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{JsonValue, PayloadPredicate};
    /// let clause = PayloadPredicate::is_in("a".into(), vec![JsonValue::Number(1.0)]);
    /// assert!(matches!(clause, PayloadPredicate::In { .. }));
    /// ```
    #[must_use]
    pub fn is_in(keypath: String, values: Vec<JsonValue>) -> Self {
        Self::In { keypath, values }
    }

    /// Existence clause.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::PayloadPredicate;
    /// let clause = PayloadPredicate::exists("a".into());
    /// assert!(matches!(clause, PayloadPredicate::Exists { .. }));
    /// ```
    #[must_use]
    pub fn exists(keypath: String) -> Self {
        Self::Exists { keypath }
    }
}

impl Serialize for PayloadPredicate {
    fn serialize<S: lgwks_std::json::serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Eq {
                ref keypath,
                ref value,
            } => {
                let pair = (JsonValue::String(keypath.clone()), value.clone());
                Serialize::serialize(
                    &BTreeMap::from([("eq", JsonValue::Array(vec![pair.0, pair.1]))]),
                    serializer,
                )
            }
            Self::Neq {
                ref keypath,
                ref value,
            } => {
                let pair = (JsonValue::String(keypath.clone()), value.clone());
                Serialize::serialize(
                    &BTreeMap::from([("neq", JsonValue::Array(vec![pair.0, pair.1]))]),
                    serializer,
                )
            }
            Self::In {
                ref keypath,
                ref values,
            } => {
                // The decoder reads `[keypath, [values...]]`; writing the
                // values flat made a saved pattern fail to reload.
                let items = vec![
                    JsonValue::String(keypath.clone()),
                    JsonValue::Array(values.clone()),
                ];
                Serialize::serialize(
                    &BTreeMap::from([("in", JsonValue::Array(items))]),
                    serializer,
                )
            }
            Self::Exists { ref keypath } => Serialize::serialize(
                &BTreeMap::from([("exists", JsonValue::String(keypath.clone()))]),
                serializer,
            ),
            Self::And(ref clauses) => Serialize::serialize(
                &BTreeMap::from([("and", JsonValue::Array(predicate_array(clauses)))]),
                serializer,
            ),
            Self::Or(ref clauses) => Serialize::serialize(
                &BTreeMap::from([("or", JsonValue::Array(predicate_array(clauses)))]),
                serializer,
            ),
        }
    }
}

/// Serialize nested clauses through the same shape.
fn predicate_array(clauses: &[PayloadPredicate]) -> Vec<JsonValue> {
    clauses
        .iter()
        .map(|clause| {
            // Serialization of these shapes is infallible; the string keeps
            // the tree shape, and names the cause, on the unreachable
            // failure arm rather than dropping the clause.
            match lgwks_std::json::to_value(clause) {
                Ok(value) => JsonValue::from_json(&value),
                Err(error) => JsonValue::String(format!("unserializable clause: {error}")),
            }
        })
        .collect()
}

impl<'de> Deserialize<'de> for PayloadPredicate {
    fn deserialize<D: lgwks_std::json::serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let raw = JsonValue::deserialize(deserializer)?;
        decode_predicate(&raw)
            .ok_or_else(|| lgwks_std::json::serde::de::Error::custom("unknown predicate operator"))
    }
}

/// Decode one predicate, trying operators in kernel order (eq, neq, in,
/// exists, and, or) and falling through on shape mismatch.
fn decode_predicate(raw: &JsonValue) -> Option<PayloadPredicate> {
    decode_predicate_depth(raw, 0)
}

/// Decode with a nesting cap matching the matcher's 64-node budget: a clause
/// at depth `d` sits under `d` ancestors, so depth 64 is the 65th node of a
/// chain and could never validate. Decoding refuses it up front; every chain
/// that fits the budget (depth at most 63) still decodes.
fn decode_predicate_depth(raw: &JsonValue, depth: usize) -> Option<PayloadPredicate> {
    if depth >= 64 {
        return None;
    }
    let map = raw.object_value()?;
    if let Some(clause) = decode_pair_clause(map, "eq") {
        return clause.map(|(keypath, value)| PayloadPredicate::eq(keypath, value));
    }
    if let Some(clause) = decode_pair_clause(map, "neq") {
        return clause.map(|(keypath, value)| PayloadPredicate::neq(keypath, value));
    }
    if let Some(members) = map.get("in") {
        if let Some(items) = members.array_value()
            && items.len() == 2
            && let Some(keypath) = items.first().and_then(JsonValue::string_value)
            && let Some(values) = items.get(1).and_then(JsonValue::array_value)
        {
            return Some(PayloadPredicate::is_in(keypath.clone(), values.clone()));
        }
        return None;
    }
    if let Some(keypath) = map.get("exists").and_then(|member| member.string_value()) {
        return Some(PayloadPredicate::exists(keypath.clone()));
    }
    if let Some(children) = decode_children(map, "and", depth) {
        return Some(PayloadPredicate::And(children));
    }
    if let Some(children) = decode_children(map, "or", depth) {
        return Some(PayloadPredicate::Or(children));
    }
    None
}

/// Decode `{"eq"|"neq": [keypath-string, value]}`.
fn decode_pair_clause(
    map: &BTreeMap<String, JsonValue>,
    operator: &str,
) -> Option<Option<(String, JsonValue)>> {
    let candidate = map.get(operator)?;
    let items = candidate.array_value()?;
    if items.len() == 2
        && let Some(key) = items.first()
        && let Some(value) = items.get(1)
        && let Some(keypath) = key.string_value()
    {
        return Some(Some((keypath.clone(), value.clone())));
    }
    Some(None)
}

/// Decode `{"and"|"or": [...]}` with recursively decoded children.
fn decode_children(
    map: &BTreeMap<String, JsonValue>,
    operator: &str,
    depth: usize,
) -> Option<Vec<PayloadPredicate>> {
    let candidate = map.get(operator)?;
    let items = candidate.array_value()?;
    let mut children = Vec::with_capacity(items.len());
    for item in items {
        children.push(decode_predicate_depth(item, depth.saturating_add(1))?);
    }
    Some(children)
}
