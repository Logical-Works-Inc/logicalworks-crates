//! The app SDK: one builder for routes, effects, approval and grants (#791).
//!
//! An [`App`] reads like an Express or ASP.NET minimal-API app and compiles to
//! ordinary Rust with no macro. Every route and `on` handler is a cell
//! transition on the #790 host: `Fn(&mut S, Input) -> Vec<Effect>`,
//! synchronous, with no await, no lock and no I/O, run on the worker that owns
//! the tenant. Effects are values. After a transition returns, the runner
//! answers the request from the first [`Effect::Reply`]. It serves every
//! other effect through the app's [`Provider`] only when the app was granted
//! the effect's cap, and feeds the answer back as a message to the `on`
//! handler the effect names, through the tenant's own bounded mailbox. An
//! ungranted effect is answered [`EffectResult::Refused`] and never reaches
//! the provider.
//!
//! [`App::check`] refuses an app before it serves: a name, route or handler
//! count past its ceiling, two routes on one verb and path, or a handler that
//! returns an effect whose cap the app was not granted. [`App::from_plan`]
//! builds an app from a model-written plan through the same check.
//!
//! A request names its tenant in its path, `/t/{tenant}/{rest}`, as on the
//! world's edge. Each tenant owns its own `S`, so no handler can read another
//! tenant's state. `GET /healthz` names no tenant: the edge answers it itself.
//!
//! [`App::authenticate`] judges every tenant request at the edge: a refusal is
//! `401` with nothing read or queued, and [`App::route_as`] hands the handler the
//! [`Principal`].
//!
//! [`Effect::Command`] hands a tenant's command to a durable service the host
//! owns, through [`Provider::command`] on a bounded pool of its own, and the
//! request stays open until an `on` handler answers it (ADR 0025).

use core::fmt;
use std::net::TcpListener;
use std::sync::{Arc, OnceLock};

use crate::cap::Cap;
use crate::cell::{COMMAND_CAP, CellError, CellHost, DEFAULT_MAILBOX_BOUND, EffectRunner};
use crate::gate::GrantSet;
use crate::rt::edge::EdgeStop;
use crate::task::RunStore;

mod auth;
mod check;
mod command;
mod error;
mod serve;
#[cfg(test)]
mod sim_app;
#[cfg(test)]
mod sim_command;
#[cfg(test)]
mod sim_command_faults;
mod transition;

pub use crate::cell::{Effect, EffectResult, OnInput, ReplyTag};
pub use auth::{Principal, RequestHeaders};
pub use command::CommandLimits;
pub use error::AppError;

use auth::Authenticate;
use error::refused;
use transition::{Answers, AppCell, AppRunner, OnEntry, RouteEntry};

/// A plan may carry at most this many bytes.
const MAX_PLAN_BYTES: usize = 64 * 1_024;

/// An app or a plan holds at most this many routes, and as many `on` handlers.
const MAX_ROUTES: usize = 256;

/// An app name is 1..=64 bytes.
const MAX_NAME_BYTES: usize = 64;

/// A route path is at most this many bytes.
const MAX_PATH_BYTES: usize = 256;

/// One transition dispatches at most this many effects; the rest are dropped
/// and logged.
const MAX_EFFECTS_PER_APPLY: usize = 8;

/// The HTTP verb a route answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Method {
    /// Read the tenant's state.
    GET,
    /// Fold a typed body into it.
    POST,
}

impl Method {
    /// `GET` or `POST` as the request line spells it; `None` for any other verb.
    fn parse(text: &str) -> Option<Self> {
        match text {
            "GET" => Some(Self::GET),
            "POST" => Some(Self::POST),
            _ => None,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::GET => f.write_str("GET"),
            Self::POST => f.write_str("POST"),
        }
    }
}

