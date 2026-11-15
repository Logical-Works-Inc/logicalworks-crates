//! Headless preview host: CLI, trace traversal, trace checker, desktop.
//!
//! Absorbed from `primitives/canvas-native/crates/preview` (kernel#850 row
//! 15). The `trace` module runs the scripted headless traversal used by the
//! `canvas_preview --headless` path; `check` is the Rust rewrite of
//! `tools/check_trace.py` (the Python is banned in product repos); `desktop`
//! and `paint` are the winit/softbuffer reference window behind the
//! default-off `desktop` feature.

pub mod check;
pub mod trace;

#[cfg(feature = "desktop")]
pub mod desktop;

#[cfg(feature = "desktop")]
pub mod paint;

/// Application failure for the preview host (single definition lives in
/// [`crate::cli`]).
pub use crate::cli::Failure;

pub use check::{TraceCheckError, check_records};
pub use trace::run_traversal;
