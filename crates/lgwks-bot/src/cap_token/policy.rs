//! Deterministic policy evaluator over bearer capability tokens.

use std::time::Duration;

use lgwks_std::json::{Deserialize, Serialize};

use super::{
    Authority, BearerToken, MAX_TEXT_LEN, MAX_TRACE, POLICY_VERSION, RevocationList, TokenError,
    chain_over, under_scope, valid_resource, valid_text,
};
use crate::clock::Clock;

/// One access request: who presents, what they want to do, and where.
///
/// The one home of the validated triple: `Caveat` wraps this with an expiry
/// rather than re-validating its own copy. Decoding validates too, so a
/// request read from the wire is never a request `new` would have refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", try_from = "RequestWire")]
#[doc(hidden)]
pub struct Request {
    /// Who presents the token.
    audience: String,
    /// What they want to do.
    action: String,
    /// Where they want to do it.
    resource: String,
}

/// The decoded shape of a request before `Request::new` judges it.
#[derive(Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct RequestWire {
    /// Who presents the token, as decoded.
    audience: String,
    /// What they want to do, as decoded.
    action: String,
    /// Where they want to do it, as decoded.
    resource: String,
}

impl TryFrom<RequestWire> for Request {
    type Error = TokenError;

    fn try_from(wire: RequestWire) -> Result<Self, TokenError> {
        Self::new(&wire.audience, &wire.action, &wire.resource)
    }
}

impl Request {
    /// Build a request from validated parts.
    ///
    /// # Errors
    ///
    /// [`TokenError::TextTooLong`] and [`TokenError::BadText`], the second
    /// also for a resource segment that is only dots or holds `\` or `%`.
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
    /// The rule that ran: `signature`, `expiry`, `revocation`, `audience`,
    /// `action` or `resource`.
    rule: String,
    /// The policy version that ran it.
    version: u32,
    /// What it read, in at most 128 bytes: on the denying step the grant the
    /// token holds first (`grant=read `), then the request fields, the
    /// judging reading and the token id prefix.
    inputs: String,
}

impl TraceStep {
    /// Which walk step decided: signature, expiry, revocation, audience,
    /// action or resource, in evaluation order.
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
    /// The rule walk, in evaluation order: an allow holds every rule, and a
    /// denial ends at the rule that denied, naming the grant the token holds.
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

/// The rules, in the one order every decision walks them.
#[derive(Debug, Clone, Copy)]
enum Rule {
    /// The chain recomputes under the judging authority's root.
    Signature,
    /// No caveat has lapsed on the judging clock.
    Expiry,
    /// The token id is not revoked.
    Revocation,
    /// Every caveat names the presenting audience.
    Audience,
    /// Every caveat names the requested action.
    Action,
    /// The resource sits under every caveat's scope.
    Resource,
}

impl Rule {
    /// The walk order.
    const ALL: [Self; 6] = [
        Self::Signature,
        Self::Expiry,
        Self::Revocation,
        Self::Audience,
        Self::Action,
        Self::Resource,
    ];

    /// The name a trace step carries.
    fn name(self) -> &'static str {
        match self {
            Self::Signature => "signature",
            Self::Expiry => "expiry",
            Self::Revocation => "revocation",
            Self::Audience => "audience",
            Self::Action => "action",
            Self::Resource => "resource",
        }
    }
}

/// The walk is one step per rule, so the trace bound holds by construction:
/// a rule added past `MAX_TRACE` is a compile error, not a dropped step.
const _: () = assert!(
    MAX_TRACE.checked_sub(Rule::ALL.len()).is_some(),
    "every rule of the walk fits the trace bound"
);

/// What one decision reads.
struct Judged<'call> {
    /// The authority whose root the chain must recompute under.
    authority: &'call Authority,
    /// The presented token.
    token: &'call BearerToken,
    /// The request it is presented for.
    request: &'call Request,
    /// The live revocation list.
    revocations: &'call RevocationList,
    /// The judging clock's reading.
    now: Duration,
}

