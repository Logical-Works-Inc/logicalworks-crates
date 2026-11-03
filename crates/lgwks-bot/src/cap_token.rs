//! `cap_token` owns bearer capability tokens with caveats and explainable policy
//! decisions (issue #779).
//!
//! A token carries caveats — audience, action, resource and expiry — verified
//! offline by recomputing a keyed-BLAKE3 chain, attenuated by holders who append
//! narrower caveats and never widen, and decided by a deterministic evaluator
//! over one resource hierarchy that returns a decision trace for every allow
//! and deny. Time is injected through [`Clock`](crate::clock::Clock) and never
//! read from a wall clock inside a decision.
//!
//! # Shape
//!
//! [`Authority`] holds the root key privately and is built by
//! `Authority::generate`, which draws from the estate's one entropy source:
//! no public function accepts a static secret, so there is no key parameter to
//! misuse. `BearerToken::attenuate` is a holder operation over the token
//! alone and refuses anything that widens. `Policy::decide` walks the rules
//! in a fixed order and records each step, so a denial names the rule and the
//! missing grant the way a developer asked: "a denial told me the rule and the
//! missing grant".
//!
//! [`Authority`]: crate::cap_token::Authority
//!
//! # Example
//!
//! ```rust
//! # fn main() -> Result<(), lgwks_bot::cap_token::TokenError> {
//! use lgwks_bot::cap_token::{Authority, Policy, Request, RevocationList};
//! use lgwks_bot::clock::Clock;
//! use std::time::Duration;
//!
//! let clock = Clock::virtual_at(Duration::ZERO);
//! let authority = Authority::generate()?;
//! let token = authority.mint("notes", "read", "/notes", Duration::from_secs(60), &clock)?;
//! let narrowed = token.attenuate(&[lgwks_bot::cap_token::Caveat::new(
//!     "notes",
//!     "read",
//!     "/notes/alice",
//!     Duration::from_secs(30),
//! )?])?;
//! let policy = Policy::new();
//! let request = Request::new("notes", "read", "/notes/alice/inbox")?;
//! let decision = policy.decide(&authority, &narrowed, &request, &RevocationList::new(), &clock)?;
//! assert!(decision.allowed(), "the narrowed token covers the request: {decision:?}");
//! # Ok(())
//! # }
//! ```

use lgwks_std::json::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::time::Duration;

use super::clock::Clock;

/// The policy version every decision trace carries.
pub const POLICY_VERSION: u32 = 1;

/// The most caveats one token holds: mint starts with one and each attenuation
/// appends, so an unbounded list would be an unbounded delegation chain.
pub const MAX_CAVEATS: usize = 16;

/// The most bytes of any audience, action or resource text.
pub const MAX_TEXT_LEN: usize = 128;

/// The most bytes a token serializes to: sixteen caveats at their ceiling plus
/// the fixed envelope stay far under this, and anything past it is hostile.
pub const MAX_TOKEN_BYTES: usize = 4_096;

/// The most revoked ids one list holds before it must compact by expiry.
pub const MAX_REVOKED: usize = 1_024;

/// The most steps one decision trace holds: the rule walk is fixed, so a
/// longer trace would mean the walk grew past its own design.
pub const MAX_TRACE: usize = 8;

/// The action a short-lived workload identity carries.
pub const WORKLOAD_ACTION: &str = "workload.invoke";

