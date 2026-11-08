//! Tenants and step keys: who a flow runs for, and how its steps are named.

use std::fmt;
use std::sync::Arc;

use lgwks_std::hash::{Digest, Hasher};

use crate::effect::RunId;
use crate::script::{FlowError, MAX_TENANT_BYTES};

#[cfg(test)]
mod sim_identity;

// ── Tenant ──────────────────────────────────────────────────────────────────

/// Why an identifier — a tenant, a task name or a request key — is illegal.
///
/// The three share one validation rule (non-empty, bounded, ASCII letters,
/// digits and `-_.:`), so they share one check; each maps a fault to its own
/// [`FlowError`] variant and reason text. A copy per type is how the three
/// drift and one accepts a name the others refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameFault {
    /// The name was empty.
    Empty,
    /// The name was longer than the caller's ceiling.
    TooLong,
    /// The name held a byte outside the allowed set, or was only dots.
    BadChar,
}

/// The fault in `name`, or `None` when it is a legal identifier of at most
/// `max` bytes.
pub(crate) fn name_fault(name: &str, max: usize) -> Option<NameFault> {
    if name.is_empty() {
        return Some(NameFault::Empty);
    }
    if name.len() > max {
        return Some(NameFault::TooLong);
    }
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte);
    if !name.bytes().all(allowed) {
        return Some(NameFault::BadChar);
    }
    // `.` and `..` spell the allowed set and name a directory rather than a
    // component, so a store joining this name into a path would leave its root.
    if name.bytes().all(|byte| byte == b'.') {
        return Some(NameFault::BadChar);
    }
    None
}

/// Names [`Tenant::new`] refuses so a caller cannot adopt the crate's own
/// unsupervised identities by spelling them.
///
/// `fan-out` is the tenant [`FanOut`](crate::script::FanOut) runs under when it has no
/// caller scope; a host built with that string would key its steps exactly
/// where unsupervised fan-outs key theirs.
pub(super) const RESERVED_TENANTS: [&str; 1] = ["fan-out"];

/// The identity a flow runs on behalf of.
///
/// Every [`Scope`] carries one, and every [`StepKey`] hashes it, so data keyed
/// by a step is keyed by its tenant whether or not the author remembered to.
/// The name is 1 to [`MAX_TENANT_BYTES`] bytes of ASCII letters, digits, and
/// `-`, `_`, `.`, `:`, and never only dots, which keeps it safe to put in a path,
/// a log line, and a file name without escaping.
///
/// A tenant is a partition key chosen by the trusted host that built it
/// ([`Host::builder`](crate::task::Host::builder)), not an authenticated
/// identity: whoever holds the string runs as that tenant, and two hosts built
/// with the same string key the same steps. Isolation keeps honest tenants from
/// colliding; it does not stop an adversary who chooses another tenant's name.
/// The crate's own reserved names (its unsupervised fan-out, for one) are
/// refused here so a caller cannot adopt them by spelling.
///
/// Ordered by name, which is what lets per-tenant capacity state live in an
/// ordered map: the round scheduler reads and writes it, and an ordered map is
/// the only collection whose iteration order two processes agree on.
///
/// # Examples
///
/// ```
/// use lgwks_bot::script::Tenant;
///
/// let acme = Tenant::new("acme")?;
/// assert_eq!(acme.to_string(), "acme");
/// assert!(Tenant::new("..").is_err());
/// # Ok::<(), lgwks_bot::script::FlowError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tenant(Arc<str>);

