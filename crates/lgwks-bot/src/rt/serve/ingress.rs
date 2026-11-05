//! The HTTP request ingress, idempotency key generation, and submission driver.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use lgwks_ast::script::RouteMethod;
use lgwks_deps::axum::{
    extract::{Request as HttpRequest, State},
    http::HeaderMap,
    response::Response as HttpResponse,
};

use lgwks_std::hash::Hasher;
use lgwks_std::wire::{Archive, Deserialize, Serialize};

use super::{MAX_RUNS_IN_FLIGHT_U64, RETRY_AFTER_SECS, ServeError, Shared, invalid_bound};
use crate::effect::InputIdentity;
use crate::rt::edge::{answer, read_body, tenant_of};
use crate::script::Scope;
use crate::task::{Host, RequestError, RequestKey, Submission, task};

/// The request body as the Host request-key path sees it.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug))]
pub(crate) struct ServeInput {
    /// The raw body bytes the edge read.
    pub(crate) body: Vec<u8>,
}

impl InputIdentity for ServeInput {
    /// The schema binding this digest to serve bodies.
    const SCHEMA_ID: &'static [u8] = b"lgwks.bot.schema.v1.serve-body";

    /// The body bytes, framed.
    fn write_identity(&self, hasher: &mut Hasher) {
        hasher.write_framed(self.body.as_slice());
    }
}

/// The answered route as the Host request-key path records it.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug))]
pub(crate) struct ServeOutput {
    /// The HTTP status to answer.
    pub(crate) status: u16,
    /// The response bytes.
    pub(crate) body: Vec<u8>,
}

/// What a driven request-key submission concluded, in owned bytes.
enum SubmitOutcome {
    /// Answer with this status and body.
    Answered {
        /// The HTTP status to answer.
        status: u16,
        /// The response body.
        body: Vec<u8>,
    },
    /// The first run is still in flight; retry with the same key.
    InFlight,
    /// The key is bound to a different input.
    Conflict {
        /// What collided, naming the key and both digests.
        message: String,
    },
    /// The submission itself was refused.
    Refused {
        /// What refused it.
        message: String,
    },
    /// The serve stopped before the run started; the run was never admitted.
    Stopping,
}

/// An in-flight request's live count, released in `Drop`.
struct LiveGuard {
    /// The shared counts this request entered.
    shared: Arc<Shared>,
}

impl LiveGuard {
    /// Enter one in-flight request.
    fn enter(shared: &Arc<Shared>) -> Self {
        shared.live.fetch_add(1, Ordering::SeqCst);
        Self {
            shared: Arc::clone(shared),
        }
    }
}