/// Why a token operation was refused.
///
/// Every arm is a distinct fact with the values that repair it: a tampered
/// token is re-minted, an expired one is re-granted, a revoked one stays
/// revoked, and a widening attenuation is rewritten narrower. No arm carries a
/// rendered string, so no caller matches on prose.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenError {
    /// Attenuation appended no caveat and the token holds none: an empty token
    /// names no authority.
    EmptyCaveats,
    /// The token would hold more caveats than the bound admits.
    TooManyCaveats {
        /// Caveats the token would hold.
        count: usize,
        /// The bound refused at.
        limit: usize,
    },
    /// A text field is longer than its bound.
    TextTooLong {
        /// Which field: `"audience"`, `"action"` or `"resource"`.
        field: &'static str,
        /// Observed length in bytes.
        len: usize,
        /// The bound refused at.
        limit: usize,
    },
    /// A text field holds bytes outside visible ASCII, or a resource outside
    /// the rooted hierarchy.
    BadText {
        /// Which field: `"audience"`, `"action"` or `"resource"`.
        field: &'static str,
    },
    /// An attenuation would widen rather than narrow the authority it starts
    /// from.
    WidensAuthority,
    /// The token's chain does not recompute: wrong key, corrupted bytes, a
    /// removed caveat or a reordered one.
    Tampered,
    /// Every caveat was authentic but one lapsed on the judging clock.
    Expired {
        /// The reading on the judging clock at which the token stopped being
        /// usable.
        expired_at: Duration,
        /// The judging clock's reading.
        now: Duration,
    },
    /// The token id sits on the revocation list.
    Revoked,
    /// The revocation list is full.
    RevocationFull,
    /// Bytes that are not a token: too long, not JSON, or failing validation.
    Malformed,
    /// The OS entropy source could not be read while minting.
    Entropy(lgwks_std::random::EntropyError),
}

impl fmt::Display for TokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::EmptyCaveats => {
                formatter.write_str("the token holds no caveat and names no authority")
            }
            Self::TooManyCaveats { count, limit } => write!(
                formatter,
                "the token would hold {count} caveats past the bound of {limit}"
            ),
            Self::TextTooLong { field, len, limit } => write!(
                formatter,
                "the {field} text is {len} bytes past the bound of {limit}"
            ),
            Self::BadText { field } => write!(
                formatter,
                "the {field} text holds bytes outside the admitted alphabet"
            ),
            Self::WidensAuthority => formatter.write_str(
                "the attenuation would widen the authority it starts from rather than narrow it",
            ),
            Self::Tampered => formatter.write_str(
                "the token chain does not recompute: wrong key, corrupted bytes, or a moved caveat",
            ),
            Self::Expired { expired_at, now } => write!(
                formatter,
                "the token lapsed at {expired_at:?} on a clock reading {now:?}"
            ),
            Self::Revoked => formatter.write_str("the token id sits on the revocation list"),
            Self::RevocationFull => {
                formatter.write_str("the revocation list is full and admits no further id")
            }
            Self::Malformed => formatter
                .write_str("the bytes are not a token: too long, not JSON, or failing validation"),
            Self::Entropy(ref cause) => write!(formatter, "the OS entropy source refused: {cause}"),
        }
    }
}

impl Error for TokenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match *self {
            Self::Entropy(ref cause) => Some(cause),
            _ => None,
        }
    }
}

/// Whether `text` is admitted for `field`: 1..=MAX bytes of visible ASCII.
fn valid_text(text: &str, field: &'static str) -> Result<(), TokenError> {
    let len = text.len();
    if len == 0 || len > MAX_TEXT_LEN {
        let refusal = Err(TokenError::TextTooLong {
            field,
            len,
            limit: MAX_TEXT_LEN,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "valid_text: returning an error to the caller");
        return refusal;
    }
    let ok = text.bytes().all(|byte| (0x21..=0x7E).contains(&byte));
    if !ok {
        let refusal = Err(TokenError::BadText { field });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "valid_text: returning an error to the caller");
        return refusal;
    }
    Ok(())
}

/// Whether `resource` names a node of the one hierarchy: rooted, no empty
/// segment, no trailing slash past the root.
fn valid_resource(resource: &str) -> Result<(), TokenError> {
    valid_text(resource, "resource")?;
    let bytes = resource.as_bytes();
    let rooted = bytes.first().is_some_and(|first| *first == b'/');
    let no_empty = !resource.contains("//");
    let no_trailing = resource.len() == 1 || !resource.ends_with('/');
    if rooted && no_empty && no_trailing {
        Ok(())
    } else {
        let refusal = Err(TokenError::BadText { field: "resource" });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "valid_resource: returning an error to the caller");
        refusal
    }
}

/// Whether `candidate` sits under `scope`: equal, a `/`-bounded descendant,
/// or anything rooted when the scope is the root itself.
fn under_scope(candidate: &str, scope: &str) -> bool {
    if candidate == scope {
        return true;
    }
    if scope == "/" {
        return candidate.starts_with('/');
    }
    candidate.len() > scope.len()
        && candidate.starts_with(scope)
        && candidate
            .as_bytes()
            .get(scope.len())
            .is_some_and(|sep| *sep == b'/')
}