/// Serves the effects an app's handlers return: fetch, ask, send, approval and
/// timers.
///
/// The runner calls it only for an effect whose cap the app was granted,
/// outside the cell and never for a replayed message. It feeds the answer back
/// to the `on` handler the effect names. `None` is answered
/// [`EffectResult::Unavailable`].
pub trait Provider: Send + Sync + 'static {
    /// Serve `effect` for `tenant`: the answer's bytes, or `None`.
    ///
    /// It runs on the worker that owns the tenant, so it must not wait. An
    /// [`Effect::Command`] never arrives here: it goes to [`Provider::command`].
    fn dispatch(&self, tenant: &str, effect: &Effect) -> Option<Vec<u8>>;

    /// Serve a command for `tenant`, asked for by `principal`, and answer once it
    /// has landed; `None` is answered [`EffectResult::Unavailable`], and so is a panic.
    ///
    /// It runs on the app's command pool, off every worker, so it may block on a
    /// durable write without stalling a tenant. The principal comes from the log and
    /// not from the handler, so it cannot be forged. A client that loses its answer
    /// retries, and this can run twice for one act: the body carries an idempotency
    /// key, and this method honours it (ADR 0025).
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::{Effect, Principal, Provider};
    ///
    /// struct Ledger;
    ///
    /// impl Provider for Ledger {
    ///     fn dispatch(&self, _: &str, _: &Effect) -> Option<Vec<u8>> {
    ///         None
    ///     }
    ///
    ///     fn command(&self, tenant: &str, _: Option<&Principal>, body: &[u8]) -> Option<Vec<u8>> {
    ///         Some(format!("{tenant} wrote {} bytes", body.len()).into_bytes())
    ///     }
    /// }
    ///
    /// assert_eq!(Ledger.command("a", None, b"xy"), Some(b"a wrote 2 bytes".to_vec()));
    /// ```
    fn command(&self, tenant: &str, principal: Option<&Principal>, body: &[u8]) -> Option<Vec<u8>> {
        lgwks_std::trace::debug!(
            tenant,
            named = principal.is_some(),
            bytes = body.len(),
            "app: this provider serves no command"
        );
        None
    }
}

/// The provider of an app that names none: every effect is unavailable.
struct NoProvider;

impl Provider for NoProvider {
    fn dispatch(&self, tenant: &str, effect: &Effect) -> Option<Vec<u8>> {
        lgwks_std::trace::debug!(tenant, ?effect, "app: no provider serves this effect");
        None
    }
}

/// A per-tenant app: routes and `on` handlers as cell transitions over `S`.
pub struct App<S = ()> {
    /// The app name, 1..=64 bytes once checked.
    name: Box<str>,
    /// The caps the runner may dispatch under.
    grants: GrantSet,
    /// Who serves the effects the handlers return.
    provider: Arc<dyn Provider>,
    /// The file store an `AwaitApproval` effect parks through, when the app
    /// was given one: the park is decidable through the same path as a flow's
    /// park, over the same file.
    approvals: Option<RunStore>,
    /// Who decides who a request is, when the app names one.
    authenticator: Option<Authenticate>,
    /// The bounds commands run under.
    commands: CommandLimits,
    /// Routes in registration order.
    routes: Vec<RouteEntry<S>>,
    /// `on` handlers in registration order.
    ons: Vec<OnEntry<S>>,
}

impl App<()> {
    /// An app named `name`, holding no routes, grants or provider.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: crate::cell::clip(name, MAX_NAME_BYTES.saturating_add(1)),
            grants: GrantSet::empty(),
            provider: Arc::new(NoProvider),
            approvals: None,
            authenticator: None,
            commands: CommandLimits::default(),
            routes: Vec::new(),
            ons: Vec::new(),
        }
    }

    /// Fix the per-tenant state type this app folds: each tenant starts from
    /// `S::default()`.
    #[must_use]
    pub fn state<S>(self) -> App<S> {
        App {
            name: self.name,
            grants: self.grants,
            provider: self.provider,
            approvals: self.approvals,
            authenticator: self.authenticator,
            commands: self.commands,
            routes: Vec::new(),
            ons: Vec::new(),
        }
    }
}

