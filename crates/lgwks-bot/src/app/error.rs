//! Why an [`App`](super::App) was refused.

use core::fmt;

use crate::cap::Cap;

/// Why an [`App`](super::App) could not be checked, built from a plan, or
/// served.
#[derive(Debug)]
#[non_exhaustive]
pub enum AppError {
    /// The app name is empty or past 64 bytes.
    BadName {
        /// Its length in bytes.
        len: usize,
    },
    /// More routes or `on` handlers than 256.
    TooMany {
        /// `"routes"` or `"handlers"`.
        what: &'static str,
        /// How many there are.
        len: usize,
    },
    /// The plan is past 64 KiB.
    PlanTooLarge {
        /// Its length in bytes.
        bytes: usize,
    },
    /// The plan is not a route document, or route `at` is malformed.
    MalformedPlan {
        /// The offending route, or `None` for the document itself.
        at: Option<usize>,
    },
    /// Two routes share one verb and path.
    DuplicateRoute {
        /// The earlier route.
        first: usize,
        /// The later route.
        second: usize,
    },
    /// Handlers return effects whose caps the app was not granted.
    MissingCaps {
        /// Routes that need a missing cap, by index.
        routes: Vec<usize>,
        /// `on` handlers that need a missing cap, by index.
        handlers: Vec<usize>,
        /// The distinct missing caps.
        caps: Vec<Cap>,
    },
    /// A cell worker could not start.
    Spawn {
        /// The operating system's refusal.
        cause: std::io::Error,
    },
    /// The listener or the driver failed.
    ServeIo {
        /// The operating system's refusal.
        cause: std::io::Error,
    },
    /// The host refused to start for a reason other than a spawn.
    Closed,
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadName { len } => write!(f, "the app name is {len} bytes; it must be 1..=64"),
            Self::TooMany { what, len } => write!(f, "the app holds {len} {what}; at most 256"),
            Self::PlanTooLarge { bytes } => write!(f, "the plan is {bytes} bytes; at most 65536"),
            Self::MalformedPlan { at: Some(at) } => write!(f, "plan route {at} is malformed"),
            Self::MalformedPlan { at: None } => f.write_str("the plan is not a route document"),
            Self::DuplicateRoute { first, second } => {
                write!(f, "routes {first} and {second} share one verb and path")
            }
            Self::MissingCaps {
                ref routes,
                ref handlers,
                ref caps,
            } => write!(
                f,
                "not granted {caps:?}, needed by routes {routes:?} and handlers {handlers:?}"
            ),
            Self::Spawn { ref cause } => write!(f, "an app cell worker could not start: {cause}"),
            Self::ServeIo { ref cause } => write!(f, "the app listener failed: {cause}"),
            Self::Closed => f.write_str("the app host refused to start"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Spawn { ref cause } | Self::ServeIo { ref cause } => Some(cause),
            _ => None,
        }
    }
}

/// Log `error` and hand it back, so every refusal leaves a record.
pub(super) fn refused<T>(error: AppError) -> Result<T, AppError> {
    lgwks_std::trace::warn!(%error, "app: refused");
    Err(error)
}
