//! `fetch`: one HTTP exchange as a step of a flow.
//!
//! A `fetch` line is a GET or POST through [`lgwks_std::http`], read under a
//! caller-declared response-size bound and an attempt budget that draws from
//! the run's shared retry budget — the same [`Policy`](super::Policy) every
//! [`retry`](super::retry) block spends — so a fetch inside a `retry` and a
//! fetch with its own attempts never multiply into a storm. There is no retry
//! loop outside that budget: each retry of one fetch spends one shared retry,
//! and a spent budget ends the fetch [`Throttled`](super::FlowError::Throttled)
//! rather than with another attempt.
//!
//! # Bounds
//!
//! The URL is validated before any socket opens. The body is refused while it
//! streams, never buffered whole, past `max_body_bytes`. Each attempt races
//! `deadline` when the word declares one; without one the exchange inherits
//! whatever bounds the enclosing `within` and the transport's own per-phase
//! timeout impose. A body past the ceiling is
//! [`InvalidBound`](super::FlowError::InvalidBound), a slow upstream is
//! [`TimedOut`](super::FlowError::TimedOut), a dropped connection is
//! [`Transient`](super::FlowError::Transient), and a 5xx that outlasts the
//! budget is [`Exhausted`](super::FlowError::Exhausted) naming the last
//! status. Tenants never share a budget, a retry count or a response: two
//! tenants key different steps, hold different policies, and no response is
//! cached anywhere.
//!
//! A POST hands an effect out, so under an [`EffectPolicy`](super::EffectPolicy)
//! that does not answer [`Reversible`](super::EffectDisposition::Reversible)
//! it runs only on a matching approval: the POST body is archived exactly as
//! an `await approval` block archives its payload, so a decision parked over
//! those bytes releases it. A GET is an observation and asks no decision.

use std::time::Duration;

use lgwks_std::http::{BodyPolicy, Client, Options, PoolConfig};

use crate::cap::Cap;
use crate::journal::frame::SaturatingFrom;
use crate::verb::EffectLifetime;

use super::control::attempts;
use super::domain::gate_approval;
use super::run_store::Durable;
use super::{FlowError, Scope, Site};

/// The most response bytes one fetch may declare interest in.
///
/// A remote server authors its own response, so an unbounded read lets it
/// author this process's memory too; this ceiling is the largest bound a word
/// may declare, and a larger one is refused rather than clamped, so a caller
/// that asked for more learns the limit instead of silently getting less.
const MAX_FETCH_BODY_BYTES: usize = 64 * 1024 * 1024;

/// The per-phase transport timeout when the word declares no deadline.
///
/// The exchange still ends: this is the transport's own bound on one phase,
/// not a replacement for a declared deadline.
const FETCH_PHASE_TIMEOUT: Duration = Duration::from_secs(30);

/// The wait between two attempts of one fetch, before doubling.
///
/// The first retry waits this long, the next twice it, saturating rather than
/// overflowing; the wait races cancellation, so a stopped run stops waiting.
const FETCH_RETRY_WAIT: Duration = Duration::from_millis(100);

/// The body of a `fetch` POST: the content type and the bytes posted.
///
/// Built whole by [`PostBody::new`] and read only by the exchange, so a body
/// handed to [`FetchOptions::post`] is exactly the bytes approved and sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostBody {
    /// The `Content-Type` the bytes are posted as.
    content_type: String,
    /// The bytes posted.
    body: Vec<u8>,
}

impl PostBody {
    /// `body` posted as `content_type`.
    #[must_use]
    pub fn new(content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            content_type: content_type.into(),
            body,
        }
    }
}

/// What one `fetch` line runs under.
///
/// Built as a value, so the word `fetch url with (options)` names its bounds
/// where the call stands: a deadline for each attempt, a response-size bound
/// refused while the body streams, an attempt budget drawn from the run's
/// shared retries, request headers, and the POST body that turns the exchange
/// from a GET into a POST. `Default` is a single GET attempt with no deadline
/// of its own — inheriting the enclosing `within` and `retry` — under a
/// one-mebibyte body bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOptions {
    /// The bound on each attempt, or `None` to inherit the enclosing blocks'.
    deadline: Option<Duration>,
    /// The most response bytes the exchange may read, refused while streaming.
    max_body_bytes: usize,
    /// How many attempts the exchange may make, within the shared budget.
    attempts: u32,
    /// Extra request headers, in the order they are sent.
    headers: Vec<(String, String)>,
    /// The POST body, or `None` for a GET.
    post: Option<PostBody>,
}

