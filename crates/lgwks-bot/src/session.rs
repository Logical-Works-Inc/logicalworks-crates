//! Validated declarative guidance flows and a synchronous session runner.
//!
//! The module deliberately has no relation to [`crate::domain::flow::Pipeline`].
//! A pipeline composes capability-gated effects; this module interprets a
//! human-facing, validated graph and records the cursor's conversation.
//!
//! # Resource bounds
//!
//! Validation bounds the *shape* of a document and [`FlowBounds`](crate::session::FlowBounds) bounds the
//! number of *steps* a session takes. Neither bounds bytes, and the two are
//! independent: `${value}` written a hundred thousand times is a
//! two-hundred-kilobyte document that interpolates to ten gigabytes, and a
//! step budget of two hundred and fifty-six does not notice, because a single
//! `say` node performs one step however much text it renders.
//!
//! [`ResourceLimits`](crate::session::ResourceLimits) is the byte bound, and it is enforced in four places:
//!
//! - [`MAX_UTTERANCE_BYTES`](crate::session::MAX_UTTERANCE_BYTES) — one answer, checked at the ingress of
//!   [`Session::answer`](crate::session::Session::answer) before the utterance is resolved, stored, or recorded;
//! - [`MAX_VALUE_BYTES`](crate::session::MAX_VALUE_BYTES) — one stored value, checked when an answer is assigned
//!   and again when the document names a candidate no session could store;
//! - [`MAX_RECORD_BYTES`](crate::session::MAX_RECORD_BYTES) — one rendered record payload, checked against the
//!   compiled template's *computed* expansion before that expansion is
//!   allocated ([`CompiledTemplate::expanded_bytes`](crate::session::CompiledTemplate::expanded_bytes)) and against the built
//!   prompt's computed size before the prompt is built;
//! - [`MAX_SESSION_BYTES`](crate::session::MAX_SESSION_BYTES) — every record and every visited node id a session
//!   retains, checked before either sink is written.
//!
//! Every one of them refuses rather than truncates: a record cut in half is a
//! record the journal claims happened and nobody can read. The refusal is
//! always before the write, so a rejected expansion leaves no partial journal
//! entry behind.
//!
//! These are the *operator's* ceilings. A document may ask for tighter ones in
//! its [`FlowBounds`](crate::session::FlowBounds), and an embedding application may ask for tighter ones on
//! a session ([`Session::with_limits`](crate::session::Session::with_limits)); neither may ask for looser, and a
//! request above the ceiling is [`SessionError::ResourceLimitAboveCeiling`](crate::session::SessionError::ResourceLimitAboveCeiling) rather
//! than a silently applied bound. The two requests combine by taking the
//! smaller value on each axis.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::num::IntErrorKind;

use lgwks_std::hash::Hasher;
use lgwks_std::json::serde;
use lgwks_std::json::{Deserialize, Serialize};

use crate::semantic::EmbedderIdentity;

mod error;
mod graph;
mod limits;
mod parse;
mod receipt;
mod resolve;
mod runner;
mod scope;
#[cfg(test)]
mod tests;
mod validate;
mod value;

pub use error::SessionError;
pub use graph::*;
pub use limits::*;
pub use receipt::*;
pub use resolve::*;
pub use runner::*;
pub use scope::*;
pub use value::*;

// The leaves with nothing public re-export nothing; the siblings reach their
// crate-private items through the root, so they are named here once.
#[cfg(feature = "ron")]
use parse::parse_ron;
use parse::{
    DeclaredNodes, check_flow_size, malformed_flow, merge_read_kinds, parse_json,
    reject_unknown_node_kinds,
};
use validate::{
    bound, valid_identifier, validate_declarations, validate_flow, validate_flow_within,
};

/// Flow validation entry point.
pub mod flow {
    use super::{FlowSpec, SessionError, validate_flow};

    /// Validate every structural invariant required before a session can run.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{FlowBounds, FlowSpec, NodeKind, flow};
    ///
    /// let nodes = BTreeMap::from([("end".to_owned(), NodeKind::End)]);
    /// let spec = FlowSpec::from_nodes(BTreeMap::new(), "end", nodes, FlowBounds::new(4))?;
    /// assert!(flow::validate(&spec).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn validate(spec: &FlowSpec) -> Result<(), SessionError> {
        validate_flow(spec)
    }
}