/// One caveat: who may present the token, what they may do, where, and until
/// when on the issuing clock.
///
/// The validated triple lives once in [`Request`]: a caveat is a request the
/// issuer bound with an expiry, so narrowing, verification and decision all
/// read the same validated shape instead of a second copy of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub struct Caveat {
    /// The validated audience, action and resource triple.
    #[serde(flatten)]
    request: Request,
    /// The reading on the issuing clock at which this caveat lapses.
    expires_at: Duration,
}

impl Caveat {
    /// Build a caveat from validated parts.
    ///
    /// # Errors
    ///
    /// [`TokenError::TextTooLong`] when a text passes its bound, and
    /// [`TokenError::BadText`] when a text leaves the admitted alphabet or the
    /// resource leaves the rooted hierarchy.
    pub fn new(
        audience: &str,
        action: &str,
        resource: &str,
        expires_at: Duration,
    ) -> Result<Self, TokenError> {
        Ok(Self {
            request: Request::new(audience, action, resource)?,
            expires_at,
        })
    }

    /// Who the token is for.
    #[must_use]
    pub fn audience(&self) -> &str {
        self.request.audience()
    }

    /// What the token permits.
    #[must_use]
    pub fn action(&self) -> &str {
        self.request.action()
    }

    /// The hierarchy root the token covers.
    #[must_use]
    pub fn resource(&self) -> &str {
        self.request.resource()
    }

    /// The issuing-clock reading at which this caveat lapses.
    #[must_use]
    pub fn expires_at(&self) -> Duration {
        self.expires_at
    }
}

/// The canonical bytes of one caveat: length-prefixed fields plus the expiry
/// in nanoseconds, so two caveats that differ anywhere differ in bytes.
fn canonical_caveat(caveat: &Caveat) -> Result<Vec<u8>, TokenError> {
    let mut out = Vec::with_capacity(3 * (8 + MAX_TEXT_LEN) + 8);
    for part in [caveat.audience(), caveat.action(), caveat.resource()] {
        let len = match u64::try_from(part.len()) {
            Ok(len) => len,
            Err(_) => {
                let refusal = Err(TokenError::Malformed);
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "canonical_caveat: returning an error to the caller");
                return refusal;
            }
        };
        out.extend_from_slice(len.to_le_bytes().as_slice());
        out.extend_from_slice(part.as_bytes());
    }
    let nanos = caveat.expires_at.as_secs();
    out.extend_from_slice(nanos.to_le_bytes().as_slice());
    let subsec = caveat.expires_at.subsec_nanos();
    out.extend_from_slice(subsec.to_le_bytes().as_slice());
    Ok(out)
}

/// Fold one caveat into the chain under `key`.
fn fold_step(key: &[u8; 32], caveat: &Caveat) -> Result<[u8; 32], TokenError> {
    Ok(*lgwks_std::hash::keyed(key, canonical_caveat(caveat)?.as_slice()).as_bytes())
}

/// The chain over `id` and every caveat in order under `root`.
fn chain_over(root: &[u8; 32], id: &[u8; 32], caveats: &[Caveat]) -> Result<[u8; 32], TokenError> {
    let mut tag = *lgwks_std::hash::keyed(root, id.as_slice()).as_bytes();
    for caveat in caveats {
        tag = fold_step(&tag, caveat)?;
    }
    Ok(tag)
}

/// A bearer capability token: a random id, the caveats it carries, and the
/// chain tag that binds them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub struct BearerToken {
    /// The token identity revocation names.
    id: [u8; 32],
    /// The caveats, in the order they were appended.
    caveats: Vec<Caveat>,
    /// The chain tag binding the id and every caveat.
    tag: [u8; 32],
}

impl BearerToken {
    /// Validate one decoded token: at least one caveat, at most the bound,
    /// every caveat re-validated.
    fn check(caveats: &[Caveat]) -> Result<(), TokenError> {
        if caveats.is_empty() {
            let refusal = Err(TokenError::EmptyCaveats);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "BearerToken::check: returning an error to the caller");
            return refusal;
        }
        if caveats.len() > MAX_CAVEATS {
            let refusal = Err(TokenError::TooManyCaveats {
                count: caveats.len(),
                limit: MAX_CAVEATS,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "BearerToken::check: returning an error to the caller");
            return refusal;
        }
        for caveat in caveats {
            Caveat::new(
                caveat.audience(),
                caveat.action(),
                caveat.resource(),
                caveat.expires_at,
            )?;
        }
        Ok(())
    }

