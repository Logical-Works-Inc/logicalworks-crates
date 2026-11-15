//! Versioned Logic OS Canvas protocol vocabulary, absorbed from the kernel.
//!
//! Source: `kernel/crates/canvas-protocol` in logic-os-kernel (row 15 of the
//! absorption audit, kernel#850). [`Capability`] is the single source of truth
//! for every kernel capability check; the closed [`NodeKind`] taxonomy fixes
//! both the strongest action a node may perform and how sensitive the data it
//! may hold is. The JSON Schema files under `assets/canvas-protocol/schema`
//! remain the wire-format source of truth; [`workspace_layout`] gives Rust
//! consumers a fail-closed decoder plus a deterministic web-renderer
//! projection without granting storage, broker, or effect authority.

use lgwks_std::json::serde::{Deserialize, Serialize};

mod vocabulary;
pub mod workspace_layout;
mod workspace_vocabulary;

pub use vocabulary::{
    ActorType, Capability, CompensationStrategy, EgressDestination, LocalActionStatus,
    OfflineBehavior, SyncStatus, TrustBin,
};
pub use workspace_vocabulary::{
    ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme,
    WorkspaceToolIcon,
};

/// Canonical session invariant.
///
/// Represents the security context anchored to a specific work object and
/// session. Fields are private: construction runs through [`SessionInvariant::new`],
/// which validates shapes before binding them, and reads go through the
/// accessors so the wire shape can stay stable. Wire input decoded from JSON
/// bypasses the constructor (serde sets fields directly), so it must still
/// pass [`crate::native_host::protocol::validate_session_invariant`].
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::SessionInvariant;
///
/// let id = "11111111-1111-1111-1111-111111111111";
/// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "tenant-a".into(), id.into())?;
/// assert_eq!(session.tenant_id(), "tenant-a");
/// assert_eq!(session.clone(), session);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct SessionInvariant {
    /// Work object this session is scoped to (UUID).
    work_object_id: String,
    /// Session identity (UUID).
    session_id: String,
    /// Policy bundle governing the session (UUID).
    policy_bundle_id: String,
    /// Tenant the session belongs to (non-empty, at most 512 bytes).
    tenant_id: String,
    /// User subject the session acts for (UUID).
    user_sub: String,
}

/// Session-invariant shape failures.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::{SessionInvariant, SessionInvariantError};
///
/// let id = "11111111-1111-1111-1111-111111111111";
/// let refused = SessionInvariant::new(id.into(), id.into(), id.into(), String::new(), id.into());
/// assert_eq!(refused, Err(SessionInvariantError::InvalidTenantId));
/// assert_eq!(SessionInvariantError::InvalidTenantId.to_string(), "tenant_id is empty or over 512 bytes");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, lgwks_macros::Error)]
#[non_exhaustive]
pub enum SessionInvariantError {
    /// `work_object_id` is not a UUID or exceeds 512 bytes.
    #[error("work_object_id is not a UUID")]
    InvalidWorkObjectId,
    /// `session_id` is not a UUID or exceeds 512 bytes.
    #[error("session_id is not a UUID")]
    InvalidSessionId,
    /// `policy_bundle_id` is not a UUID or exceeds 512 bytes.
    #[error("policy_bundle_id is not a UUID")]
    InvalidPolicyBundleId,
    /// `tenant_id` is empty or exceeds 512 bytes.
    #[error("tenant_id is empty or over 512 bytes")]
    InvalidTenantId,
    /// `user_sub` is not a UUID or exceeds 512 bytes.
    #[error("user_sub is not a UUID")]
    InvalidUserSub,
}

/// Maximum bytes for any invariant field.
const INVARIANT_FIELD_BYTES: usize = 512;

/// Reject a non-UUID or overlong identity field.
///
/// Canonical hyphenated 8-4-4-4-12 hex, enforced by the estate [`lgwks_std::id`]
/// parser (which, unlike the `uuid` crate, refuses the simple, braced, and URN
/// forms the conformance fixtures pin as invalid).
fn require_uuid(value: &str, error: SessionInvariantError) -> Result<(), SessionInvariantError> {
    (value.len() <= INVARIANT_FIELD_BYTES && lgwks_std::id::Uuid::parse(value).is_ok())
        .then_some(())
        .ok_or(error)
}

