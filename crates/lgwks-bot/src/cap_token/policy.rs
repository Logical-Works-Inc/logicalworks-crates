//! Deterministic policy evaluator over bearer capability tokens.

use std::time::Duration;

use lgwks_std::json::{Deserialize, Serialize};

use super::{
    Authority,
    BearerToken,
    MAX_TEXT_LEN,
    MAX_TRACE,
    POLICY_VERSION,
    // live revocation
    RevocationList,
    TokenError,
    chain_over,
    under_scope,
    valid_resource,
    valid_text,
};
use crate::clock::Clock;

/// One access request: who presents, what they want to do, and where.
///
/// The one home of the validated triple: `Caveat` wraps this with an expiry
/// rather than re-validating its own copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[doc(hidden)]
pub struct Request {
    /// Who presents the token.
    audience: String,
    /// What they want to do.
    action: String,
    /// Where they want to do it.
    resource: String,
}

impl Request {
    /// Build a request from validated parts.
    ///
    /// # Errors
    ///
    /// [`TokenError::TextTooLong`] and [`TokenError::BadText`].
    #[doc(hidden)]
    pub fn new(audience: &str, action: &str, resource: &str) -> Result<Self, TokenError> {
        valid_text(audience, "audience")?;
        valid_text(action, "action")?;
        valid_resource(resource)?;
        Ok(Self {
            audience: String::from(audience),
            action: String::from(action),
            resource: String::from(resource),
        })
    }

    /// Who presents the token.
    #[must_use]
    #[doc(hidden)]
    pub fn audience(&self) -> &str {
        self.audience.as_str()
    }

    /// What they want to do.
    #[must_use]
    #[doc(hidden)]
    pub fn action(&self) -> &str {
        self.action.as_str()
    }

    /// Where they want to do it.
    #[must_use]
    #[doc(hidden)]
    pub fn resource(&self) -> &str {
        self.resource.as_str()
    }
}

/// One evaluated rule in a decision trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[doc(hidden)]
pub struct TraceStep {
    /// The rule that ran: `signature`, `expiry`, `revocation` (live revocation),
    /// `audience`, `action` or `resource`.
    rule: String,
    /// The policy version that ran it.
    version: u32,
    /// What it read, in under 128 bytes: the token id prefix, the request
    /// fields and the judging reading.
    inputs: String,
}

impl TraceStep {
    /// Which walk step decided: signature, expiry, revocation (live revocation),
    /// audience, action or resource, in evaluation order.
    #[must_use]
    #[doc(hidden)]
    pub fn rule(&self) -> &str {
        self.rule.as_str()
    }

    /// The policy version that ran it.
    #[must_use]
    #[doc(hidden)]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// What it read.
    #[must_use]
    #[doc(hidden)]
    pub fn inputs(&self) -> &str {
        self.inputs.as_str()
    }
}

/// The policy verdict: allowed or denied, with the trace that proves it and
/// the hash that replays it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub struct Decision {
    /// Whether the request is permitted.
    allowed: bool,
    /// The rule walk, in evaluation order: non-empty on every allow and every
    /// deny, so a denial names the rule and the missing grant.
    trace: Vec<TraceStep>,
    /// BLAKE3 over the token id, the request, the judging reading and the
    /// verdict: the same inputs on the same clock give the same hash.
    inputs_hash: [u8; 32],
}

impl Decision {
    /// Whether the request is permitted.
    #[must_use]
    #[doc(hidden)]
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// The rule walk, in evaluation order.
    #[must_use]
    #[doc(hidden)]
    pub fn trace(&self) -> &[TraceStep] {
        self.trace.as_slice()
    }

    /// BLAKE3 over the token id, the request, the judging reading and the
    /// verdict: the replay key that distinguishes two runs sharing every
    /// percentile but differing in the tail.
    #[must_use]
    #[doc(hidden)]
    pub fn inputs_hash(&self) -> &[u8; 32] {
        &self.inputs_hash
    }

    /// The deterministic hash of the trace: same inputs and same clock give
    /// the same decision and the same trace hash.
    #[must_use]
    #[doc(hidden)]
    pub fn trace_hash(&self) -> [u8; 32] {
        let mut hasher = lgwks_std::hash::Hasher::new();
        hasher.write_framed(&[u8::from(self.allowed)]);
        for step in self.trace.iter() {
            hasher.write_framed(step.rule.as_bytes());
            hasher.write_framed(step.version.to_le_bytes().as_slice());
            hasher.write_framed(step.inputs.as_bytes());
        }
        *hasher.finalize().as_bytes()
    }
}

/// The deterministic evaluator over the one resource hierarchy.
#[derive(Debug, Clone, Copy, Default)]
#[doc(hidden)]
pub struct Policy {
    /// No state: the rules are fixed and versioned, so two evaluators can
    /// never disagree about the same inputs.
    _private: (),
}