impl Judged<'_> {
    /// `None` when `rule` passes, or the grant the token holds that the
    /// request misses. Every caveat must pass: a holder only ever appends, so
    /// a conjunction is what keeps an appended caveat narrowing.
    ///
    /// # Errors
    ///
    /// [`TokenError::Malformed`] when a caveat cannot be encoded for the chain.
    fn miss(&self, rule: Rule) -> Result<Option<String>, TokenError> {
        let caveats = self.token.caveats.as_slice();
        let missed = match rule {
            Rule::Signature => {
                let recomputed = chain_over(&self.authority.root, &self.token.id, caveats)?;
                let authentic = lgwks_std::hash::Digest::from_bytes(recomputed)
                    .ct_eq(&lgwks_std::hash::Digest::from_bytes(self.token.tag));
                (!authentic).then(|| String::from("issuer"))
            }
            Rule::Expiry => caveats
                .iter()
                .find(|caveat| self.now >= caveat.expires_at())
                .map(|caveat| caveat.expires_at().as_nanos().to_string()),
            Rule::Revocation => self
                .revocations
                .contains(&self.token.id)
                .then(|| String::from("unrevoked")),
            Rule::Audience => caveats
                .iter()
                .find(|caveat| caveat.audience() != self.request.audience())
                .map(|caveat| String::from(caveat.audience())),
            Rule::Action => caveats
                .iter()
                .find(|caveat| caveat.action() != self.request.action())
                .map(|caveat| String::from(caveat.action())),
            Rule::Resource => caveats
                .iter()
                .find(|caveat| !under_scope(self.request.resource(), caveat.resource()))
                .map(|caveat| String::from(caveat.resource())),
        };
        Ok(missed)
    }

    /// The inputs line of one step: the missed grant first when there is one,
    /// then the request, the reading and the token id prefix, cut to the bound.
    fn stamp(&self, missed: Option<&str>) -> String {
        let id_prefix = lgwks_std::hex::encode(&self.token.id[0..8]);
        let grant = match missed {
            Some(grant) => format!("grant={grant} "),
            None => String::new(),
        };
        let mut detail = format!(
            "{grant}aud={} act={} res={} now={} id={id_prefix}",
            self.request.audience(),
            self.request.action(),
            self.request.resource(),
            self.now.as_nanos(),
        );
        // Every part is visible ASCII, so any byte offset is a char boundary.
        detail.truncate(MAX_TEXT_LEN);
        detail
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
    /// resource, recording every step on the injected `clock`. The walk stops
    /// at the first rule that denies, so a denial's last step names its rule.
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
        revocations: &RevocationList,
        clock: &Clock,
    ) -> Result<Decision, TokenError> {
        BearerToken::check(token.caveats.as_slice())?;
        let judged = Judged {
            authority,
            token,
            request,
            revocations,
            now: clock.now(),
        };
        let mut trace: Vec<TraceStep> = Vec::with_capacity(MAX_TRACE);
        for rule in Rule::ALL {
            let missed = judged.miss(rule)?;
            trace.push(TraceStep {
                rule: String::from(rule.name()),
                version: POLICY_VERSION,
                inputs: judged.stamp(missed.as_deref()),
            });
            if missed.is_some() {
                lgwks_std::trace::debug!(rule = rule.name(), "cap_token_denied");
                return Ok(finish(false, trace, &judged));
            }
        }
        Ok(finish(true, trace, &judged))
    }
}

/// Seal a verdict with its deterministic inputs hash.
fn finish(allowed: bool, trace: Vec<TraceStep>, judged: &Judged<'_>) -> Decision {
    let mut hasher = lgwks_std::hash::Hasher::new();
    hasher.write_framed(judged.token.id.as_slice());
    hasher.write_framed(judged.request.audience().as_bytes());
    hasher.write_framed(judged.request.action().as_bytes());
    hasher.write_framed(judged.request.resource().as_bytes());
    hasher.write_framed(judged.now.as_nanos().to_le_bytes().as_slice());
    hasher.write_framed(&[u8::from(allowed)]);
    Decision {
        allowed,
        trace,
        inputs_hash: *hasher.finalize().as_bytes(),
    }
}