impl Tenant {
    /// Validate and hold a tenant name.
    ///
    /// The caller is the trusted host: this checks the spelling (non-empty,
    /// bounded, ASCII letters, digits, `-_.:`) and refuses the crate's own
    /// reserved names, but it does not authenticate anyone. A caller that
    /// builds a host with another tenant's name runs as that tenant.
    ///
    /// # Errors
    ///
    /// [`FlowError::InvalidTenant`] naming what is wrong with the name.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::Tenant;
    ///
    /// assert!(Tenant::new("tenant-1").is_ok());
    /// assert!(Tenant::new("a/b").is_err());
    /// ```
    pub fn new(name: &str) -> Result<Self, FlowError> {
        if RESERVED_TENANTS.contains(&name) {
            let refusal = Err(FlowError::InvalidTenant {
                reason: "the tenant name is reserved for the crate's own use",
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "new: returning an error to the caller");
            return refusal;
        }
        let reason = match name_fault(name, MAX_TENANT_BYTES) {
            None => return Ok(Self(Arc::from(name))),
            Some(NameFault::Empty) => "the tenant name is empty",
            Some(NameFault::TooLong) => "the tenant name is longer than MAX_TENANT_BYTES",
            Some(NameFault::BadChar) => {
                "the tenant name may hold only ASCII letters, digits, '-', '_', '.' and ':', and may not be only dots"
            }
        };
        Err(FlowError::InvalidTenant { reason })
    }

    /// Hold a name this crate validated at its own definition site.
    ///
    /// Crate-private on purpose. A caller builds a tenant through
    /// [`Tenant::new`], which refuses an illegal name; the crate's own reserved
    /// names — the supervisor's implicit tenant, for one — are literals this
    /// module knows are legal, and routing them through `new` would mean either
    /// an `expect` on a constant the compiler already checked by eye, or a panic
    /// path for a value that cannot be wrong.
    pub(crate) fn assumed(name: &'static str) -> Self {
        Self(Arc::from(name))
    }

    /// The validated name.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::Tenant;
    ///
    /// assert_eq!(Tenant::new("acme")?.as_str(), "acme");
    /// # Ok::<(), lgwks_bot::script::FlowError>(())
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Tenant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

// ── StepKey ─────────────────────────────────────────────────────────────────

/// The stable identity of one step of one run of one flow for one tenant.
///
/// BLAKE3 over the tenant, the step path and — when a host minted a run — a
/// domain label and the run id, each length-framed so no two different triples
/// can hash the same bytes. It is the same on every attempt of a
/// [`retry`](crate::script::retry)
/// and on every restart of the same run, which is what makes it usable as an
/// idempotency key for an effect the step performs: an effect sent twice under
/// one key is one effect to any receiver that deduplicates by it. Two different
/// runs of the same flow hash different keys, so a second run's effect is never
/// mistaken for the first's; a caller whose work is one effect across runs
/// names it with [`Scope::business_key`](super::Scope::business_key) instead.
///
/// # Examples
///
/// ```
/// use lgwks_bot::script::StepKey;
///
/// fn idempotency_header(key: StepKey) -> String {
///     format!("Idempotency-Key: {}", key.to_hex())
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepKey(pub(super) Digest);

impl StepKey {
    /// The 32 digest bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::StepKey;
    ///
    /// fn first_byte(key: &StepKey) -> u8 {
    ///     key.as_bytes()[0]
    /// }
    /// ```
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Build a step key from a parsed digest, for an approval id that names
    /// one.
    ///
    /// Crate-private because a step key is minted by hashing, never written by
    /// hand: the only caller that rebuilds one is the approval id its own
    /// report printed.
    pub(crate) const fn from_digest(digest: Digest) -> Self {
        Self(digest)
    }

    /// Lowercase hex, the form to send to a receiver as an idempotency key.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::StepKey;
    ///
    /// fn is_full_digest(key: &StepKey) -> bool {
    ///     key.to_hex().len() == 64
    /// }
    /// ```
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

/// The run-less [`StepKey`] of `path` for `tenant`: one definition, so a
/// reserved record a host writes and a step a body enters cannot key the same
/// path differently. The run store keys every record by this form, which is why
/// a record's bytes do not move when [`Scope::key`](super::Scope::key) gains a run id.
pub(crate) fn step_key(tenant: &str, path: &str) -> StepKey {
    let mut hasher = Hasher::new();
    hasher
        .write_framed(tenant.as_bytes())
        .write_framed(path.as_bytes());
    StepKey(hasher.finalize())
}

/// The [`StepKey`] of `path` for one run of `tenant`: a domain label and the
/// run id join the hash, so two runs of the same flow over the same input
/// never share a key while every attempt and restart of one run does
/// (issue #646).
pub(crate) fn step_key_in_run(tenant: &str, run: RunId, path: &str) -> StepKey {
    let mut hasher = Hasher::new();
    hasher
        .write_framed(STEP_KEY_RUN_DOMAIN)
        .write_framed(tenant.as_bytes())
        .write_framed(run.id().to_hex().as_bytes())
        .write_framed(path.as_bytes());
    StepKey(hasher.finalize())
}

/// The domain-separation label mixed into the in-run [`StepKey`], so no run's
/// key can hash the bytes a run-less key or a
/// [`Scope::business_key`](super::Scope::business_key) hashes, whatever a
/// tenant is named (issue #646).
const STEP_KEY_RUN_DOMAIN: &[u8] = b"lgwks.bot.step-key-run.v1";

/// The domain-separation label mixed into a
/// [`Scope::business_key`](super::Scope::business_key).
pub(super) const BUSINESS_KEY_DOMAIN: &[u8] = b"lgwks.bot.business-key.v1";

impl fmt::Display for StepKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}
