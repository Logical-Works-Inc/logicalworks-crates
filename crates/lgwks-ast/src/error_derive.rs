//! Zero-syn procedural macro expansion engine for `#[derive(Error)]` (#842).
//!
//! Parses an enum or struct token stream using `proc_macro2` and emits implementations
//! of [`core::fmt::Display`], [`core::error::Error`] (with `source(&self)`), and
//! [`core::convert::From`] for any fields marked with `#[from]`.

mod cursor;
mod expand;
mod format;
mod parse;

pub use cursor::ErrorDeriveError;
pub use expand::expand;