impl SessionInvariant {
    /// Bind a session to its work object, session, policy, tenant, and user,
    /// validating every shape before binding.
    ///
    /// # Errors
    ///
    /// Returns [`SessionInvariantError`] naming the first malformed field:
    /// work object, session, policy, and user must parse as UUIDs, and the
    /// tenant must be non-empty; every field is bounded to 512 bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::{SessionInvariant, SessionInvariantError};
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let bound = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into());
    /// assert!(bound.is_ok());
    /// let refused = SessionInvariant::new(id.into(), "nope".into(), id.into(), "t".into(), id.into());
    /// assert_eq!(refused, Err(SessionInvariantError::InvalidSessionId));
    /// ```
    pub fn new(
        work_object_id: String,
        session_id: String,
        policy_bundle_id: String,
        tenant_id: String,
        user_sub: String,
    ) -> Result<Self, SessionInvariantError> {
        let invariant = Self {
            work_object_id,
            session_id,
            policy_bundle_id,
            tenant_id,
            user_sub,
        };
        invariant.check_shape()?;
        Ok(invariant)
    }

    /// Re-check every field's shape, in constructor order. Wire input decoded
    /// from JSON skips [`SessionInvariant::new`], so admission calls this
    /// again.
    pub(crate) fn check_shape(&self) -> Result<(), SessionInvariantError> {
        require_uuid(
            &self.work_object_id,
            SessionInvariantError::InvalidWorkObjectId,
        )?;
        require_uuid(&self.session_id, SessionInvariantError::InvalidSessionId)?;
        require_uuid(
            &self.policy_bundle_id,
            SessionInvariantError::InvalidPolicyBundleId,
        )?;
        (!self.tenant_id.is_empty() && self.tenant_id.len() <= INVARIANT_FIELD_BYTES)
            .then_some(())
            .ok_or(SessionInvariantError::InvalidTenantId)?;
        require_uuid(&self.user_sub, SessionInvariantError::InvalidUserSub)
    }

    /// UUID of the work object every artifact presented in this session must
    /// be keyed by.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    ///
    /// let (work, id) = ("11111111-1111-1111-1111-111111111111", "22222222-2222-2222-2222-222222222222");
    /// let session = SessionInvariant::new(work.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// assert_eq!(session.work_object_id(), work);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn work_object_id(&self) -> &str {
        &self.work_object_id
    }

    /// UUID that tells this session apart from other sessions on the same
    /// work object.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    ///
    /// let (id, session_id) = ("11111111-1111-1111-1111-111111111111", "22222222-2222-2222-2222-222222222222");
    /// let session = SessionInvariant::new(id.into(), session_id.into(), id.into(), "t".into(), id.into())?;
    /// assert_eq!(session.session_id(), session_id);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// UUID of the policy bundle whose rules this session was admitted
    /// under.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    ///
    /// let (id, policy) = ("11111111-1111-1111-1111-111111111111", "33333333-3333-3333-3333-333333333333");
    /// let session = SessionInvariant::new(id.into(), id.into(), policy.into(), "t".into(), id.into())?;
    /// assert_eq!(session.policy_bundle_id(), policy);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn policy_bundle_id(&self) -> &str {
        &self.policy_bundle_id
    }

    /// Tenant that owns the session; the scope key every store and cache
    /// built on this session must carry.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "tenant-b".into(), id.into())?;
    /// assert_eq!(session.tenant_id(), "tenant-b");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// UUID of the user the session acts on behalf of (the token subject).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    ///
    /// let (id, user) = ("11111111-1111-1111-1111-111111111111", "44444444-4444-4444-4444-444444444444");
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), user.into())?;
    /// assert_eq!(session.user_sub(), user);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn user_sub(&self) -> &str {
        &self.user_sub
    }
}