impl<S> App<S>
where
    S: Clone + Default + Send + 'static,
{
    /// Answer `method` `path` with `handler`, given the body decoded as JSON `T`.
    ///
    /// A body that does not decode as `T` is answered `400` without reaching
    /// the handler; an empty body decodes as JSON `null`.
    #[must_use]
    pub fn route<T, F>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        T: lgwks_std::json::serde::de::DeserializeOwned + 'static,
        F: Fn(&mut S, T) -> Vec<Effect> + Send + Sync + 'static,
    {
        self.routes
            .push(RouteEntry::new(method, normalise_path(path), handler));
        self
    }

    /// Answer `method` `path` with `handler`, given who the request is from.
    ///
    /// [`App::check`] refuses such a route on an app with no authenticator.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::{App, Effect, Method, Principal, RequestHeaders};
    ///
    /// let app = App::new("whoami")
    ///     .state::<u64>()
    ///     .authenticate(|tenant: &str, headers: &RequestHeaders<'_>| {
    ///         let key = headers.bearer()?;
    ///         if key == format!("key-{tenant}") {
    ///             Principal::new(tenant)
    ///         } else {
    ///             None
    ///         }
    ///     })
    ///     .route_as(Method::GET, "/me", |_: &mut u64, who: &Principal, (): ()| {
    ///         vec![Effect::reply(200, who.as_str().as_bytes().to_vec())]
    ///     });
    /// assert!(app.check().is_ok());
    /// ```
    #[must_use]
    pub fn route_as<T, F>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        T: lgwks_std::json::serde::de::DeserializeOwned + 'static,
        F: Fn(&mut S, &Principal, T) -> Vec<Effect> + Send + Sync + 'static,
    {
        self.routes
            .push(RouteEntry::named(method, normalise_path(path), handler));
        self
    }

    /// Decide who every tenant request is, at the edge, with `authenticator`.
    ///
    /// It runs on the driver after the tenant name is read and before the body is, so
    /// it is synchronous and does no I/O, like a transition. `None` is answered `401`
    /// with nothing queued. A head past 64 headers or 16 KiB is answered `431` without
    /// asking. It compares a secret however its author writes it, so it should compare
    /// a digest or in constant time.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::{App, Principal, RequestHeaders};
    ///
    /// let app = App::new("locked").authenticate(|_: &str, _: &RequestHeaders<'_>| {
    ///     None::<Principal>
    /// });
    /// assert!(app.check().is_ok());
    /// ```
    #[must_use]
    pub fn authenticate<F>(mut self, authenticator: F) -> Self
    where
        F: Fn(&str, &RequestHeaders<'_>) -> Option<Principal> + Send + Sync + 'static,
    {
        self.authenticator = Some(Arc::new(authenticator));
        self
    }

    /// Answer the effect results tagged `tag` with `handler`.
    #[must_use]
    pub fn on<F>(mut self, tag: ReplyTag, handler: F) -> Self
    where
        F: Fn(&mut S, &OnInput) -> Vec<Effect> + Send + Sync + 'static,
    {
        self.ons.push(OnEntry::new(tag, handler));
        self
    }

    /// Grant `cap` to the runner.
    #[must_use]
    pub fn grant(mut self, cap: Cap) -> Self {
        self.grants = self.grants.grant(cap);
        self
    }

    /// Serve the handlers' effects through `provider`.
    #[must_use]
    pub fn provider(mut self, provider: impl Provider) -> Self {
        self.provider = Arc::new(provider);
        self
    }

    /// Park the handlers' `AwaitApproval` effects through `store`.
    ///
    /// Without a store an approval the provider cannot answer is
    /// [`EffectResult::Unavailable`] and nothing more. With one, the wait is
    /// parked under the tenant beside the answer, decidable through
    /// [`Host::decide`](crate::task::Host::decide) over the same file a flow's
    /// park is decided through — one decision path for flows and apps.
    #[must_use]
    pub fn approvals(mut self, store: RunStore) -> Self {
        self.approvals = Some(store);
        self
    }

    /// Run commands under `limits`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use lgwks_bot::Cap;
    /// use lgwks_bot::app::{App, CommandLimits};
    ///
    /// let limits = CommandLimits::new(8, 64, 16, Duration::from_secs(5));
    /// let app = App::new("ledger").grant(Cap::new("app.command")).command_limits(limits);
    /// assert!(app.check().is_ok());
    /// ```
    #[must_use]
    pub fn command_limits(mut self, limits: CommandLimits) -> Self {
        self.commands = limits;
        self
    }

    /// Check the app, then serve `listener` until `stop`, answering every
    /// tenant from its own state.
    ///
    /// # Errors
    ///
    /// Whatever [`App::check`] refuses, [`AppError::Spawn`] when no worker
    /// could start, and [`AppError::ServeIo`] when the listener fails.
    pub fn serve(self, listener: TcpListener, stop: &EdgeStop) -> Result<(), AppError> {
        let authenticator = self.authenticator.clone();
        let held_for = self.runs_commands().then(|| self.commands.deadline());
        let host = self.start(auto_workers(), DEFAULT_MAILBOX_BOUND)?;
        serve::serve_host(host, authenticator, held_for, listener, stop)
    }

    /// Whether this app was granted commands, and so owns a pool to run them on.
    fn runs_commands(&self) -> bool {
        self.grants.grants(&Cap::new(COMMAND_CAP))
    }

    /// Check the app and start its cell host on `workers` threads, the runner
    /// bound to feed answers back through it.
    fn start(
        self,
        workers: usize,
        mailbox_bound: usize,
    ) -> Result<Arc<CellHost<AppCell<S>>>, AppError> {
        self.check()?;
        let answers = Arc::new(Answers::new());
        let commands = if self.runs_commands() {
            let feed: Arc<dyn command::Feedback> = Arc::<Answers<S>>::clone(&answers);
            let provider = Arc::clone(&self.provider);
            match command::CommandPool::start(&self.commands, provider, feed) {
                Ok(pool) => Some(pool),
                Err(cause) => return refused(AppError::Spawn { cause }),
            }
        } else {
            None
        };
        let runner = Arc::new(AppRunner::new(
            self.grants,
            self.provider,
            self.approvals,
            answers,
            commands,
        ));
        let cell = AppCell::new(self.routes, self.ons);
        let erased: Arc<dyn EffectRunner> = Arc::<AppRunner<S>>::clone(&runner);
        let host = match CellHost::start(cell, erased, workers, mailbox_bound) {
            Ok(host) => Arc::new(host),
            Err(CellError::Spawn(cause)) => return refused(AppError::Spawn { cause }),
            Err(error) => {
                lgwks_std::trace::warn!(%error, "app: the host could not start");
                return refused(AppError::Closed);
            }
        };
        runner.bind(&host);
        Ok(host)
    }
}

/// The host a runner feeds answers back through, set once the host starts.
type HostSlot<S> = OnceLock<std::sync::Weak<CellHost<AppCell<S>>>>;

/// Worker threads for the app host: the machine's parallelism.
fn auto_workers() -> usize {
    match std::thread::available_parallelism() {
        Ok(threads) => threads.get(),
        Err(error) => {
            lgwks_std::trace::warn!(%error, "app: parallelism is unreadable; serving on four workers");
            4
        }
    }
}

/// A stored path: rooted at `/` and clipped to [`MAX_PATH_BYTES`].
fn normalise_path(path: &str) -> Box<str> {
    let rooted = match path.strip_prefix('/') {
        Some(rest) => rest,
        None => path,
    };
    let mut stored = String::with_capacity(rooted.len().saturating_add(1));
    stored.push('/');
    stored.push_str(&crate::cell::clip(rooted, MAX_PATH_BYTES.saturating_sub(1)));
    stored.into_boxed_str()
}