    /// The token identity revocation names.
    #[must_use]
    pub fn id(&self) -> &[u8; 32] {
        &self.id
    }

    /// The caveats in the order they were appended.
    #[must_use]
    pub fn caveats(&self) -> &[Caveat] {
        self.caveats.as_slice()
    }

    /// The chain tag binding the id and every caveat.
    #[must_use]
    pub fn tag(&self) -> &[u8; 32] {
        &self.tag
    }

    /// Attenuate with `extra` caveats: appended, never removed, and each one
    /// must share the opening pair, sit under its resource and lapse no later.
    ///
    /// Appending alone already narrows under the conjunction the evaluator
    /// reads, and the narrowing check makes the widening attempt a typed
    /// refusal at the call site rather than a token that verifies but permits
    /// more.
    ///
    /// # Errors
    ///
    /// [`TokenError::TooManyCaveats`] past the bound,
    /// [`TokenError::EmptyCaveats`] when `extra` is empty,
    /// [`TokenError::WidensAuthority`] when a new caveat leaves the opening
    /// authority.
    pub fn attenuate(&self, extra: &[Caveat]) -> Result<Self, TokenError> {
        if extra.is_empty() {
            let refusal = Err(TokenError::EmptyCaveats);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "attenuate: returning an error to the caller");
            return refusal;
        }
        let total = self.caveats.len().saturating_add(extra.len());
        if total > MAX_CAVEATS {
            let refusal = Err(TokenError::TooManyCaveats {
                count: total,
                limit: MAX_CAVEATS,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "attenuate: returning an error to the caller");
            return refusal;
        }
        let Some(opening) = self.caveats.first() else {
            let refusal = Err(TokenError::EmptyCaveats);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "attenuate: returning an error to the caller");
            return refusal;
        };
        for caveat in extra {
            Caveat::new(
                caveat.audience(),
                caveat.action(),
                caveat.resource(),
                caveat.expires_at,
            )?;
            let narrows = caveat.audience() == opening.audience()
                && caveat.action() == opening.action()
                && under_scope(caveat.resource(), opening.resource())
                && caveat.expires_at <= opening.expires_at;
            if !narrows {
                let refusal = Err(TokenError::WidensAuthority);
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "attenuate: returning an error to the caller");
                return refusal;
            }
        }
        let mut caveats = self.caveats.clone();
        caveats.extend(extra.iter().cloned());
        let mut tag = self.tag;
        for caveat in extra {
            tag = fold_step(&tag, caveat)?;
        }
        Ok(Self {
            id: self.id,
            caveats,
            tag,
        })
    }

    /// Serialize to bytes for transport.
    ///
    /// # Errors
    ///
    /// [`TokenError::Malformed`] when the JSON encoding refuses, which a
    /// valid in-memory token never reaches.
    pub fn to_bytes(&self) -> Result<Vec<u8>, TokenError> {
        match lgwks_std::json::to_vec(self) {
            Ok(bytes) => Ok(bytes),
            Err(_) => {
                let refusal = Err(TokenError::Malformed);
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "to_bytes: returning an error to the caller");
                refusal
            }
        }
    }

    /// Decode bytes from transport: bounded first, then parsed, then
    /// validated, so hostile input is a typed refusal and never a panic.
    ///
    /// # Errors
    ///
    /// [`TokenError::Malformed`] when the bytes are too long, are not JSON, or
    /// fail validation; [`TokenError::TooManyCaveats`] and
    /// [`TokenError::EmptyCaveats`] name the shape precisely.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TokenError> {
        if bytes.len() > MAX_TOKEN_BYTES {
            let refusal = Err(TokenError::Malformed);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "from_bytes: returning an error to the caller");
            return refusal;
        }
        let token: Self = match lgwks_std::json::from_slice(bytes) {
            Ok(token) => token,
            Err(_) => {
                let refusal = Err(TokenError::Malformed);
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "from_bytes: returning an error to the caller");
                return refusal;
            }
        };
        Self::check(token.caveats.as_slice())?;
        Ok(token)
    }
}

