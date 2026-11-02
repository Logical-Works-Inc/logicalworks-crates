//! The `serve` word's runtime: the tenant HTTP edge as a flow step.
//!
//! The language reads the words; this module runs them. [`serve_at`] serves
//! the routes a `serve` block declared, over [`ServeOptions`], and answers
//! [`ServeReport`]. The heavy edge lives in [`crate::rt::serve`]; this is the
//! thin flow step over it, so the word expands to one call here.

pub use crate::rt::serve::{ServeError, ServeOptions, ServeReport};

use crate::script::{FlowError, Scope, Site};

/// Serve the routes a `serve` block declared, until the scope stops.
///
/// Enters the step's own scope, resolves the port and ceiling the word
/// declared, serves every `route` under it with tenant isolation and
/// idempotency, drains within the declared deadline on stop, and reports
/// what drained and what was cancelled. Called by `script!`.
pub async fn serve_at(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    options: ServeOptions,
    routes: Vec<crate::rt::serve::TypedRoute>,
) -> Result<ServeReport, FlowError> {
    let child = scope.enter_at(step, site)?;
    let port = options.port();
    let listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}")).map_err(|cause| {
        FlowError::failed(format!("the serve listener could not bind: {cause}"))
    })?;
    let outcome = crate::rt::serve::serve_async(options, routes, listener, child.token().clone())
        .await
        .map_err(|cause| FlowError::failed(format!("the serve edge refused: {cause}")));
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}
