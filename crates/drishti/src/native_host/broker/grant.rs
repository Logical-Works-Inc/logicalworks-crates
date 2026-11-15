//! Injected broker callbacks and the signed ML grant they verify.

use std::sync::Arc;

use lgwks_std::id::Uuid;

use super::super::protocol::SignalEnvelope;
use super::{AuditEvent, Permission, TapEvent};
use crate::canvas_protocol::{Capability, SessionInvariant};

/// Widget signal listener.
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::native_host::broker::SignalListener;
/// let listener: SignalListener = Arc::new(|envelope| drop(envelope));
/// assert_eq!(Arc::strong_count(&listener), 1);
/// ```
pub type SignalListener = Arc<dyn Fn(SignalEnvelope) + Send + Sync>;

/// Tap event listener.
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::native_host::broker::TapListener;
/// let listener: TapListener = Arc::new(|event| drop(event));
/// assert_eq!(Arc::strong_count(&listener), 1);
/// ```
pub type TapListener = Arc<dyn Fn(TapEvent) + Send + Sync>;

/// Injected millisecond clock (frozen in tests, wall time in production).
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::native_host::broker::ClockFn;
/// let frozen: ClockFn = Arc::new(|| 1_000);
/// assert_eq!((frozen(), frozen()), (1_000, 1_000));
/// ```
pub type ClockFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Injected envelope identity (counter in tests, random v4 in production).
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::native_host::broker::IdFn;
/// let fixed: IdFn = Arc::new(|| lgwks_std::id::Uuid::from([7_u8; 16]));
/// assert_eq!(fixed(), fixed());
/// ```
pub type IdFn = Arc<dyn Fn() -> Uuid + Send + Sync>;

/// Injected ML grant verifier (tests decide; production verifies Ed25519).
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::broker::{GrantVerificationResult, SignedMlGrant, VerifyGrantFn};
/// let reject: VerifyGrantFn = Arc::new(|_| GrantVerificationResult::InvalidSignature);
/// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![Capability::TapeRead], 9, 1, vec![]);
/// assert_eq!(reject(&grant), GrantVerificationResult::InvalidSignature);
/// ```
pub type VerifyGrantFn = Arc<dyn Fn(&SignedMlGrant) -> GrantVerificationResult + Send + Sync>;

/// Injected grant store: does this session hold this permission? Asked at
/// every gated decision, so a flip between two calls decides the second.
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::broker::{GrantFn, Permission};
/// let deny_all: GrantFn = Arc::new(|_, _| false);
/// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
/// assert!(!deny_all(&session, &Permission::Capability(Capability::IntentEmit)));
/// # Ok::<(), drishti::cli::Failure>(())
/// ```
pub type GrantFn = Arc<dyn Fn(&SessionInvariant, &Permission) -> bool + Send + Sync>;

/// Injected audit sink: sees every decision, required at construction.
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::native_host::broker::{AuditEvent, AuditFn};
/// let sink: AuditFn = Arc::new(|event: &AuditEvent| drop(event.seq()));
/// assert_eq!(Arc::strong_count(&sink), 1);
/// ```
pub type AuditFn = Arc<dyn Fn(&AuditEvent) + Send + Sync>;

/// ML grant verification outcome.
/// # Examples
/// ```
/// use drishti::native_host::broker::GrantVerificationResult;
/// assert_ne!(GrantVerificationResult::Valid, GrantVerificationResult::Expired);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GrantVerificationResult {
    /// Grant verified.
    Valid,
    /// Signature check failed.
    InvalidSignature,
    /// Grant expired.
    Expired,
    /// Grant holds a capability the peer may not hold.
    CapabilityNotPermitted(Capability),
    /// Grant is structurally invalid.
    InvalidGrant,
}

/// Signed ML grant. The signature covers the grant fields; verification is
/// injected so tests decide and production checks Ed25519.
///
/// Fields are private: construction runs through [`SignedMlGrant::new`] and
/// reads go through the accessors.
/// # Examples
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::broker::SignedMlGrant;
/// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![Capability::TapeRead], 9, 1, vec![0xAA]);
/// assert_eq!(grant.tenant_id(), "t");
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SignedMlGrant {
    /// Grant identity.
    grant_id: Uuid,
    /// Tenant the grant is scoped to.
    tenant_id: String,
    /// Granted capabilities (must include an ML-only one).
    capabilities: Vec<Capability>,
    /// Issue time in millisecond time.
    issued_at_ms: u64,
    /// Expiry time in millisecond time.
    expires_at_ms: u64,
    /// Signature bytes over the grant fields.
    signature: Vec<u8>,
}

impl SignedMlGrant {
    /// Assemble a grant.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::native_host::broker::SignedMlGrant;
    /// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![Capability::ViewInject], 9_000, 1_000, vec![1, 2]);
    /// assert_eq!((grant.issued_at_ms(), grant.expires_at_ms()), (1_000, 9_000));
    /// ```
    #[must_use]
    pub fn new(
        grant_id: Uuid,
        tenant_id: String,
        capabilities: Vec<Capability>,
        expires_at_ms: u64,
        issued_at_ms: u64,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            grant_id,
            tenant_id,
            capabilities,
            expires_at_ms,
            issued_at_ms,
            signature,
        }
    }

    /// Unique id the grant issuer assigned; copied out, never borrowed.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::SignedMlGrant;
    /// let id = lgwks_std::id::Uuid::from([3_u8; 16]);
    /// let grant = SignedMlGrant::new(id, "t".into(), vec![], 9, 1, vec![]);
    /// assert_eq!(grant.grant_id(), id);
    /// ```
    #[must_use]
    pub const fn grant_id(&self) -> Uuid {
        self.grant_id
    }

    /// Tenant the grant is scoped to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::SignedMlGrant;
    /// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "tenant-a".into(), vec![], 9, 1, vec![]);
    /// assert_eq!(grant.tenant_id(), "tenant-a");
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Capabilities the grant conveys; registration requires an ML-only one.
    /// # Examples
    /// ```
    /// # use drishti::canvas_protocol::Capability;
    /// # use drishti::native_host::broker::SignedMlGrant;
    /// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![Capability::TapeRead], 9, 1, vec![]);
    /// assert_eq!(grant.capabilities(), [Capability::TapeRead]);
    /// ```
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Issue time in millisecond time.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::SignedMlGrant;
    /// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![], 9, 4, vec![]);
    /// assert_eq!(grant.issued_at_ms(), 4);
    /// ```
    #[must_use]
    pub const fn issued_at_ms(&self) -> u64 {
        self.issued_at_ms
    }

    /// Expiry time in millisecond time.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::SignedMlGrant;
    /// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![], 9, 4, vec![]);
    /// assert_eq!(grant.expires_at_ms(), 9);
    /// ```
    #[must_use]
    pub const fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    /// Signature bytes over the grant fields.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::SignedMlGrant;
    /// let grant = SignedMlGrant::new(lgwks_std::id::Uuid::from([1_u8; 16]), "t".into(), vec![], 9, 4, vec![7, 8]);
    /// assert_eq!(grant.signature(), [7, 8]);
    /// ```
    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }
}