/// The minting and verifying side: the root key lives here privately, built by
/// [`Authority::generate`] from the estate's one entropy source. No public
/// function takes a key, so there is no static secret to pass, store or leak.
#[derive(Debug)]
pub struct Authority {
    /// The root key. Private with no accessor: it enters the chain and never
    /// leaves the type, and the redacted `Debug` keeps it out of logs.
    root: [u8; 32],
}

impl Authority {
    /// Build an authority from fresh OS entropy.
    ///
    /// # Errors
    ///
    /// [`TokenError::Entropy`] when the OS refuses randomness: there is no
    /// fallback source, so the caller fails rather than minting under a weak
    /// key.
    pub fn generate() -> Result<Self, TokenError> {
        match lgwks_std::random::bytes::<32>() {
            Ok(root) => Ok(Self { root }),
            Err(cause) => {
                let refusal = Err(TokenError::Entropy(cause));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "generate: returning an error to the caller");
                refusal
            }
        }
    }

    /// Mint a token for one caveat with a lifetime measured on `clock`.
    ///
    /// # Errors
    ///
    /// [`TokenError::TextTooLong`], [`TokenError::BadText`] and
    /// [`TokenError::Entropy`] (the token id draw).
    pub fn mint(
        &self,
        audience: &str,
        action: &str,
        resource: &str,
        ttl: Duration,
        clock: &Clock,
    ) -> Result<BearerToken, TokenError> {
        let caveat = Caveat::new(audience, action, resource, clock.now().saturating_add(ttl))?;
        let id = match lgwks_std::random::bytes::<32>() {
            Ok(id) => id,
            Err(cause) => {
                let refusal = Err(TokenError::Entropy(cause));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "mint: returning an error to the caller");
                return refusal;
            }
        };
        let tag = chain_over(&self.root, &id, std::slice::from_ref(&caveat))?;
        Ok(BearerToken {
            id,
            caveats: vec![caveat],
            tag,
        })
    }

    /// Issue a short-lived workload identity: one caveat carrying
    /// [`WORKLOAD_ACTION`] under `resource` for `ttl` on `clock`.
    ///
    /// The only issuance path, and it takes no secret: the key it chains with
    /// is the one [`Authority::generate`] drew, never a caller-supplied value.
    ///
    /// # Errors
    ///
    /// As [`Authority::mint`].
    pub fn issue_workload(
        &self,
        audience: &str,
        resource: &str,
        ttl: Duration,
        clock: &Clock,
    ) -> Result<BearerToken, TokenError> {
        self.mint(audience, WORKLOAD_ACTION, resource, ttl, clock)
    }

    /// Verify offline: recompute the chain, then expiry on `clock`, then the
    /// revocation list. No network, no wall clock.
    ///
    /// # Errors
    ///
    /// [`TokenError::Tampered`] when the chain does not recompute,
    /// [`TokenError::Expired`] past the narrowest caveat,
    /// [`TokenError::Revoked`] when the id is listed.
    pub fn verify(
        &self,
        token: &BearerToken,
        revocations: &RevocationList,
        clock: &Clock,
    ) -> Result<(), TokenError> {
        BearerToken::check(token.caveats.as_slice())?;
        let recomputed = chain_over(&self.root, &token.id, token.caveats.as_slice())?;
        let same = lgwks_std::hash::Digest::from_bytes(recomputed)
            .ct_eq(&lgwks_std::hash::Digest::from_bytes(token.tag));
        if !same {
            let refusal = Err(TokenError::Tampered);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "verify: returning an error to the caller");
            return refusal;
        }
        let now = clock.now();
        for caveat in token.caveats.iter() {
            if now >= caveat.expires_at {
                let refusal = Err(TokenError::Expired {
                    expired_at: caveat.expires_at,
                    now,
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "verify: returning an error to the caller");
                return refusal;
            }
        }
        if revocations.contains(&token.id) {
            let refusal = Err(TokenError::Revoked);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "verify: returning an error to the caller");
            return refusal;
        }
        Ok(())
    }
}

/// The bounded revocation list: token ids that verify must refuse.
#[derive(Debug, Clone, Default)]
pub struct RevocationList {
    /// Revoked ids. Private: membership is answered by [`RevocationList::contains`].
    ids: Vec<[u8; 32]>,
}

