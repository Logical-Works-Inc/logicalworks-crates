//! Capability and operation-status vocabulary of the Canvas protocol.
//!
//! Re-exported from [`crate::canvas_protocol`]; the wire strings here are the
//! ones the JSON Schema files pin.

use std::fmt;

use lgwks_std::json::serde::{Deserialize, Serialize};

/// Permission token.
///
/// `ARCH-02` establishes this as the single source of truth for all kernel
/// capability checks. The serde wire strings and the [`fmt::Display`] strings are
/// identical on purpose: both are pinned by tests against
/// `assets/canvas-protocol/schema/Capability.json`.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::Capability;
///
/// assert_eq!("signal.emit".parse::<Capability>(), Ok(Capability::SignalEmit));
/// assert_eq!(Capability::RemoteCompute.to_string(), "compute.remote");
/// assert!("signal.nope".parse::<Capability>().is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// Emit a signal onto the broker.
    #[serde(rename = "signal.emit")]
    SignalEmit,
    /// Subscribe to signals matching a pattern.
    #[serde(rename = "signal.subscribe")]
    SignalSubscribe,
    /// Read the governance tape.
    #[serde(rename = "tape.read")]
    TapeRead,
    /// Inject a view directive.
    #[serde(rename = "view.inject")]
    ViewInject,
    /// Emit a user intent from the command bar.
    #[serde(rename = "intent.emit")]
    IntentEmit,
    /// Schedule motion work.
    #[serde(rename = "motion.schedule")]
    MotionSchedule,
    /// Observe motion frames.
    #[serde(rename = "motion.observe")]
    MotionObserve,
    /// Patch a running motion node.
    #[serde(rename = "motion.patch")]
    MotionPatch,
    /// Register a motion plugin.
    #[serde(rename = "motion.plugin.register")]
    MotionPluginRegister,
    /// Replay a recorded motion trace.
    #[serde(rename = "motion.replay")]
    MotionReplay,
    /// Shred (efface) held data.
    #[serde(rename = "efface.shred")]
    Shred,
    /// Right-to-be-forgotten erasure.
    #[serde(rename = "efface.rtbf")]
    Rtbf,
    /// Outbound remote computation request (#447).
    #[serde(rename = "compute.remote")]
    RemoteCompute,
}

impl fmt::Display for Capability {
    /// Wire strings, identical to the serde forms on purpose: both are pinned
    /// by tests against `assets/canvas-protocol/schema/Capability.json`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match *self {
            Self::SignalEmit => "signal.emit",
            Self::SignalSubscribe => "signal.subscribe",
            Self::TapeRead => "tape.read",
            Self::ViewInject => "view.inject",
            Self::IntentEmit => "intent.emit",
            Self::MotionSchedule => "motion.schedule",
            Self::MotionObserve => "motion.observe",
            Self::MotionPatch => "motion.patch",
            Self::MotionPluginRegister => "motion.plugin.register",
            Self::MotionReplay => "motion.replay",
            Self::Shred => "efface.shred",
            Self::Rtbf => "efface.rtbf",
            Self::RemoteCompute => "compute.remote",
        };
        formatter.write_str(text)
    }
}

impl std::str::FromStr for Capability {
    /// The wire string itself on failure: unknown input carries no structured
    /// detail beyond the input, which the caller already holds.
    type Err = &'static str;

    /// Parse the wire strings the [`fmt::Display`] impl writes.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "signal.emit" => Ok(Self::SignalEmit),
            "signal.subscribe" => Ok(Self::SignalSubscribe),
            "tape.read" => Ok(Self::TapeRead),
            "view.inject" => Ok(Self::ViewInject),
            "intent.emit" => Ok(Self::IntentEmit),
            "motion.schedule" => Ok(Self::MotionSchedule),
            "motion.observe" => Ok(Self::MotionObserve),
            "motion.patch" => Ok(Self::MotionPatch),
            "motion.plugin.register" => Ok(Self::MotionPluginRegister),
            "motion.replay" => Ok(Self::MotionReplay),
            "efface.shred" => Ok(Self::Shred),
            "efface.rtbf" => Ok(Self::Rtbf),
            "compute.remote" => Ok(Self::RemoteCompute),
            _ => Err("unknown capability"),
        }
    }
}

impl Capability {
    /// Every capability in [`fmt::Display`] order, for schema-lock comparisons.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::Capability;
    ///
    /// assert_eq!(Capability::all().len(), 13);
    /// for capability in Capability::all() {
    ///     assert_eq!(capability.to_string().parse::<Capability>(), Ok(capability));
    /// }
    /// ```
    #[must_use]
    pub fn all() -> [Self; 13] {
        [
            Self::SignalEmit,
            Self::SignalSubscribe,
            Self::TapeRead,
            Self::ViewInject,
            Self::IntentEmit,
            Self::MotionSchedule,
            Self::MotionObserve,
            Self::MotionPatch,
            Self::MotionPluginRegister,
            Self::MotionReplay,
            Self::Shred,
            Self::Rtbf,
            Self::RemoteCompute,
        ]
    }
}

