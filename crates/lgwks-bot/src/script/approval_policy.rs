//! Which actions a run may perform without asking, and which owe a decision.
//!
//! The policy classifies actions by their [`EffectLifetime`]; the run's `act`
//! enforces the classification before the action runs, so a gated action never
//! runs ungated and an ungated one costs no archive. The default gates every
//! action whose effect leaves the process.

use std::future::Future;
use std::sync::Arc;

use crate::rt::task_local;
use crate::verb::EffectLifetime;

task_local! {
    /// The policy a run's `act` steps are gated by, when the flow installed one.
    static EFFECT_POLICY: Option<Arc<dyn EffectPolicy>>;
}

/// What running an action costs its run: whether the run may proceed, must
/// hold a human decision first, or is over.
///
/// A disposition is about the *run*, not the action: the same action is
/// reversible under one policy and gated under another, and the policy is
/// what says which. The run's `act` asks the installed [`EffectPolicy`]
/// before the action runs, so a gated action never runs ungated and an
/// ungated one costs no decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffectDisposition {
    /// The action runs as written: local, or explicitly allowed.
    Reversible,
    /// The action runs only when the run holds an approval whose digest
    /// matches the archived input.
    NeedsConfirmation,
    /// The run is over unless the run holds a matching approval: the action
    /// ends something that cannot be resumed.
    Terminal,
}

/// Which actions a run may perform without asking, and which owe a decision.
///
/// The policy classifies actions; the run enforces the classification. A
/// policy that returns [`EffectDisposition::NeedsConfirmation`] or
/// [`EffectDisposition::Terminal`] for an action refuses that action at its
/// step unless the run holds a matching approval, decided through the host.
pub trait EffectPolicy: Send + Sync {
    /// The disposition of an action with this lifetime.
    fn disposition(&self, lifetime: EffectLifetime) -> EffectDisposition;
}

/// The policy a run enforces when no other is installed: every action whose
/// effect leaves the process owes a human decision.
///
/// [`EffectLifetime::Local`] actions are [`EffectDisposition::Reversible`];
/// everything else is [`EffectDisposition::NeedsConfirmation`]. A test that
/// drives an external stub installs its own policy explicitly, so the suite
/// never leans on a default to run an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DefaultPolicy;

impl Default for DefaultPolicy {
    /// The default policy, for a caller that installs it by name: a run whose
    /// external acts must ask even inside a scope that allowed them.
    fn default() -> Self {
        Self
    }
}

impl EffectPolicy for DefaultPolicy {
    /// `Reversible` for local effects, `NeedsConfirmation` for everything
    /// else: an effect whose lifetime nobody classified leaves the process
    /// until proven otherwise.
    fn disposition(&self, lifetime: EffectLifetime) -> EffectDisposition {
        match lifetime {
            EffectLifetime::Local => EffectDisposition::Reversible,
            _ => EffectDisposition::NeedsConfirmation,
        }
    }
}

/// Run `body` with `policy` gating every `act` it performs.
///
/// The door tests use to name the policy an external action runs under, with
/// the reason beside it: the default ([`DefaultPolicy`]) gates every external
/// action, and a test that drives a stub installs its own explicitly rather
/// than leaning on the default to run an effect.
pub async fn with_effect_policy<P, R>(policy: P, body: impl Future<Output = R>) -> R
where
    P: EffectPolicy + 'static,
{
    EFFECT_POLICY.scope(Some(Arc::new(policy)), body).await
}

/// The policy this run's `act` steps answer to: the installed one, or the
/// default that gates every external action.
pub(crate) fn installed_policy() -> Arc<dyn EffectPolicy> {
    let installed = EFFECT_POLICY.try_with(Clone::clone);
    if let Err(ref access) = installed {
        lgwks_std::trace::debug!(error = ?access, "installed_policy: polled outside every scope, so the default policy governs");
    }
    match installed.ok().flatten() {
        Some(policy) => policy,
        None => Arc::new(DefaultPolicy),
    }
}
