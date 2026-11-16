//! The estate smoke's doubles, its error type and its two directory helpers.

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::rt::estate::EstateError;
use lgwks_bot::verb::{EffectLifetime, Execute, Observe};

/// A source that answers once and requires `bot.net`, so the grant-replacement
/// arm (live revocation) has an authority to withdraw mid-run.
pub(super) struct SmokeSource {
    /// The one capability this source requires.
    caps: [Cap; 1],
}

impl SmokeSource {
    /// A source that requires `bot.net`.
    pub(super) fn gated() -> Self {
        Self { caps: [Cap::net()] }
    }
}

impl Observe for SmokeSource {
    /// The observed value.
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(self.required_caps())?;
        Ok(503)
    }

    fn domain_id(&self) -> &str {
        "smoke::source"
    }
}

/// An action that counts its invocations: the observable effect a live
/// revocation has to stop.
pub(super) struct SmokeAction {
    /// How many times the action ran.
    pub(super) fired: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Execute for SmokeAction {
    /// The observed value it consumes.
    type Input = u16;
    /// It answers nothing.
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u16)) -> Result<(), EcsError> {
        call.0.check(Execute::required_caps(self))?;
        self.fired.set(self.fired.get().saturating_add(1));
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "smoke::action"
    }
}

/// Create `path` and its parents. A synchronous frame of its own, so the async
/// smoke body calls no blocking file operation inline.
pub(super) fn make_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Remove `path` and what is under it; a refusal is logged, because the smoke
/// is past needing the directory.
pub(super) fn remove_dir(path: &std::path::Path) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        lgwks_std::trace::debug!(error = ?error, "estate-smoke: the denied dir was not removed");
    }
}

/// Why the smoke binary failed.
pub(super) enum SmokeError {
    /// A write to stdout or a scratch file failed.
    Io(std::io::Error),
    /// An estate tier refused to build or drain.
    Estate(EstateError),
    /// A reference command (`serve`, `apps`, `approvals`, `sealed`) failed.
    Command {
        /// The command word.
        name: &'static str,
        /// What it reported.
        cause: String,
    },
    /// A step of the evening failed, or the evening proved nothing.
    Step(String),
}

impl SmokeError {
    /// A failed reference command, named by its word.
    pub(super) fn command(name: &'static str, cause: &dyn std::fmt::Display) -> Self {
        Self::Command {
            name,
            cause: cause.to_string(),
        }
    }
}

impl std::fmt::Display for SmokeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Io(ref cause) => write!(formatter, "i/o: {cause}"),
            Self::Estate(ref cause) => write!(formatter, "estate: {cause}"),
            Self::Command { name, ref cause } => write!(formatter, "{name}: {cause}"),
            Self::Step(ref cause) => formatter.write_str(cause),
        }
    }
}

/// `main` prints its error through `Debug`, so the reader gets the sentence.
impl std::fmt::Debug for SmokeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for SmokeError {}

impl From<std::io::Error> for SmokeError {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

impl From<EstateError> for SmokeError {
    fn from(cause: EstateError) -> Self {
        Self::Estate(cause)
    }
}

impl From<String> for SmokeError {
    fn from(cause: String) -> Self {
        Self::Step(cause)
    }
}

impl From<&str> for SmokeError {
    fn from(cause: &str) -> Self {
        Self::Step(cause.to_owned())
    }
}