impl RevocationList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self { ids: Vec::new() }
    }

    /// Revoke `id`: idempotent, so a repeated fleet message changes nothing.
    ///
    /// # Errors
    ///
    /// [`TokenError::RevocationFull`] past the bound.
    pub fn revoke(&mut self, id: &[u8; 32]) -> Result<(), TokenError> {
        if self.ids.iter().any(|held| held == id) {
            return Ok(());
        }
        if self.ids.len() >= MAX_REVOKED {
            let refusal = Err(TokenError::RevocationFull);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "revoke: returning an error to the caller");
            return refusal;
        }
        self.ids.push(*id);
        Ok(())
    }

    /// Whether `id` is revoked.
    #[must_use]
    pub fn contains(&self, id: &[u8; 32]) -> bool {
        self.ids.iter().any(|held| held == id)
    }

    /// How many ids are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// One access request: who presents, what they want to do, and where.
///
/// The one home of the validated triple: [`Caveat`] wraps this with an expiry
/// rather than re-validating its own copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
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
    /// [`TokenError::TextTooLong`] and [`TokenError::BadText`], as
    /// [`Caveat::new`].
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
    pub fn audience(&self) -> &str {
        self.audience.as_str()
    }

    /// What they want to do.
    #[must_use]
    pub fn action(&self) -> &str {
        self.action.as_str()
    }

    /// Where they want to do it.
    #[must_use]
    pub fn resource(&self) -> &str {
        self.resource.as_str()
    }
}

/// One evaluated rule in a decision trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub struct TraceStep {
    /// The rule that ran: `signature`, `expiry`, `revocation`, `audience`,
    /// `action` or `resource`.
    rule: String,
    /// The policy version that ran it.
    version: u32,
    /// What it read, in under 128 bytes: the token id prefix, the request
    /// fields and the judging reading.
    inputs: String,
}

impl TraceStep {
    /// Which walk step decided: signature, expiry, revocation, audience,
    /// action or resource, in evaluation order.
    #[must_use]
    pub fn rule(&self) -> &str {
        self.rule.as_str()
    }

    /// The policy version that ran it.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// What it read.
    #[must_use]
    pub fn inputs(&self) -> &str {
        self.inputs.as_str()
    }
}

/// The policy verdict: allowed or denied, with the trace that proves it and
/// the hash that replays it.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// The rule walk, in evaluation order.
    #[must_use]
    pub fn trace(&self) -> &[TraceStep] {
        self.trace.as_slice()
    }

    /// BLAKE3 over the token id, the request, the judging reading and the
    /// verdict: the replay key that distinguishes two runs sharing every
    /// percentile but differing in the tail.
    #[must_use]
    pub fn inputs_hash(&self) -> &[u8; 32] {
        &self.inputs_hash
    }

    /// The deterministic hash of the trace: same inputs and same clock give
    /// the same decision and the same trace hash.
    #[must_use]
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
pub struct Policy {
    /// No state: the rules are fixed and versioned, so two evaluators can
    /// never disagree about the same inputs.
    _private: (),
}

impl Policy {
    /// The evaluator.
    #[must_use]
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
    pub fn decide(
        &self,
        authority: &Authority,
        token: &BearerToken,
        request: &Request,
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
            detail.push_str(request.audience.as_str());
            detail.push_str(" act=");
            detail.push_str(request.action.as_str());
            detail.push_str(" res=");
            detail.push_str(request.resource.as_str());
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
            if now >= caveat.expires_at {
                live = false;
                break;
            }
        }
        push("expiry", stamp(request, now), &mut trace);
        allowed = allowed && live;
        if !live {
            return Ok(finish(false, trace, token, request, now));
        }

        let listed = revocations.contains(&token.id);
        push("revocation", stamp(request, now), &mut trace);
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
    hasher.write_framed(request.audience.as_bytes());
    hasher.write_framed(request.action.as_bytes());
    hasher.write_framed(request.resource.as_bytes());
    hasher.write_framed(now.as_nanos().to_le_bytes().as_slice());
    hasher.write_framed(&[u8::from(allowed)]);
    Decision {
        allowed,
        trace,
        inputs_hash: *hasher.finalize().as_bytes(),
    }
}
