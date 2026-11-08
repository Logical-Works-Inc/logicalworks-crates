//! Serving an [`App`](super::App) over HTTP: the world edge's framing, tenant
//! prefix and refusals, with the app's cell host behind it.

use std::future::IntoFuture as _;
use std::net::TcpListener;
use std::sync::Arc;

use lgwks_deps::axum::Router;
use lgwks_deps::axum::extract::{Request as HttpRequest, State};
use lgwks_deps::axum::http::{HeaderMap, HeaderValue, header};
use lgwks_deps::axum::response::Response as HttpResponse;

use super::auth::{Authenticate, RequestHeaders};
use super::transition::{AppCell, AppMessage};
use super::{AppError, Method, Principal, normalise_path, refused};
use crate::cell::{CellHost, DynHost, PendingReply, ReplySlot, SubmitRefused};
use crate::rt::edge::{
    EdgeStop, answer, answer_pending, liveness, read_body, refused_answer, tenant_of,
};
use crate::rt::sync::CancellationToken;

/// What every request of one serve reads.
struct Edge<S: Clone + Default + Send + 'static> {
    /// The tenants.
    host: Arc<CellHost<AppCell<S>>>,
    /// Who decides who a request is, when the app names one.
    authenticator: Option<Authenticate>,
    /// Whether the edge still accepts, for a liveness probe.
    stop: CancellationToken,
}

/// Serve `host` on `listener` until `stop`, then close the host.
pub(super) fn serve_host<S>(
    host: Arc<CellHost<AppCell<S>>>,
    authenticator: Option<Authenticate>,
    listener: TcpListener,
    stop: &EdgeStop,
) -> Result<(), AppError>
where
    S: Clone + Default + Send + 'static,
{
    let edge = Edge {
        host: Arc::clone(&host),
        authenticator,
        stop: stop.token().clone(),
    };
    let served = serve_http(edge, listener, stop);
    host.close();
    served
}

/// axum on the process driver until `stop` is given and every connection
/// has drained.
fn serve_http<S>(edge: Edge<S>, listener: TcpListener, stop: &EdgeStop) -> Result<(), AppError>
where
    S: Clone + Default + Send + 'static,
{
    let ready = listener
        .set_nonblocking(true)
        .and_then(|()| crate::rt::runtime::driver());
    let handle = match ready {
        Ok(handle) => handle,
        Err(cause) => return refused(AppError::ServeIo { cause }),
    };
    let router = Router::new()
        .fallback(ingress::<S>)
        .with_state(Arc::new(edge));
    let token = stop.token().clone();
    let served = handle.block_on(async move {
        let listener = lgwks_deps::tokio::net::TcpListener::from_std(listener)?;
        lgwks_deps::axum::serve(listener, router)
            .with_graceful_shutdown(token.cancelled_owned())
            .into_future()
            .await
    });
    match served {
        Ok(()) => Ok(()),
        Err(cause) => refused(AppError::ServeIo { cause }),
    }
}

/// Who `headers` say this request is, or the answer that refuses it.
fn admit(
    authenticator: &Authenticate,
    tenant: &str,
    headers: &HeaderMap,
) -> Result<Principal, Box<HttpResponse>> {
    let Some(view) = RequestHeaders::bounded(headers) else {
        lgwks_std::trace::debug!(tenant, "app: a request head was over the ceiling");
        return Err(Box::new(answer(
            431,
            b"the request head is over the edge's ceiling".to_vec(),
        )));
    };
    match authenticator(tenant, &view) {
        Some(who) => Ok(who),
        None => {
            lgwks_std::trace::debug!(tenant, "app: a request was not authenticated");
            let mut unknown = answer(401, b"name yourself".to_vec());
            unknown
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            Err(Box::new(unknown))
        }
    }
}

/// Queue one request on its tenant's cell and await the answer.
async fn ingress<S>(State(edge): State<Arc<Edge<S>>>, request: HttpRequest) -> HttpResponse
where
    S: Clone + Default + Send + 'static,
{
    let (parts, body) = request.into_parts();
    if let Some(probe) = liveness(parts.method.as_str(), parts.uri.path(), &edge.stop) {
        return probe;
    }
    let Some((tenant, rest)) = tenant_of(parts.uri.path()) else {
        return answer(400, b"name a tenant: /t/{tenant}/...".to_vec());
    };
    if !crate::ecs::valid_tenant(tenant) {
        return refused_answer(SubmitRefused::Tenant);
    }
    let principal = match edge.authenticator {
        Some(ref authenticator) => match admit(authenticator, tenant, &parts.headers) {
            Ok(who) => Some(who),
            Err(refusal) => return *refusal,
        },
        None => None,
    };
    let Some(method) = Method::parse(parts.method.as_str()) else {
        return answer(405, b"an app route answers GET or POST".to_vec());
    };
    let body = match read_body(body).await {
        Ok(body) => body,
        Err(over) => return *over,
    };
    let message = AppMessage::Route {
        tenant: tenant.into(),
        method,
        path: normalise_path(rest),
        body: body.as_ref().into(),
        principal,
    };
    let reply = Arc::new(ReplySlot::new());
    match edge.host.try_submit(tenant, message, Arc::clone(&reply)) {
        Ok(()) => answer_pending(PendingReply::new(reply)).await,
        Err(refusal) => refused_answer(refusal),
    }
}
