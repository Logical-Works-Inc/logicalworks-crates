//! The run state a host attaches to a run's root scope: its authority, its
//! durable store, its definition identity and its effect policy.
//!
//! Kept beside [`Scope`]'s path, tenant and cancellation because it is a second
//! responsibility: how a run is identified and which rules it answers to,
//! rather than where in the flow it is. All of it rides on the scope because a
//! task-local does not cross a spawn, so a step polled on a spawned task would
//! otherwise lose it (#909, ADR 0045, ADR 0049).

use std::sync::Arc;

use crate::task::DefinitionIdentity;

use super::super::approval_policy::EffectPolicy;
use super::super::run_store::{Authority, Records};
use super::{Scope, ScopeInner};

impl Scope {
    /// This root scope, carrying the run state `authority`, `records` and
    /// `definition` from now on.
    ///
    /// The store and definition ride here beside the authority because a
    /// task-local does not cross a spawn: a step polled on a spawned task would
    /// otherwise run unrecorded and replay under a definition its run never
    /// declared (#909).
    ///
    /// Crate-private because the host is the one issuer: a caller that could
    /// attach its own authority would grant itself what `require` asks about,
    /// and one that could attach its own store would record under a run it
    /// never minted.
    pub(crate) fn hosted(
        &self,
        authority: Authority,
        records: Option<Records>,
        definition: Arc<DefinitionIdentity>,
    ) -> Self {
        Self {
            inner: Arc::new(ScopeInner {
                authority: Some(authority),
                records,
                definition: Some(definition),
                ..ScopeInner::clone(&self.inner)
            }),
        }
    }

    /// The authority the host admitted this run under, when one did.
    pub(crate) fn authority(&self) -> Option<&Authority> {
        self.inner.authority.as_ref()
    }

    /// The store the host attached to this run, when it attached one.
    pub(crate) fn records(&self) -> Option<&Records> {
        self.inner.records.as_ref()
    }

    /// The definition identity the host attached to this run, when it attached
    /// one.
    pub(crate) fn definition(&self) -> Option<&Arc<DefinitionIdentity>> {
        self.inner.definition.as_ref()
    }

    /// The effect policy this run's `act` steps answer to, when the flow
    /// installed one.
    pub(crate) fn effect_policy(&self) -> Option<&Arc<dyn EffectPolicy>> {
        self.inner.effect_policy.as_ref()
    }
}
