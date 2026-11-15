//! The HTTP request ingress, idempotency key generation, and submission driver.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lgwks_deps::axum::{
    extract::{Request as HttpRequest, State},
    http::HeaderMap,
    response::Response as HttpResponse,
};

use lgwks_std::hash::Hasher;
use lgwks_std::wire::{Archive, Deserialize, Serialize};

use lgwks_deps::tokio::sync::OwnedSemaphorePermit;

use super::table::RouteTask;
use super::{MAX_RUNS_IN_FLIGHT_U64, RETRY_AFTER_SECS, ServeError, Shared, invalid_bound};
use crate::effect::InputIdentity;
use crate::rt::edge::{answer, read_body, tenant_of};
use crate::task::{Host, RequestError, RequestKey, Submission};

/// The request as the Host request-key path sees it.
///
/// One route task serves every tenant, so the tenant and content type ride in
/// the input rather than in the task's closure. Only the body is the request's
/// identity: the tenant already keys the run through the host, and the content
/// type is how the handler reads the body, not a second request.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug))]
pub(crate) struct ServeInput {
    /// The raw body bytes the edge read.
    pub(crate) body: Vec<u8>,
    /// The tenant the path named; not part of the identity.
    pub(crate) tenant: String,
    /// The request's `Content-Type`, when it sent one; not part of the identity.
    pub(crate) content_type: Option<String>,
}

impl InputIdentity for ServeInput {
    /// The schema binding this digest to serve bodies.
    const SCHEMA_ID: &'static [u8] = b"lgwks.bot.schema.v1.serve-body";

    /// The body bytes, framed; the tenant and content type are not identity.
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
    shared.hosts.host(tenant, || build_host(shared, tenant))
}

/// Build `tenant`'s host over the shared run store dir.
fn build_host(shared: &Arc<Shared>, tenant: &str) -> Result<Host, ServeError> {
    let bound = match std::num::NonZeroUsize::new(shared.max_in_flight) {
        Some(bound) => bound,
        None => {
            return invalid_bound("in-flight ceiling", 0, MAX_RUNS_IN_FLIGHT_U64);
        }
    };
    Host::builder(tenant)
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
        })
}

/// One admitted request, detached from its connection so a client that
/// hangs up cannot drop the run half way (see `runs`).
struct Admitted {
    /// The serve's shared state, read for the stop token.
    shared: Arc<Shared>,
    /// The tenant's host.
    host: Host,
    /// The route's task, built once at start.
    route: Arc<RouteTask>,
    /// The idempotency key.
    key: RequestKey,
    /// The tenant, content type and body the route's task is run with.
    input: ServeInput,
    /// The in-flight permit, released when the run ends.
    permit: OwnedSemaphorePermit,
    /// The live count, released when the run ends.
    live: LiveGuard,
}

impl Admitted {
    /// Run the request once through the tenant's host, on the caller's task.
    async fn drive(self) -> SubmitOutcome {
        let Self {
            shared,
            host,
            route,
            key,
            input,
            permit,
            live,
        } = self;
        let _held = (permit, live);
        if shared.token.is_cancelled() {
            return SubmitOutcome::Stopping;
        }
        outcome_of(host.submit(&key, route.as_ref(), input).await)
    }
}

/// What a finished submission concluded, in owned bytes.
fn outcome_of(submitted: Result<Submission<ServeOutput>, RequestError>) -> SubmitOutcome {
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
    let verb = parts.method.as_str();
    let key = idempotency_key(&parts.headers, tenant, verb, rest, &body);
    let key = match RequestKey::new(&key) {
        Ok(key) => key,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the idempotency key is refused");
            return answer(400, b"the idempotency key is not a valid name".to_vec());
        }
    };
    let Ok(permit) = Arc::clone(&shared.room).try_acquire_owned() else {
        lgwks_std::trace::debug!("serve: past the in-flight ceiling; answering 503");
        return answer_retry(503, b"the serve is past its in-flight ceiling".to_vec());
    };
    let Some(route) = shared.routes.find(verb, rest) else {
        return answer(404, b"no route answers this method and path".to_vec());
    };
    if shared.token.is_cancelled() {
        return answer(503, b"the serve is stopping".to_vec());
    }
    let host = match host_for(&shared, tenant) {
        Ok(host) => host,
        Err(ServeError::TooManyTenants { cap }) => {
            lgwks_std::trace::debug!(cap, "serve: the tenant table is full; answering 503");
            return answer_retry(503, b"the serve holds its most tenants".to_vec());
        }
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the tenant host is refused");
            return answer(500, b"the tenant host is refused".to_vec());
        }
    };
    let content_type = parts
        .headers
        .get(lgwks_deps::axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(str::to_owned);
    let (reply, answered) = lgwks_deps::tokio::sync::oneshot::channel();
    let admitted = Admitted {
        host,
        route,
        key,
        input: ServeInput {
            body: body.to_vec(),
            tenant: tenant.to_owned(),
            content_type,
        },
        permit,
        live: LiveGuard::enter(&shared),
        shared: Arc::clone(&shared),
    };
    shared.runs.spawn(async move {
        let outcome = admitted.drive().await;
        if let Err(unsent) = reply.send(outcome) {
            drop(unsent);
            lgwks_std::trace::debug!("serve: the client left before its run answered");
        }
    });
    let outcome = match answered.await {
        Ok(outcome) => outcome,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the run ended without an answer");
            SubmitOutcome::Refused {
                message: String::from("the run ended without an answer"),
            }
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

#[cfg(test)]
mod tests {
    use super::{Hasher, InputIdentity as _, ServeInput};

    /// The identity digest of `input`.
    fn digest_of(input: &ServeInput) -> Vec<u8> {
        let mut hasher = Hasher::new();
        input.write_identity(&mut hasher);
        hasher.finalize().as_bytes().to_vec()
    }

    /// Requirement: only the body is the request's identity (#1008).
    ///
    /// The tenant and content type ride in the input so one task serves every
    /// request; a digest that took them in would turn the same key and body from
    /// another content type into a conflict, which the edge never did.
    #[test]
    fn only_the_body_is_the_identity_of_a_serve_input() {
        let plain = ServeInput {
            body: b"alpha".to_vec(),
            tenant: String::from("acme"),
            content_type: None,
        };
        let moved = ServeInput {
            body: b"alpha".to_vec(),
            tenant: String::from("other"),
            content_type: Some(String::from("application/json")),
        };
        let changed = ServeInput {
            body: b"beta".to_vec(),
            ..plain.clone()
        };
        assert_eq!(
            digest_of(&plain),
            digest_of(&moved),
            "tenant or content type entered the digest"
        );
        assert_ne!(
            digest_of(&plain),
            digest_of(&changed),
            "the body left the digest"
        );
    }
}
