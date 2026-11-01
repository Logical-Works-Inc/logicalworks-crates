//! The capabilities the outside words declare, checked before the first step.
//!
//! `fetch`, `ask` and `send` each reach outside the process — an upstream, a
//! model, a receiver — so each declares the capability it needs as an
//! [`OutsideWord`]. [`admit_outside`] checks every word a flow names against
//! the run's authority in one pass, before the flow's first step, and refuses
//! with one [`FlowError::Blocked`] carrying the whole shortfall rather than
//! the first missing grant: the repair ticket is written once, not once per
//! word. That is the entry half; each word's own step checks its own grant
//! again through [`Scope::require`](crate::script::Scope::require), so a step
//! reached without the entry check is still refused at the step.

use crate::cap::Cap;

use super::{FlowError, Scope};

/// One outside word a flow names, and the capability it needs.
///
/// Built by the `script!` expansion at a flow's entry, one per `fetch`,
/// `ask` and `send` the flow writes, so the admission names every outbound
/// reach before the run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutsideWord {
    /// `fetch`: an HTTP call through `lgwks_std::http`, needing `bot.net`.
    Fetch,
    /// `ask`: a model call through the `proposal` door, needing `bot.model`.
    Ask,
    /// `send`: an outbound effect, needing `bot.notify`.
    Send,
}

impl OutsideWord {
    /// The capability this word needs.
    ///
    /// `fetch` dials out, so it needs the network; `ask` calls a model, which
    /// is its own capability rather than the network, so a grant for endpoints
    /// is not a grant for models; `send` hands an effect out — mail, a webhook,
    /// a payment — so it needs notification delivery.
    #[must_use]
    pub(crate) fn capability(&self) -> Cap {
        match *self {
            Self::Fetch => Cap::net(),
            Self::Ask => Cap::new("bot.model"),
            Self::Send => Cap::notify(),
        }
    }
}

/// Refuse the run `scope` is running unless its host's grants cover every
/// word in `uses`: `bot.net` for `fetch`, `bot.model` for `ask` and
/// `bot.notify` for `send`.
///
/// One pass over all of them, so the refusal names every missing grant at
/// once; called by the expansion at the flow's entry, before its first step.
///
/// # Errors
///
/// [`FlowError::Blocked`] carrying the whole shortfall.
pub fn admit_outside(scope: &Scope, uses: &[OutsideWord]) -> Result<(), FlowError> {
    let mut caps: Vec<Cap> = Vec::with_capacity(uses.len());
    for used in uses {
        let cap = used.capability();
        if !caps.iter().any(|held| held == &cap) {
            caps.push(cap);
        }
    }
    if caps.is_empty() {
        return Ok(());
    }
    scope.require(&caps).map_err(|error| {
        lgwks_std::trace::debug!(
            ?error,
            "admit_outside: the entry refuses the whole shortfall"
        );
        error
    })
}