/// Typed outbound destinations for egress operations (#447).
///
/// Closed enum to prevent freeform exfiltration.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::EgressDestination;
///
/// let wire = lgwks_std::json::to_string(&EgressDestination::CloudRelay)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"cloudrelay\"");
/// assert!(lgwks_std::json::from_str::<EgressDestination>("\"pastebin\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum EgressDestination {
    /// The canonical Logic OS cloud relay.
    CloudRelay,
    /// Authorized audit-log collector.
    AuditVault,
}

/// Identifies who initiated an operation.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::ActorType;
///
/// let wire = lgwks_std::json::to_string(&ActorType::Agent).map_err(|error| error.to_string())?;
/// let back: ActorType = lgwks_std::json::from_str(&wire).map_err(|error| error.to_string())?;
/// assert_eq!(back, ActorType::Agent);
/// assert!(lgwks_std::json::from_str::<ActorType>("\"Robot\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum ActorType {
    /// A person acting through the UI.
    Human,
    /// An agent acting through admitted operations.
    Agent,
    /// The system itself.
    System,
}

/// Component trust levels.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::TrustBin;
///
/// let wire = lgwks_std::json::to_string(&TrustBin::Untrusted).map_err(|error| error.to_string())?;
/// let back: TrustBin = lgwks_std::json::from_str(&wire).map_err(|error| error.to_string())?;
/// assert_eq!(back, TrustBin::Untrusted);
/// assert!(lgwks_std::json::from_str::<TrustBin>("\"Trusted\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum TrustBin {
    /// First-party system code.
    System,
    /// Managed third-party code.
    Managed,
    /// Generated code under review.
    Generated,
    /// Untrusted guest code.
    Untrusted,
}

/// Lifecycle status of a local action.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::LocalActionStatus;
///
/// let wire = lgwks_std::json::to_string(&LocalActionStatus::Compensating)
///     .map_err(|error| error.to_string())?;
/// let back: LocalActionStatus =
///     lgwks_std::json::from_str(&wire).map_err(|error| error.to_string())?;
/// assert_eq!(back, LocalActionStatus::Compensating);
/// assert!(lgwks_std::json::from_str::<LocalActionStatus>("\"Paused\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum LocalActionStatus {
    /// Admitted but not started.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with failure.
    Failed,
    /// Undoing a partial effect.
    Compensating,
    /// Undone.
    Compensated,
}

/// Sync state with the cloud substrate.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::SyncStatus;
///
/// let wire = lgwks_std::json::to_string(&SyncStatus::ConflictDetected)
///     .map_err(|error| error.to_string())?;
/// let back: SyncStatus = lgwks_std::json::from_str(&wire).map_err(|error| error.to_string())?;
/// assert_eq!(back, SyncStatus::ConflictDetected);
/// assert!(lgwks_std::json::from_str::<SyncStatus>("\"Merged\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum SyncStatus {
    /// Never uploaded.
    NotSynced,
    /// Upload in flight.
    Uploading,
    /// Server acknowledged receipt.
    Acknowledged,
    /// Server reported a conflict.
    ConflictDetected,
}

/// Strategy for resolving failed actions.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::CompensationStrategy;
///
/// let wire = lgwks_std::json::to_string(&CompensationStrategy::Rollback)
///     .map_err(|error| error.to_string())?;
/// let back: CompensationStrategy =
///     lgwks_std::json::from_str(&wire).map_err(|error| error.to_string())?;
/// assert_eq!(back, CompensationStrategy::Rollback);
/// assert!(lgwks_std::json::from_str::<CompensationStrategy>("\"Ignore\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum CompensationStrategy {
    /// Leave the partial effect in place.
    None,
    /// Roll back to the pre-action snapshot.
    Rollback,
    /// Run a compensating action.
    Compensate,
    /// Record the failure and move on.
    Abandon,
}

/// Offline behavior for capability dispatches.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::OfflineBehavior;
///
/// let wire = lgwks_std::json::to_string(&OfflineBehavior::FailClosed)
///     .map_err(|error| error.to_string())?;
/// let back: OfflineBehavior =
///     lgwks_std::json::from_str(&wire).map_err(|error| error.to_string())?;
/// assert_eq!(back, OfflineBehavior::FailClosed);
/// assert!(lgwks_std::json::from_str::<OfflineBehavior>("\"FailOpen\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub enum OfflineBehavior {
    /// Hold locally until connectivity returns.
    QueueLocally,
    /// Refuse while offline.
    FailClosed,
    /// Serve a stale cache entry, marked as such.
    DegradeToCache,
    /// The operation has no offline mode.
    Unsupported,
}
