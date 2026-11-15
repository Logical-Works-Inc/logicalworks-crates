//! Capability guards for the native host.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasProtocol/
//! CapabilityGuard.swift` (kernel#850 row 15). Mirrors the kernel's
//! capability sets over the absorbed [`crate::canvas_protocol::Capability`].

use lgwks_macros::Error;

use crate::canvas_protocol::Capability;

/// True for capabilities only the ML layer may hold.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::capability::is_ml_only;
///
/// assert!(is_ml_only(&Capability::TapeRead));
/// // The command bar may hold intent.emit, so it is not ML-only.
/// assert!(!is_ml_only(&Capability::IntentEmit));
/// ```
#[must_use]
pub const fn is_ml_only(capability: &Capability) -> bool {
    matches!(capability, Capability::TapeRead | Capability::ViewInject)
}

/// True for capabilities requiring elevated trust (command bar or ML only).
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::capability::is_privileged;
///
/// assert!(is_privileged(&Capability::IntentEmit));
/// assert!(!is_privileged(&Capability::SignalEmit));
/// ```
#[must_use]
pub const fn is_privileged(capability: &Capability) -> bool {
    matches!(
        capability,
        Capability::IntentEmit | Capability::TapeRead | Capability::ViewInject
    )
}

/// Capability assertion failure.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::capability::{CapabilityError, assert_capability};
///
/// assert_eq!(
///     assert_capability(&[], Capability::TapeRead),
///     Err(CapabilityError::NotGranted(Capability::TapeRead))
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CapabilityError {
    /// The required capability is not in the granted set.
    #[error("capability not granted: {0}")]
    NotGranted(Capability),
}

/// Succeeds when `need` is in `have`; fails with the missing capability
/// otherwise.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::capability::assert_capability;
///
/// let granted = [Capability::SignalEmit];
/// assert!(assert_capability(&granted, Capability::SignalEmit).is_ok());
/// assert!(assert_capability(&granted, Capability::SignalSubscribe).is_err());
/// ```
pub fn assert_capability(have: &[Capability], need: Capability) -> Result<(), CapabilityError> {
    if have.iter().any(|held| held == &need) {
        Ok(())
    } else {
        Err(CapabilityError::NotGranted(need))
    }
}