impl FetchOptions {
    /// Race each attempt against `deadline`.
    #[must_use]
    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Read at most `bytes` of response body, refused while it streams; zero
    /// or past the ceiling is refused when the fetch runs.
    #[must_use]
    pub fn max_body_bytes(mut self, bytes: usize) -> Self {
        self.max_body_bytes = bytes;
        self
    }

    /// Make at most `attempts` attempts, each retry drawn from the run's
    /// shared budget; outside `1..=1000` is refused when the fetch runs.
    #[must_use]
    pub fn attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    /// Send the header `name: value`, after any added before it.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// POST `body` instead of a GET; the policy gates it as an external act.
    #[must_use]
    pub fn post(mut self, body: PostBody) -> Self {
        self.post = Some(body);
        self
    }
}

impl Default for FetchOptions {
    /// A single GET attempt under a one-mebibyte body bound.
    fn default() -> Self {
        Self {
            deadline: None,
            max_body_bytes: 1024 * 1024,
            attempts: 1,
            headers: Vec::new(),
            post: None,
        }
    }
}

/// What a `fetch` answered: status, headers and the whole bounded body.
///
/// The body is whole by construction: a body that reached the bound is
/// refused while it streams rather than handed back as a prefix, so a caller
/// holding these bytes holds everything the upstream sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    /// The HTTP status code, 200 to 299.
    status: u16,
    /// The response headers, in upstream order.
    headers: Vec<(String, String)>,
    /// The whole response body, at most the declared bound.
    body: Vec<u8>,
}

