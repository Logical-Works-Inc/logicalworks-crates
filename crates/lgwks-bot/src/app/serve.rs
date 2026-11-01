//! Serving an [`App`](super::App) over HTTP: the world edge's framing, tenant
//! prefix and refusals, with the app's cell host behind it.

use std::future::IntoFuture as _;
use std::net::TcpListener;
use std::sync::Arc;

use lgwks_deps::axum::Router;
use lgwks_deps::axum::extract::{Request as HttpRequest, State};
use lgwks_deps::axum::response::Response as HttpResponse;

use super::transition::{AppCell, AppMessage};
use super::{AppError, Method, normalise_path, refused};
use crate::cell::{CellHost, DynHost, PendingReply, ReplySlot, SubmitRefused};
use crate::rt::edge::{EdgeStop, answer, answer_pending, read_body, refused_answer, tenant_of};

/// Serve `host` on `listener` until `stop`, then close the host.
pub(super) fn serve_host<S>(
    host: Arc<CellHost<AppCell<S>>>,
    listener: TcpListener,
    stop: &EdgeStop,
) -> Result<(), AppError>
where
    S: Clone + Default + Send + 'static,
{
    let served = serve_http(Arc::clone(&host), listener, stop);
    host.close();
    served
}

/// axum on the process driver until `stop` is given and every connection
/// has drained.
fn serve_http<S>(
    host: Arc<CellHost<AppCell<S>>>,
    listener: TcpListener,
    stop: &EdgeStop,
) -> Result<(), AppError>
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
    let router = Router::new().fallback(ingress::<S>).with_state(host);
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

/// Queue one request on its tenant's cell and await the answer.
async fn ingress<S>(
    State(host): State<Arc<CellHost<AppCell<S>>>>,
    request: HttpRequest,
) -> HttpResponse
where
    S: Clone + Default + Send + 'static,
{
    let (parts, body) = request.into_parts();
    let Some((tenant, rest)) = tenant_of(parts.uri.path()) else {
        return answer(400, b"name a tenant: /t/{tenant}/...".to_vec());
    };
    if !crate::ecs::valid_tenant(tenant) {
        return refused_answer(SubmitRefused::Tenant);
    }
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
    };
    let reply = Arc::new(ReplySlot::new());
    match host.try_submit(tenant, message, Arc::clone(&reply)) {
        Ok(()) => answer_pending(PendingReply::new(reply)).await,
        Err(refusal) => refused_answer(refusal),
    }
}
