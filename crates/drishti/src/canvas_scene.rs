//! Native canvas scene: bounded presentation data plus the single-owner reducer.
//!
//! Absorbed from `primitives/canvas-native/crates/canvas` (kernel#850 row
//! 15): `model` holds the bounded presentation types, `frame` the prepared
//! frame and its frame-local identity, `scene` the reducer with its bounded
//! inbox.

mod artifact;
mod delta;
pub mod frame;
mod lease;
pub mod model;
pub mod scene;

pub use frame::{Frame, FrameToken};
pub use model::{Action, Artifact, CommitStatus, Error, Limits, PresentationLease, View};
pub use scene::{Canvas, Delta, Statistics};