impl FetchResponse {
    /// The HTTP status code, 200 to 299.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The response headers, in upstream order.
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The whole response body, at most the declared bound.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// `fetch url with (options)`: one HTTP exchange as a step of its own.
///
/// Enters the `fetch` step, checks the run holds `bot.net`, gates a POST
/// behind the installed effect policy, and runs the exchange under `options`.
///
/// # Errors
///
/// [`FlowError::Blocked`] without the grant; [`FlowError::NeedsApproval`]
/// for a POST the policy gates and no approval matches;
/// [`FlowError::InvalidBound`] for an attempt count outside `1..=1000` or a
/// body bound of zero or past the 64 MiB ceiling;
/// [`FlowError::Failed`] for a malformed URL, a non-success status outside
/// the 5xx range, or a transport the policy cannot retry; and the attempt
/// failures [`FlowError::TimedOut`], [`FlowError::Transient`],
/// [`FlowError::Exhausted`] and [`FlowError::Throttled`] as documented above.
///
/// # Examples
///
/// ```rust
/// lgwks_bot::script! {
///     pub flow probe(url: &str) -> String:
///         let answered = fetch url with (lgwks_bot::script::FetchOptions::default())
///         give back format!("status {}", answered.status())
/// }
///
/// # fn main() -> Result<(), lgwks_bot::script::FlowError> {
/// let scope = lgwks_bot::script::Scope::root(lgwks_bot::script::Tenant::new("acme")?);
/// let refused = lgwks_bot::block_on(probe(&scope, "not a url"));
/// assert!(refused.is_err(), "a malformed URL is refused before any socket opens");
/// # Ok(())
/// # }
/// ```
pub async fn fetch(
    scope: &Scope,
    step: &str,
    url: &str,
    options: &FetchOptions,
) -> Result<FetchResponse, FlowError> {
    let child = scope.enter(step)?;
    let outcome = fetch_in(&child, url, options).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// [`fetch`] for a `fetch <url> [with (options)]` line that `script!` wrote
/// at `site`. Called by `script!`.
///
/// # Errors
///
/// As [`fetch`], located at the step.
#[doc(hidden)]
pub async fn fetch_at(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    url: &str,
    options: &FetchOptions,
) -> Result<FetchResponse, FlowError> {
    let child = scope.enter_at(step, site)?;
    let outcome = fetch_in(&child, url, options).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// The exchange itself, in the step `scope` already entered.
async fn fetch_in(
    scope: &Scope,
    url: &str,
    options: &FetchOptions,
) -> Result<FetchResponse, FlowError> {
    if scope.justification().is_some() {
        Box::pin(scope.justified(0)).await?;
    }
    scope.require(&[Cap::net()])?;
    let budget = attempts(options.attempts)?;
    let bound_bytes = u64::saturating_from(options.max_body_bytes);
    let bound_ceiling = u64::saturating_from(MAX_FETCH_BODY_BYTES);
    if options.max_body_bytes == 0 || options.max_body_bytes > MAX_FETCH_BODY_BYTES {
        let refusal = Err(FlowError::InvalidBound {
            what: "the fetch response body bound",
            value: bound_bytes,
            max: bound_ceiling,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: returning an error to the caller");
        return refusal;
    }
    if let Err(invalid) = lgwks_std::http::validate_url(url) {
        let refusal = Err(FlowError::failed(format!(
            "the fetch URL is not an absolute http(s) URI: {invalid:?}"
        )));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: returning an error to the caller");
        return refusal;
    }
    if options.post.is_some() {
        let archived = options
            .post
            .as_ref()
            .map(|post| post.body.to_record())
            .transpose()
            .map_err(|cause| {
                FlowError::failed(format!(
                    "this fetch POST body could not be archived: {cause}"
                ))
            })?;
        gate_approval(scope, EffectLifetime::External, archived)?;
    }
    let per_phase = phase_timeout(options);
    if per_phase.is_zero() {
        let refusal = Err(FlowError::InvalidBound {
            what: "the fetch deadline",
            value: 0,
            max: u64::MAX,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: returning an error to the caller");
        return refusal;
    }
    let mut exchange = Options::default()
        .timeout(per_phase)
        .deadline(per_phase)
        .max_body_bytes(options.max_body_bytes)
        .body_policy(BodyPolicy::Whole);
    for header in options.headers.iter() {
        exchange = exchange.header(header.0.clone(), header.1.clone());
    }
    let client = Client::with_pool(
        exchange,
        PoolConfig::default()
            .max_idle_connections(4)
            .max_idle_connections_per_host(2),
    )
    .map_err(|refused| {
        FlowError::failed(format!("the fetch client could not be built: {refused}"))
    })?;
    scope.policy().first_attempt();
    let mut attempt: u32 = 1;
    loop {
        let failed = match exchange_once(scope, url, options, &client).await {
            Ok(answered) => return Ok(answered),
            Err(failed) => failed,
        };
        if !failed.is_retryable() {
            let refusal = Err(failed);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: returning an error to the caller");
            return refusal;
        }
        // One attempt is one answer: a word that declared no retry budget
        // reports what its attempt said — a slow upstream reads `TimedOut`,
        // not `Exhausted` of one — while a word that declared retries reports
        // the budget they spent.
        if budget.get() == 1 {
            let refusal = Err(failed);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: returning an error to the caller");
            return refusal;
        }
        let spent = if attempt >= budget.get() {
            Some(Spent::Budget)
        } else if !scope.policy().take_retry() {
            Some(Spent::Run)
        } else {
            None
        };
        if let Some(spent) = spent {
            let refusal = Err(spent.refusal(attempt, failed).located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: the retry budget is spent");
            return refusal;
        }
        let wait = FETCH_RETRY_WAIT.saturating_mul(attempt);
        if !wait.is_zero()
            && scope
                .token()
                .run_until_cancelled(crate::rt::time::sleep(wait))
                .await
                .is_none()
        {
            let refusal = Err(FlowError::Cancelled {
                at: std::sync::Arc::clone(scope.shared_path()),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fetch_in: returning an error to the caller");
            return refusal;
        }
        attempt = attempt.saturating_add(1);
    }
}

/// One attempt: the exchange on the blocking pool, classified.
async fn exchange_once(
    scope: &Scope,
    url: &str,
    options: &FetchOptions,
    client: &Client,
) -> Result<FetchResponse, FlowError> {
    let url = url.to_owned();
    let post = options.post.clone();
    let owned = client.clone();
    let job = lgwks_std::task::try_spawn_blocking(move || {
        let call = owned.options().clone();
        match post {
            Some(post) => owned.post_with(&url, &post.content_type, &post.body, &call),
            None => owned.get_with(&url, &call),
        }
    });
    let exchange = match job {
        Ok(exchange) => exchange,
        Err(refused) => {
            let refusal = Err(FlowError::transient(format!(
                "no blocking thread was available for the fetch: {refused}"
            ))
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "exchange_once: returning an error to the caller");
            return refusal;
        }
    };
    let answered = exchange.await;
    classify(scope, options, answered)
}

/// The exchange's answer as a response or a typed failure.
fn classify(
    scope: &Scope,
    options: &FetchOptions,
    answered: Result<lgwks_std::http::Response, lgwks_std::http::Error>,
) -> Result<FetchResponse, FlowError> {
    match answered {
        Ok(response) => {
            if (200..300).contains(&response.status) {
                return Ok(FetchResponse {
                    status: response.status,
                    headers: response.headers().to_vec(),
                    body: response.body().to_vec(),
                });
            }
            let status = response.status;
            if (500..600).contains(&status) {
                let refusal = Err(transient_at(
                    scope,
                    format!("the upstream answered {status}"),
                ));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), status, "classify: the upstream answered 5xx");
                return refusal;
            }
            let refusal = Err(FlowError::failed(format!(
                "the upstream answered {status}, outside the 2xx range"
            ))
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), status, "classify: returning an error to the caller");
            refusal
        }
        Err(lgwks_std::http::Error::InvalidUrl) => {
            let refusal = Err(
                FlowError::failed("the fetch URL is not an absolute http(s) URI").located(scope),
            );
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: returning an error to the caller");
            refusal
        }
        Err(lgwks_std::http::Error::BodyTooLarge { limit }) => {
            let reached = u64::saturating_from(limit);
            let refusal = Err(FlowError::InvalidBound {
                what: "the fetch response body",
                value: reached,
                max: reached,
            }
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: the body reached the ceiling while streaming");
            refusal
        }
        Err(lgwks_std::http::Error::Failure { stage, kind, .. }) => {
            use lgwks_std::http::FailureKind;
            match kind {
                FailureKind::Timeout => {
                    let refusal = Err(FlowError::TimedOut {
                        at: std::sync::Arc::from(""),
                        after: phase_timeout(options),
                    }
                    .located(scope));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: the exchange timed out");
                    refusal
                }
                FailureKind::Transport | FailureKind::Interrupted => {
                    let refusal = Err(transient_at(
                        scope,
                        format!("the connection dropped during {stage:?}"),
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: the connection dropped");
                    refusal
                }
                FailureKind::InvalidRequest
                | FailureKind::Resource
                | FailureKind::RedirectLimit
                | FailureKind::InvalidUtf8
                | _ => {
                    let refusal = Err(FlowError::failed(format!(
                        "the fetch failed during {stage:?}: {kind:?}"
                    ))
                    .located(scope));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: returning an error to the caller");
                    refusal
                }
            }
        }
        Err(lgwks_std::http::Error::RedirectLimitTooLarge { requested, maximum }) => {
            let refusal = Err(FlowError::failed(format!(
                "the fetch redirect limit {requested} exceeds the maximum {maximum}"
            ))
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: returning an error to the caller");
            refusal
        }
        Err(other) => {
            let refusal = Err(FlowError::failed(format!(
                "the fetch failed with a newer transport refusal: {other:?}"
            ))
            .located(scope));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "classify: returning an error to the caller");
            refusal
        }
    }
}

/// Which retry budget a failing fetch ran out of.
#[derive(Debug, Clone, Copy)]
enum Spent {
    /// The word's own declared attempts.
    Budget,
    /// The run's shared retry budget, which other steps also draw on.
    Run,
}

impl Spent {
    /// The typed refusal for this budget, carrying the last failure.
    fn refusal(self, attempts: u32, last: FlowError) -> FlowError {
        let at = std::sync::Arc::from("");
        let last = Box::new(last);
        match self {
            Spent::Budget => FlowError::Exhausted { at, attempts, last },
            Spent::Run => FlowError::Throttled { at, attempts, last },
        }
    }
}

/// The per-phase timeout the word declared, or the default when it declared
/// none: one reading, so the exchange's bound and a timeout's report agree.
fn phase_timeout(options: &FetchOptions) -> std::time::Duration {
    match options.deadline {
        Some(declared) => declared,
        None => FETCH_PHASE_TIMEOUT,
    }
}

/// A retryable fetch failure at this step.
fn transient_at(scope: &Scope, reason: String) -> FlowError {
    FlowError::Transient {
        at: std::sync::Arc::clone(scope.shared_path()),
        reason,
    }
}
