//! Deterministic motion core, absorbed from the kernel.
//!
//! Source: `primitives/canvas-motion/crates/motion-core` in logic-os-kernel
//! (kernel#850 row 15). Pure frame evaluation over a persistent motion graph:
//! tweens evaluate a closed easing at wall-clock time, accumulators integrate
//! spring/scrub/drag state held outside the graph, and patches apply
//! atomically with a version bump per success.
//!
//! Two deliberate adaptations from the kernel source, both behavior-preserving
//! on every absorbed test: [`graph`] stores nodes in a `BTreeMap` rather than
//! an `im::HashMap` (same persistent clone-on-write API, but iteration order
//! is key order, so replay traces are canonical), and [`frame()`] sorts nothing
//! because it no longer needs to. Integer arithmetic uses explicit
//! wrapping/saturating operations per this repository's lint contract.

pub mod clock;
pub mod easing;
pub mod error;
pub mod frame;
pub mod graph;
pub mod interpolate;
pub mod node;
pub mod patch;
pub mod scrub;

pub use clock::Clock;
pub use easing::{Easing, NamedCurve};
pub use error::{MotionError, MotionErrorCode};
pub use frame::{BindingValue, BindingWrite, Frame, TargetHandle, frame};
pub use graph::{ConflictPolicy, MotionGraph, MotionGraphBuilder, PropertyKey};
pub use interpolate::color_oklab::interp_oklab;
pub use interpolate::scalar::lerp_scalar;
pub use node::accumulator::{
    AccumulatorComponent, AccumulatorKind, AccumulatorState, AccumulatorStore,
};
pub use node::spring::{SpringConfig, integrate_spring};
pub use node::tween::{AnimValue, TweenComponent};
pub use node::{BindingTarget, MotionNode, NodeHandle, NodeKind};
pub use patch::{MotionPatch, PatchOp, PatchValue, apply_patch};
pub use scrub::{ConstantScrub, EmptyScrubMap, HashScrubMap, ScrubSource, ScrubSourceMap};