impl Drop for LiveGuard {
    /// Leave, however the handler future ended: completion, error, or drop.
    fn drop(&mut self) {
        self.shared.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The idempotency key for this request: the client's header, else the digest
/// of tenant, route and body.
fn idempotency_key(
    headers: &HeaderMap,
    tenant: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> String {
    if let Some(header) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
    {
        let trimmed = header.trim();
        if !trimmed.is_empty() && trimmed.len() <= 128 {
            let allowed = trimmed
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte));
            if allowed {
                return trimmed.to_owned();
            }
        }
    }
    let mut hasher = Hasher::new();
    hasher.write_framed(b"lgwks.bot.serve.v1");
    hasher.write_framed(tenant.as_bytes());
    hasher.write_framed(method.as_bytes());
    hasher.write_framed(path.as_bytes());
    hasher.write_framed(body);
    let digest = hasher.finalize();
    lgwks_std::hex::encode(digest.as_bytes())
}

/// The host for `tenant`, built once over the shared run store dir.
fn host_for(shared: &Arc<Shared>, tenant: &str) -> Result<Host, ServeError> {
    let mut hosts = shared.hosts.lock().map_err(|cause| {
        let refusal = ServeError::Host {
            reason: format!("the tenant hosts are poisoned: {cause}"),
        };
        lgwks_std::trace::debug!(error = ?refusal, "serve: the host lock is refused");
        refusal
    })?;
    if let Some(host) = hosts.get(tenant) {
        return Ok(host.clone());
    }
    let bound = match std::num::NonZeroUsize::new(shared.max_in_flight) {
        Some(bound) => bound,
        None => {
            return invalid_bound("in-flight ceiling", 0, MAX_RUNS_IN_FLIGHT_U64);
        }
    };
    let stored = Host::builder(tenant)
        .map_err(|cause| {
            let refusal = ServeError::Host {
                reason: format!("the tenant is refused: {cause}"),
            };
            lgwks_std::trace::debug!(error = ?refusal, "serve: the host builder is refused");
            refusal
        })?
        .max_concurrent_tasks(bound)
        .run_store(shared.store_dir.as_path())
        .map_err(|cause| {
            let refusal = ServeError::Io(std::io::Error::other(format!(
                "the run store is refused: {cause}"
            )));
            lgwks_std::trace::debug!(error = ?refusal, "serve: the run store is refused");
            refusal
        })?
        .build()
        .map_err(|cause| {
            let refusal = ServeError::Host {
                reason: format!("the host is refused: {cause}"),
            };
            lgwks_std::trace::debug!(error = ?refusal, "serve: the host is refused");
            refusal
        })?;
    hosts.insert(tenant.to_owned(), stored.clone());
    Ok(stored)
}

std::thread_local! {
    static SUBMIT_RUNTIME: std::cell::RefCell<Option<lgwks_deps::tokio::runtime::Runtime>> =
        const { std::cell::RefCell::new(None) };
}

/// Drive `future` — one `Host::submit` — to completion on this thread's
/// private runtime.
fn block_on_submit<Fut>(future: Fut) -> Result<Fut::Output, std::io::Error>
where
    Fut: Future,
{
    SUBMIT_RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let built = lgwks_deps::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            *slot = Some(built);
        }
        let Some(runtime) = slot.as_ref() else {
            let refusal = Err(std::io::Error::other(
                "the submit runtime slot is empty after building it",
            ));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "serve: the submit runtime is refused");
            return refusal;
        };
        Ok(runtime.block_on(future))
    })
}

