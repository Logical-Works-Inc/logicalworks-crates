//! The supported surface for a consumer building its own durable runtime.
//!
//! `script!` is the way to write a flow. A consumer that hosts flows needs the parts
//! underneath them, and it is these eight modules, which carry the same
//! compatibility promise as `script!`: a removal or a changed signature is a major
//! release of this crate. An item marked `#[doc(hidden)]` inside one of them stays
//! outside that promise, and so does every other module of the crate.
//!
//! Each module here is a path to the same items, not a copy, so a type named through
//! `kernel` and one named by its support path are one type. ADR 0021 records the
//! choice. `tests/it/kernel_surface.rs` names items from every module through these
//! paths, so taking one away fails to compile.

/// The declared clock, and the epoch-anchored [`RecordedInstant`](clock::RecordedInstant).
pub mod clock {
    pub use crate::clock::*;
}

/// Monotonic deadlines, wall timestamps and the bound on how far two clocks disagree.
pub mod skew {
    pub use crate::skew::*;
}

/// The durable journal, for the effect ladder and for a record the consumer defines.
pub mod journal {
    pub use crate::journal::*;
}

/// Which environments a run owns, and the authority to hand an attempt to one.
pub mod broker {
    pub use crate::broker::*;
}

/// The cross-host lease with fencing epochs, and the coordinator behind a dispatch.
pub mod dist_lease {
    pub use crate::dist_lease::*;
}

/// Grant sets: admission when a bot is built, and a proof minted on each tick.
pub mod gate {
    pub use crate::gate::*;
}

/// Durable effect identity: the exact key a settlement is a statement about.
pub mod effect {
    pub use crate::effect::*;
}

/// Whether a failed attempt may be tried again.
pub mod retry {
    pub use crate::retry::*;
}