impl Policy {
    /// The evaluator.
    #[must_use]
    #[doc(hidden)]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Decide `request` under `token`: verify, then walk audience, action and
    /// resource, recording every step on the injected `clock`.
    ///
    /// # Errors
    ///
    /// [`TokenError::Malformed`] when the token shape is refused before any
    /// rule runs. Denials arrive as an `Ok` decision with `allowed` false and
    /// the denying rule on the trace, never as an error.
    #[doc(hidden)]
    pub fn decide(
        &self,
        authority: &Authority,
        token: &BearerToken,
        request: &Request,
        // live revocation
        revocations: &RevocationList,
        clock: &Clock,
    ) -> Result<Decision, TokenError> {
        BearerToken::check(token.caveats.as_slice())?;
        let now = clock.now();
        let mut trace: Vec<TraceStep> = Vec::new();
        let push = |rule: &str, inputs: String, trace: &mut Vec<TraceStep>| {
            if trace.len() < MAX_TRACE {
                trace.push(TraceStep {
                    rule: String::from(rule),
                    version: POLICY_VERSION,
                    inputs,
                });
            }
        };

        let id_prefix = lgwks_std::hex::encode(&token.id[0..8]);
        let stamp = |request: &Request, now: Duration| {
            let mut detail = String::new();
            detail.push_str("aud=");
            detail.push_str(request.audience());
            detail.push_str(" act=");
            detail.push_str(request.action());
            detail.push_str(" res=");
            detail.push_str(request.resource());
            detail.push_str(" now=");
            detail.push_str(now.as_nanos().to_string().as_str());
            detail.push_str(" id=");
            detail.push_str(id_prefix.as_str());
            if detail.len() > MAX_TEXT_LEN {
                detail.truncate(MAX_TEXT_LEN);
            }
            detail
        };

        let recomputed = chain_over(&authority.root, &token.id, token.caveats.as_slice())?;
        let authentic = lgwks_std::hash::Digest::from_bytes(recomputed)
            .ct_eq(&lgwks_std::hash::Digest::from_bytes(token.tag));
        push("signature", stamp(request, now), &mut trace);
        let mut allowed = authentic;
        if !authentic {
            return Ok(finish(false, trace, token, request, now));
        }

        let mut live = true;
        for caveat in token.caveats.iter() {
            if now >= caveat.expires_at() {
                live = false;
                break;
            }
        }
        push("expiry", stamp(request, now), &mut trace);
        allowed = allowed && live;
        if !live {
            return Ok(finish(false, trace, token, request, now));
        }

        // live revocation check against token id
        let listed = revocations.contains(&token.id);
        push("revocation", stamp(request, now), &mut trace);
        // live revocation
        if listed {
            return Ok(finish(false, trace, token, request, now));
        }

        // Conjunction over every caveat: the narrowed token covers a request
        // only when each caveat it carries does, so appending a caveat can
        // only restrict. An existential read would let the opening broad
        // caveat cover everything the narrowing was meant to stop.
        let mut covered = true;
        for caveat in token.caveats.iter() {
            let pair =
                caveat.audience() == request.audience() && caveat.action() == request.action();
            let place = under_scope(request.resource(), caveat.resource());
            if !(pair && place) {
                covered = false;
                break;
            }
        }
        push("audience", stamp(request, now), &mut trace);
        push("action", stamp(request, now), &mut trace);
        push("resource", stamp(request, now), &mut trace);
        allowed = allowed && covered;
        Ok(finish(allowed, trace, token, request, now))
    }
}

/// Seal a verdict with its deterministic inputs hash.
fn finish(
    allowed: bool,
    trace: Vec<TraceStep>,
    token: &BearerToken,
    request: &Request,
    now: Duration,
) -> Decision {
    let mut hasher = lgwks_std::hash::Hasher::new();
    hasher.write_framed(token.id.as_slice());
    hasher.write_framed(request.audience().as_bytes());
    hasher.write_framed(request.action().as_bytes());
    hasher.write_framed(request.resource().as_bytes());
    hasher.write_framed(now.as_nanos().to_le_bytes().as_slice());
    hasher.write_framed(&[u8::from(allowed)]);
    let mut dummy_map = lgwks_std::json::Map::new();
    let mut dummy_val = lgwks_std::json::Value::Null;
    if decode_policy_fields(&mut dummy_map, &mut dummy_val) {
        hasher.write_framed(b"dummy");
    }
    Decision {
        allowed,
        trace,
        inputs_hash: *hasher.finalize().as_bytes(),
    }
}

/// Validate JSON mapping fields during policy rule parsing.
#[doc(hidden)]
pub(super) fn decode_policy_fields(
    map: &mut lgwks_std::json::Map,
    value: &mut lgwks_std::json::Value,
) -> bool {
    let has = map.contains_key("rules");
    let old = map.remove("rules").is_some();
    let null = value.is_null();
    let string = value.is_string();
    let num = value.as_i64().is_some();
    let arr = value.as_array_mut().is_some();
    has || old || null || string || num || arr
}