/// One request: tenant from the path, one run through the tenant's host.
pub(crate) async fn ingress(
    State(shared): State<Arc<Shared>>,
    request: HttpRequest,
) -> HttpResponse {
    let (parts, body) = request.into_parts();
    let Some((tenant, rest)) = tenant_of(parts.uri.path()) else {
        return answer(400, b"name a tenant: /t/{tenant}/...".to_vec());
    };
    if !crate::ecs::valid_tenant(tenant) {
        return answer(400, b"the tenant is not a valid name".to_vec());
    }
    let body = match read_body(body).await {
        Ok(body) => body,
        Err(over) => return *over,
    };
    let verb = parts.method.as_str().to_owned();
    let path = rest.to_owned();
    let key = idempotency_key(&parts.headers, tenant, &verb, &path, &body);
    let key = match RequestKey::new(&key) {
        Ok(key) => key,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the idempotency key is refused");
            return answer(400, b"the idempotency key is not a valid name".to_vec());
        }
    };
    let Ok(_permit) = shared.room.try_acquire() else {
        lgwks_std::trace::debug!("serve: past the in-flight ceiling; answering 503");
        return answer_retry(503, b"the serve is past its in-flight ceiling".to_vec());
    };
    let method = match verb.as_str() {
        "GET" => Some(RouteMethod::Get),
        "POST" => Some(RouteMethod::Post),
        _ => None,
    };
    let Some(route) = shared
        .routes
        .iter()
        .find(|route| Some(route.method) == method && route.path.as_str() == path.as_str())
    else {
        return answer(404, b"no route answers this method and path".to_vec());
    };
    if shared.token.is_cancelled() {
        return answer(503, b"the serve is stopping".to_vec());
    }
    let host = match host_for(&shared, tenant) {
        Ok(host) => host,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the tenant host is refused");
            return answer(500, b"the tenant host is refused".to_vec());
        }
    };
    let _live = LiveGuard::enter(&shared);
    let content_type = parts
        .headers
        .get(lgwks_deps::axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(str::to_owned);
    let handler = Arc::clone(&route.handler);
    let tenant_owned = tenant.to_owned();
    let body_vec = body.to_vec();
    let stopping = Arc::clone(&shared);
    let driven = lgwks_std::task::try_spawn_blocking(move || {
        if stopping.token.is_cancelled() {
            return SubmitOutcome::Stopping;
        }
        let inner_tenant = tenant_owned.clone();
        let run_task = match task("serve-route", move |_scope: Scope, input: ServeInput| {
            let handler = Arc::clone(&handler);
            let tenant_owned = inner_tenant.clone();
            let content = content_type.clone();
            async move {
                match handler(tenant_owned, content, input.body).await {
                    Ok(bytes) => Ok(ServeOutput {
                        status: 200,
                        body: bytes,
                    }),
                    Err((status, message)) => Ok(ServeOutput {
                        status,
                        body: message.into_bytes(),
                    }),
                }
            }
        }) {
            Ok(run_task) => run_task,
            Err(cause) => {
                return SubmitOutcome::Refused {
                    message: format!("the route task is refused: {cause}"),
                };
            }
        };
        let submitted =
            match block_on_submit(host.submit(&key, &run_task, ServeInput { body: body_vec })) {
                Ok(submitted) => submitted,
                Err(cause) => {
                    return SubmitOutcome::Refused {
                        message: format!("the submit runtime is refused: {cause}"),
                    };
                }
            };
        match submitted {
            Ok(Submission::Executed(report)) | Ok(Submission::Reattached(report)) => {
                match report.output() {
                    Some(answered) => SubmitOutcome::Answered {
                        status: answered.status,
                        body: answered.body.clone(),
                    },
                    None => match report.error() {
                        Some(cause) => SubmitOutcome::Refused {
                            message: format!("the route run failed: {cause}"),
                        },
                        None => SubmitOutcome::Refused {
                            message: String::from("the route run reported nothing"),
                        },
                    },
                }
            }
            Ok(Submission::InFlight(_)) => SubmitOutcome::InFlight,
            Err(RequestError::Conflict(conflict)) => SubmitOutcome::Conflict {
                message: conflict.to_string(),
            },
            Err(cause) => SubmitOutcome::Refused {
                message: format!("the submission is refused: {cause}"),
            },
        }
    });
    let outcome = match driven {
        Ok(driven) => driven.await,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the blocking pool is full");
            return answer_retry(503, b"the serve is past its in-flight ceiling".to_vec());
        }
    };
    match outcome {
        SubmitOutcome::Answered { status, body } => {
            if status != 200 {
                lgwks_std::trace::debug!(status, "serve: the route answered an error");
            }
            shared.drained.fetch_add(1, Ordering::SeqCst);
            answer(status, body)
        }
        SubmitOutcome::InFlight => {
            lgwks_std::trace::debug!("serve: the first run is still in flight; answering 503");
            answer_retry(
                503,
                b"the first run with this key is still in flight".to_vec(),
            )
        }
        SubmitOutcome::Conflict { message } => {
            lgwks_std::trace::debug!(
                conflict = message.as_str(),
                "serve: the idempotency key conflicts"
            );
            answer(422, message.into_bytes())
        }
        SubmitOutcome::Refused { message } => {
            lgwks_std::trace::debug!(
                refusal = message.as_str(),
                "serve: the submission is refused"
            );
            answer(500, message.into_bytes())
        }
        SubmitOutcome::Stopping => {
            lgwks_std::trace::debug!(
                "serve: the serve stopped before the run started; answering 503"
            );
            answer(503, b"the serve is stopping".to_vec())
        }
    }
}

/// [`answer`] with `Retry-After`, for a full house.
pub(crate) fn answer_retry(status: u16, body: Vec<u8>) -> HttpResponse {
    let mut response = answer(status, body);
    let value = match lgwks_deps::axum::http::HeaderValue::from_str(&RETRY_AFTER_SECS.to_string()) {
        Ok(value) => value,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "serve: the retry delay cannot be a header");
            return response;
        }
    };
    response
        .headers_mut()
        .insert(lgwks_deps::axum::http::header::RETRY_AFTER, value);
    response
}