/// The closed node-type taxonomy (#361 / A4.11, ADR-085). A node's *kind* fixes
/// both the strongest action it may perform ([`max_action_tier`](Self::max_action_tier))
/// and how sensitive the data it may hold is ([`data_exposure_level`](Self::data_exposure_level)).
/// These are two distinct axes — action strength vs data sensitivity — and the
/// cross-trust chain rules gate hops on the data-exposure axis.
///
/// Maps to physical trust boundaries: Vault → Apple Secure Enclave / TrustZone;
/// App → REE / WASM sandbox; VisionLayer → the metadata-only window onto a vault.
///
/// This enum is deliberately exhaustive (no wildcard): adding a kind forces
/// every trust gate to handle it at compile time.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::NodeKind;
///
/// let wire = lgwks_std::json::to_string(&NodeKind::VisionLayer)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"VisionLayer\"");
/// assert!(lgwks_std::json::from_str::<NodeKind>("\"Quantum\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum NodeKind {
    /// Hardware-isolated (Secure Enclave / SGX enclave). Holds raw vault-tier data;
    /// the highest data-exposure level.
    Vault,
    /// On-device processing engine. May read raw data to compute, but is
    /// machine-bound and never egresses raw data.
    DataEngine,
    /// Rich Execution Environment or bounded WASM sandbox. Receives only
    /// redacted data; the mediated path to outbound egress.
    App,
    /// The content-free / metadata-only window onto a vault. The ONLY node a
    /// speculative path may reach a vault through.
    VisionLayer,
    /// OS-level relay: no data access, only causal-hash routing.
    Relay,
}

impl NodeKind {
    /// The strongest action tier this node kind may hold (action-strength axis).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::NodeKind;
    ///
    /// assert_eq!(NodeKind::Vault.max_action_tier(), 3);
    /// assert_eq!(NodeKind::App.max_action_tier(), NodeKind::VisionLayer.max_action_tier());
    /// assert_eq!(NodeKind::Relay.max_action_tier(), 0);
    /// ```
    #[must_use]
    pub const fn max_action_tier(self) -> u8 {
        match self {
            Self::Vault => 3,
            Self::DataEngine => 2,
            Self::App | Self::VisionLayer => 1,
            Self::Relay => 0,
        }
    }

    /// The data-sensitivity level this node kind is trusted to hold (#361).
    ///
    /// Higher means more sensitive. Separate from [`max_action_tier`](Self::max_action_tier):
    /// a `VisionLayer` and an `App` can perform similar (Read-class) actions,
    /// but a VisionLayer is trusted with strictly less data (metadata only).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::NodeKind;
    ///
    /// assert_eq!(NodeKind::Vault.data_exposure_level(), 4);
    /// assert!(NodeKind::VisionLayer.data_exposure_level() < NodeKind::App.data_exposure_level());
    /// ```
    #[must_use]
    pub const fn data_exposure_level(self) -> u8 {
        match self {
            Self::Vault => 4,
            Self::DataEngine => 3,
            Self::App => 2,
            Self::VisionLayer => 1,
            Self::Relay => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NodeKind;

    /// Format a test failure reason for serde round-trips.
    fn round_trip(kind: NodeKind, wire: &str) -> Result<(), String> {
        let encoded = lgwks_std::json::to_string(&kind)
            .map_err(|error| format!("encode {kind:?}: {error}"))?;
        if encoded != wire {
            return Err(format!(
                "wire form drifted for {kind:?}: got {encoded}, want {wire}"
            ));
        }
        let back: NodeKind =
            lgwks_std::json::from_str(wire).map_err(|error| format!("decode {wire}: {error}"))?;
        if back != kind {
            return Err(format!("round-trip mismatch for {kind:?}"));
        }
        Ok(())
    }

    /// Wire-form golden lock. `NodeKind` has no schema-lock counterpart, so a
    /// rename would silently break cross-runtime peers; this pins the exact
    /// serialized strings plus closedness.
    #[test]
    fn node_kind_wire_form_is_pinned() -> Result<(), String> {
        for (kind, wire) in [
            (NodeKind::Vault, "\"Vault\""),
            (NodeKind::DataEngine, "\"DataEngine\""),
            (NodeKind::App, "\"App\""),
            (NodeKind::VisionLayer, "\"VisionLayer\""),
            (NodeKind::Relay, "\"Relay\""),
        ] {
            round_trip(kind, wire)?;
        }
        if lgwks_std::json::from_str::<NodeKind>("\"Quantum\"").is_ok() {
            return Err(String::from(
                "unknown kind from a future peer must fail closed",
            ));
        }
        Ok(())
    }
}
