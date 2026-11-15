//! The presentation lease: who may present, on which surface, within what
//! envelope.

use std::sync::Arc;

use super::model::{Error, Limits};
use crate::canvas_protocol::SessionInvariant;

/// Versioned presentation lease binding principal, workspace/session,
/// instance generation, surface id, resource envelope, and allowed local
/// interactions.
///
/// Fields are private: construction runs through the validating
/// [`PresentationLease::new`] and reads go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::SessionInvariant;
/// use drishti::canvas_scene::{Limits, PresentationLease};
///
/// let id = "11111111-1111-1111-1111-111111111111";
/// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
/// let lease = PresentationLease::new("lease-1", "surface-1", session, 2, Limits::default())?;
/// assert_eq!((lease.lease_id(), lease.surface_id(), lease.generation()), ("lease-1", "surface-1", 2));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresentationLease {
    /// Lease schema version, always 1.
    schema_version: u32,
    /// Lease identity.
    lease_id: Arc<str>,
    /// Surface identity.
    surface_id: Arc<str>,
    /// Bound session.
    session: SessionInvariant,
    /// Instance generation, always positive.
    generation: u64,
    /// Resource envelope.
    limits: Limits,
}

impl PresentationLease {
    /// Bind a lease, validating it immediately.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for empty or overlong id parts,
    /// control characters, zero generation, or an invalid envelope.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    /// use drishti::canvas_scene::{Error, Limits, PresentationLease};
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let refused = PresentationLease::new("lease-1", "surface-1", session, 0, Limits::default());
    /// assert_eq!(refused.err(), Some(Error::InvalidInput));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        lease_id: &str,
        surface_id: &str,
        session: SessionInvariant,
        generation: u64,
        limits: Limits,
    ) -> Result<Self, Error> {
        let lease = Self {
            schema_version: 1,
            lease_id: Arc::from(lease_id),
            surface_id: Arc::from(surface_id),
            session,
            generation,
            limits,
        };
        lease.validate()?;
        Ok(lease)
    }

    /// Validate again at admission: public fields are caller-controlled data.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for any malformed field.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    /// use drishti::canvas_scene::{Limits, PresentationLease};
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let lease = PresentationLease::new("lease-1", "surface-1", session, 1, Limits::default())?;
    /// assert_eq!(lease.validate(), Ok(()));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn validate(&self) -> Result<(), Error> {
        self.limits.validate()?;
        Error::require(
            self.schema_version == 1 && self.generation > 0,
            Error::InvalidInput,
        )?;
        // A session decoded from the wire skipped the constructor's shape
        // checks (UUID identities, bounded tenant); make them again here.
        Error::require(self.session.check_shape().is_ok(), Error::InvalidInput)?;
        for part in [
            self.lease_id.as_ref(),
            self.surface_id.as_ref(),
            self.session.work_object_id(),
            self.session.session_id(),
            self.session.policy_bundle_id(),
            self.session.tenant_id(),
            self.session.user_sub(),
        ] {
            Error::require(
                !part.is_empty() && part.len() <= 512 && !part.chars().any(char::is_control),
                Error::InvalidInput,
            )?;
        }
        Ok(())
    }

    /// Lease schema version, always 1.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Limits, PresentationLease};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let lease = PresentationLease::new("lease-1", "surface-1", session, 1, Limits::default())?;
    /// assert_eq!(lease.schema_version(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Identity of this lease, unique per grant, as passed to
    /// [`PresentationLease::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Limits, PresentationLease};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let lease = PresentationLease::new("lease-9", "surface-1", session, 1, Limits::default())?;
    /// assert_eq!(lease.lease_id(), "lease-9");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn lease_id(&self) -> &str {
        &self.lease_id
    }

    /// Identity of the display surface the lease authorizes presenting on.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Limits, PresentationLease};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let lease = PresentationLease::new("lease-1", "surface-7", session, 1, Limits::default())?;
    /// assert_eq!(lease.surface_id(), "surface-7");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    /// Session the lease was granted to; deltas must carry an equal session.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Limits, PresentationLease};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "tenant-z".into(), id.into())?;
    /// let lease = PresentationLease::new("lease-1", "surface-1", session.clone(), 1, Limits::default())?;
    /// assert_eq!(lease.session(), &session);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn session(&self) -> &SessionInvariant {
        &self.session
    }

    /// Instance generation the lease binds; always positive, and a delta
    /// naming another generation is stale.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Limits, PresentationLease};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let lease = PresentationLease::new("lease-1", "surface-1", session, 5, Limits::default())?;
    /// assert_eq!(lease.generation(), 5);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Resource envelope.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Limits, PresentationLease};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let limits = Limits::new(2048, 4, 1000)?;
    /// let lease = PresentationLease::new("lease-1", "surface-1", session, 1, limits)?;
    /// assert_eq!(lease.limits(), limits);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }
}
