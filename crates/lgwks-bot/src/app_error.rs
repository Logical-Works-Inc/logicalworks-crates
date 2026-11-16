//! The app layer's error: one type a consumer holds for any failure this crate
//! raises, as a sum of the layer errors rather than a flat list of variants.
//!
//! Each layer defines the error it raises where it raises it (INV-BOT-ERROR-TYPED,
//! ADR 0061), and a layer that sits above another wraps it with `From`. This is
//! the top of that stack. Its variants name one layer each, so a failure keeps the
//! type of the layer that raised it, and a consumer that matches here matches on
//! the layer first and on the variant second.
//!
//! The wrapper is transparent. `Display` writes the wrapped error's text and
//! `source` is the wrapped error's source, so wrapping adds no line to a report
//! and no hop to a chain. `retry_class` and `dispatch_certainty` delegate, so a
//! failure reads the same through every layer it travels.
//!
//! It lives in its own module, ungated, because the layer errors it names are
//! built in every configuration: the `script` layer is the only one behind a
//! feature, and its variant is gated with it.

use std::fmt;

#[cfg(feature = "script")]
use crate::script::FlowError;
use crate::{
    ecs::EcsError,
    error::{DispatchCertainty, KernelError, RetryClass},
    session::SessionError,
};

/// Any failure this crate raises, tagged with the layer that raised it.
///
/// `#[non_exhaustive]`: a layer is added when the crate gains one, and a consumer
/// matching on this must keep a wildcard arm so that is a compile-time prompt
/// there rather than a silent fallthrough.
///
/// # Examples
///
/// A kernel failure rides in the `Kernel` arm and prints as the kernel's own line.
///
/// ```
/// use lgwks_bot::app_error::BotError;
/// use lgwks_bot::error::KernelError;
///
/// let kernel = KernelError::EffectIndeterminate {
///     domain: String::from("gh::issue"),
///     cause: String::from("timed out"),
/// };
/// let line = kernel.to_string();
/// let failure = BotError::from(kernel);
/// assert!(matches!(failure, BotError::Kernel(_)));
/// assert_eq!(failure.to_string(), line);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum BotError {
    /// A kernel failure: authority, conflict, domain and dispatch refusals.
    Kernel(KernelError),
    /// A world-layer failure: the verbs, the spec, the registry and the tick.
    World(EcsError),
    /// A session-layer failure: a flow refused at load, or a session step refused.
    Session(SessionError),
    /// A script-layer failure: a flow step, located by its path.
    #[cfg(feature = "script")]
    Script(FlowError),
}

impl BotError {
    /// What this failure establishes about the effect it was making.
    ///
    /// Delegates to the layer error it wraps.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app_error::BotError;
    /// use lgwks_bot::error::{DispatchCertainty, KernelError};
    ///
    /// let failure = BotError::from(KernelError::EffectIndeterminate {
    ///     domain: String::from("gh::issue"),
    ///     cause: String::from("the reply was lost"),
    /// });
    /// // The effect may exist: a retry is a possible duplicate.
    /// assert_eq!(failure.dispatch_certainty(), DispatchCertainty::Unsettled);
    /// ```
    #[must_use]
    pub fn dispatch_certainty(&self) -> DispatchCertainty {
        match *self {
            Self::Kernel(ref source) => source.dispatch_certainty(),
            Self::World(ref source) => source.dispatch_certainty(),
            Self::Session(ref source) => source.dispatch_certainty(),
            #[cfg(feature = "script")]
            Self::Script(ref source) => source.dispatch_certainty(),
        }
    }

    /// What this failure permits a caller to do about it.
    ///
    /// Delegates to the layer error it wraps, so a failure keeps its class through
    /// every layer.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app_error::BotError;
    /// use lgwks_bot::ecs::EcsError;
    /// use lgwks_bot::error::RetryClass;
    ///
    /// let failure = BotError::from(EcsError::TickInsideRuntime);
    /// assert_eq!(failure.retry_class(), RetryClass::Never);
    /// ```
    #[must_use]
    pub fn retry_class(&self) -> RetryClass {
        match *self {
            Self::Kernel(ref source) => source.retry_class(),
            Self::World(ref source) => source.retry_class(),
            Self::Session(ref source) => source.retry_class(),
            #[cfg(feature = "script")]
            Self::Script(ref source) => source.retry_class(),
        }
    }
}

impl fmt::Display for BotError {
    /// The wrapped error's own text, unchanged.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Kernel(ref source) => fmt::Display::fmt(source, formatter),
            Self::World(ref source) => fmt::Display::fmt(source, formatter),
            Self::Session(ref source) => fmt::Display::fmt(source, formatter),
            #[cfg(feature = "script")]
            Self::Script(ref source) => fmt::Display::fmt(source, formatter),
        }
    }
}

impl std::error::Error for BotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Kernel(ref source) => std::error::Error::source(source),
            Self::World(ref source) => std::error::Error::source(source),
            Self::Session(ref source) => std::error::Error::source(source),
            #[cfg(feature = "script")]
            Self::Script(ref source) => std::error::Error::source(source),
        }
    }
}

impl From<KernelError> for BotError {
    /// Lets `?` carry a kernel failure up to the app layer unchanged.
    fn from(source: KernelError) -> Self {
        Self::Kernel(source)
    }
}

impl From<EcsError> for BotError {
    /// Lets `?` carry a world-layer failure up to the app layer unchanged.
    fn from(source: EcsError) -> Self {
        Self::World(source)
    }
}

impl From<SessionError> for BotError {
    /// Lets `?` carry a session-layer failure up to the app layer unchanged.
    fn from(source: SessionError) -> Self {
        Self::Session(source)
    }
}

#[cfg(feature = "script")]
impl From<FlowError> for BotError {
    /// Lets `?` carry a script-layer failure up to the app layer unchanged.
    fn from(source: FlowError) -> Self {
        Self::Script(source)
    }
}
