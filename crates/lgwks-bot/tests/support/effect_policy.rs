//! An explicit effect policy for tests driving stub actions.
//!
//! The default policy gates every action whose effect leaves the process
//! behind a human decision; a test whose stubs are doubles installs this
//! instead, with the reason beside the installation, so the suite never leans
//! on the default to run an effect.

use lgwks_bot::script::{EffectDisposition, EffectPolicy};
use lgwks_bot::verb::EffectLifetime;

/// Every action is reversible: the suite owns its doubles.
pub struct AllowExternal;

impl EffectPolicy for AllowExternal {
    /// Reversible, whatever the lifetime: a test double owes no decision.
    fn disposition(&self, _lifetime: EffectLifetime) -> EffectDisposition {
        EffectDisposition::Reversible
    }
}

/// Drive `body` with [`AllowExternal`] installed: the one door a test uses to
/// run its stub actions, so each call site names the reason once, here.
pub(crate) async fn allowing<R>(body: impl std::future::Future<Output = R>) -> R {
    lgwks_bot::script::with_effect_policy(AllowExternal, body).await
}
